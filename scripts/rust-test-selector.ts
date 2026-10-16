import { dirname, relative, sep } from "node:path";

export type CargoPackage = {
  name: string;
  manifest_path: string;
  dependencies: Array<{ path?: string }>;
};

export type RustTestPlan = {
  full: boolean;
  packages: string[];
  reason: string;
};

const NON_RUST_PREFIXES = [".github/", "ai-docs/", "scripts/", "src-web/", "tests/web/", "public/"];
const NON_RUST_FILES = new Set([
  "index.html",
  "vite.config.ts",
  "vitest.config.ts",
  "tsconfig.json",
]);
const FULL_TEST_FILES = new Set(["Cargo.toml", "Cargo.lock", "package.json", "bun.lock"]);
const FULL_TEST_PREFIXES = [".cargo/"];

function toRepoPath(path: string, root: string): string {
  return relative(root, path).split(sep).join("/");
}

function packageRoots(packages: CargoPackage[], root: string): Map<string, string> {
  return new Map(packages.map((pkg) => [pkg.name, toRepoPath(dirname(pkg.manifest_path), root)]));
}

function fullPlan(packages: CargoPackage[], reason: string): RustTestPlan {
  return { full: true, packages: packages.map(({ name }) => name).sort(), reason };
}

/** Maps changed files to workspace crates; uncertain paths always select the full workspace. */
export function selectRustTestPlan(
  changedPaths: string[],
  packages: CargoPackage[],
  root: string,
): RustTestPlan {
  if (changedPaths.length === 0) return fullPlan(packages, "empty change set");

  const roots = packageRoots(packages, root);
  const packageByRoot = [...roots].sort((a, b) => b[1].length - a[1].length);
  const selected = new Set<string>();

  for (const rawPath of changedPaths) {
    const path = rawPath.replaceAll("\\", "/").replace(/^\.\//, "");
    if (!path || path.startsWith("/") || path.split("/").includes("..")) {
      return fullPlan(packages, `invalid path: ${rawPath}`);
    }
    if (FULL_TEST_FILES.has(path) || FULL_TEST_PREFIXES.some((prefix) => path.startsWith(prefix))) {
      return fullPlan(packages, `shared test configuration changed: ${path}`);
    }
    // Embedded scenario sources are compiled / consumed by several Rust layers.
    if (path.startsWith("resources/scripts/")) {
      for (const name of ["aoidos-engine", "aoidos-script", "aoidos-theme", "aoidos"]) {
        if (roots.has(name)) selected.add(name);
      }
      continue;
    }

    // These shared inputs are compiled into or included by the Tauri application.
    if (path.startsWith("locales/") || path === "scripts/rust/tauri-build.rs") {
      if (roots.has("aoidos")) selected.add("aoidos");
      continue;
    }

    // The theme crate has compile-time assertions over these frontend assets.
    if (path === "src-web/app.css" || path === "src-web/styles/generated/theme-defaults.css") {
      if (roots.has("aoidos-theme")) selected.add("aoidos-theme");
      continue;
    }

    if (
      path.endsWith(".md") ||
      NON_RUST_FILES.has(path) ||
      NON_RUST_PREFIXES.some((prefix) => path.startsWith(prefix))
    ) {
      continue;
    }

    const owner = packageByRoot.find(
      ([, packageRoot]) => path === packageRoot || path.startsWith(`${packageRoot}/`),
    );
    if (!owner) return fullPlan(packages, `unmapped path: ${path}`);
    selected.add(owner[0]);
  }

  if (selected.size === 0) return { full: false, packages: [], reason: "no Rust package affected" };

  // Test direct consumers too: an API change can break any transitive workspace dependent.
  const dependents = new Map<string, Set<string>>();
  for (const pkg of packages) {
    for (const dependency of pkg.dependencies) {
      if (!dependency.path) continue;
      const dependencyPath = toRepoPath(dependency.path, root);
      const target = packageByRoot.find(([, packageRoot]) => dependencyPath === packageRoot)?.[0];
      if (!target) continue;
      const consumers = dependents.get(target) ?? new Set<string>();
      consumers.add(pkg.name);
      dependents.set(target, consumers);
    }
  }

  const queue = [...selected];
  while (queue.length > 0) {
    const dependency = queue.pop();
    if (!dependency) continue;
    for (const consumer of dependents.get(dependency) ?? []) {
      if (selected.has(consumer)) continue;
      selected.add(consumer);
      queue.push(consumer);
    }
  }

  return {
    full: false,
    packages: [...selected].sort(),
    reason: "affected packages and workspace dependents",
  };
}
