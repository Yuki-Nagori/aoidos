import { dirname, relative, sep } from "node:path";
import type { CargoPackage, RustTestPlan } from "./rust-test-selector";
import { selectRustTestPlan } from "./rust-test-selector";

export type CiPlan = {
  full: boolean;
  reason: string;
  rustTestPackages: string[];
  rustLintPackages: string[];
  rustFormat: boolean;
  vitestFiles: string[] | "all";
  biomeFormatFiles: string[] | "all";
  dprintFormatFiles: string[] | "all";
  biomeLintFiles: string[] | "all";
  runI18nCheck: boolean;
  runLayerCheck: boolean;
  runNativeTests: boolean;
};

const SHARED_CONFIG = new Set([
  "Cargo.toml",
  "Cargo.lock",
  "package.json",
  "bun.lock",
  "biome.json",
  "dprint.json",
  "vitest.config.ts",
  "vite.config.ts",
  "tsconfig.json",
  "rust-toolchain.toml",
]);
// These inputs affect tool execution or compiled resources across otherwise unrelated crates.
const SHARED_PREFIXES = [".cargo/"];
const BIOME_EXTENSIONS = new Set([
  ".css",
  ".cjs",
  ".cts",
  ".js",
  ".json",
  ".jsonc",
  ".mjs",
  ".mts",
  ".ts",
  ".tsx",
]);
const DPRINT_EXTENSIONS = new Set([".html", ".md", ".vue", ".yaml", ".yml"]);
const BIOME_IGNORED_PREFIXES = [
  "src-tauri/gen/",
  "tests/rust/native-platform/gen/",
  "resources/scripts/mistbell/",
];
const DPRINT_IGNORED_PREFIXES = [
  "src-tauri/gen/",
  "tests/rust/native-platform/gen/",
  "resources/scripts/mistbell/",
];
const BIOME_IGNORED_FILES = new Set(["public/theme-bootstrap.js", "public/locale-bootstrap.js"]);

function repoPath(path: string, root: string): string {
  return relative(root, path).split(sep).join("/");
}

function packageRoots(packages: CargoPackage[], root: string): Map<string, string> {
  return new Map(packages.map((pkg) => [pkg.name, repoPath(dirname(pkg.manifest_path), root)]));
}

function isRustPath(path: string): boolean {
  return path.endsWith(".rs") || path.endsWith("Cargo.toml") || path === "build.rs";
}

function isRustRelevantPath(path: string): boolean {
  return (
    path.startsWith("src-rust/") ||
    path.startsWith("src-tauri/") ||
    path.startsWith("tests/rust/") ||
    path.startsWith("resources/scripts/") ||
    path.startsWith("locales/") ||
    path === "scripts/rust/tauri-build.rs" ||
    path === "src-web/app.css" ||
    path === "src-web/styles/generated/theme-defaults.css"
  );
}

function vitestCandidate(path: string, knownTests: Set<string>): string | "all" | null {
  if (path.startsWith("tests/web/") || path.startsWith("tests/scripts/")) {
    return path.endsWith(".test.ts") && knownTests.has(path) ? path : "all";
  }
  if (path.startsWith("src-web/")) {
    if (path === "src-web/app.css") return "tests/web/appcss.test.ts";
    const relativePath = path.slice("src-web/".length).replace(/\.(?:vue|ts)$/, ".test.ts");
    const candidate = `tests/web/${relativePath}`;
    return knownTests.has(candidate) ? candidate : "all";
  }
  if (path.startsWith("scripts/")) {
    const candidate = `tests/scripts/${path.slice("scripts/".length).replace(/\.(?:mts|ts)$/, ".test.ts")}`;
    return knownTests.has(candidate) ? candidate : "all";
  }
  if (path.startsWith("locales/")) return "all";
  return null;
}

function fullPlan(reason: string, packages: CargoPackage[]): CiPlan {
  const allPackages = packages.map(({ name }) => name).sort();
  return {
    full: true,
    reason,
    rustTestPackages: allPackages,
    rustLintPackages: allPackages,
    rustFormat: true,
    vitestFiles: "all",
    biomeFormatFiles: "all",
    dprintFormatFiles: "all",
    biomeLintFiles: "all",
    runI18nCheck: true,
    runLayerCheck: true,
    runNativeTests: true,
  };
}

export function conservativeFullPlan(reason: string): CiPlan {
  return fullPlan(reason, []);
}

export function createCiPlan(
  changedPaths: string[],
  packages: CargoPackage[],
  root: string,
  knownTests: string[],
  existingPaths: string[] = changedPaths,
): CiPlan {
  if (changedPaths.length === 0) return fullPlan("empty or unavailable diff", packages);
  const normalized = changedPaths.map((path) => path.replaceAll("\\", "/").replace(/^\.\//, ""));
  for (const path of normalized) {
    if (!path || path.startsWith("/") || path.split("/").includes("..")) {
      return fullPlan(`invalid path: ${path}`, packages);
    }
    if (SHARED_CONFIG.has(path) || SHARED_PREFIXES.some((prefix) => path.startsWith(prefix))) {
      return fullPlan(`shared CI configuration changed: ${path}`, packages);
    }
  }

  const rustPaths = normalized.filter(isRustRelevantPath);
  const rustPlan: RustTestPlan = rustPaths.length
    ? selectRustTestPlan(rustPaths, packages, root)
    : { full: false, packages: [], reason: "no Rust package affected" };
  if (rustPlan.full) return fullPlan(rustPlan.reason, packages);

  const knownTestSet = new Set(knownTests);
  const existingPathSet = new Set(
    existingPaths.map((path) => path.replaceAll("\\", "/").replace(/^\.\//, "")),
  );
  const vitest = new Set<string>();
  let allVitest = false;
  let allBiome = false;
  let allDprint = false;
  let runI18nCheck = false;
  let runLayerCheck = false;
  let runNativeTests = false;
  const roots = packageRoots(packages, root);
  const biomePaths = new Set<string>();
  const dprintPaths = new Set<string>();

  for (const path of normalized) {
    if (
      path.startsWith("src-tauri/") ||
      path.startsWith("tests/rust/native-platform/") ||
      path.startsWith("src-web/api/") ||
      path.startsWith("src-web/utils/") ||
      [
        "src-web/app.css",
        "src-web/composables/useEnginePhase.ts",
        "src-web/composables/useGameView.ts",
        "src-web/composables/useLlmTurn.ts",
        "src-web/composables/useMigration.ts",
        "src-web/composables/useRecordView.ts",
        "src-web/locale-bootstrap.ts",
        "src-web/main.ts",
        "src-web/styles/generated/theme-defaults.css",
        "src-web/theme-bootstrap.ts",
      ].includes(path) ||
      path.startsWith("public/") ||
      path === "src-web/app.css" ||
      path === "src-web/styles/generated/theme-defaults.css" ||
      path === "src-web/theme-bootstrap.ts" ||
      path === "src-web/locale-bootstrap.ts" ||
      path === "scripts/build-native-fixture.mts" ||
      path === "scripts/rust/tauri-build.rs"
    ) {
      runNativeTests = true;
    }
    if (path.endsWith(".rs") || path.endsWith("Cargo.toml")) runLayerCheck = true;
    if (
      path === "locales/zh-Hans.json" ||
      path === "locales/en.json" ||
      path.startsWith("src-web/i18n/") ||
      path === "src-web/locale-bootstrap.ts"
    ) {
      runI18nCheck = true;
    }
    if (path.startsWith("src-web/") && /\.(?:ts|vue)$/.test(path)) runI18nCheck = true;

    const candidate = vitestCandidate(path, knownTestSet);
    if (candidate === "all") allVitest = true;
    else if (candidate) vitest.add(candidate);

    const biomeMatch = BIOME_EXTENSIONS.has(path.slice(path.lastIndexOf(".")));
    const dprintMatch = DPRINT_EXTENSIONS.has(path.slice(path.lastIndexOf(".")));
    // Keep deleted paths in impact analysis, but never pass nonexistent or ignored paths to tools.
    if (
      existingPathSet.has(path) &&
      biomeMatch &&
      !BIOME_IGNORED_FILES.has(path) &&
      !BIOME_IGNORED_PREFIXES.some((prefix) => path.startsWith(prefix))
    ) {
      biomePaths.add(path);
    }
    if (
      existingPathSet.has(path) &&
      dprintMatch &&
      !DPRINT_IGNORED_PREFIXES.some((prefix) => path.startsWith(prefix))
    ) {
      dprintPaths.add(path);
    }

    const isKnownArea =
      path.startsWith("src-rust/") ||
      path.startsWith("src-tauri/") ||
      path.startsWith("tests/rust/") ||
      path.startsWith("tests/web/") ||
      path.startsWith("tests/scripts/") ||
      path.startsWith("src-web/") ||
      path.startsWith("scripts/") ||
      path.startsWith("locales/") ||
      path.startsWith("resources/scripts/") ||
      path.startsWith(".github/") ||
      path.startsWith("ai-docs/") ||
      path.startsWith("public/") ||
      path === "README.md" ||
      path === "AGENTS.md";
    if (!isKnownArea) return fullPlan(`unmapped path: ${path}`, packages);

    // Unknown source extensions may affect a checker's input; widen that checker safely.
    if (path.startsWith("src-web/") && !biomeMatch && !dprintMatch && !path.endsWith(".css")) {
      allBiome = true;
      allDprint = true;
    }
  }

  const rustTestPackages = rustPlan.packages;
  const rustLintPackages = rustPlan.packages;
  runNativeTests ||= rustPlan.packages.includes("aoidos-native-tests");
  const rustFormat = normalized.some(isRustPath);
  if (normalized.some((path) => path === "src-web/app.css")) {
    if (roots.has("aoidos-theme") && !rustTestPackages.includes("aoidos-theme")) {
      rustTestPackages.push("aoidos-theme");
      rustLintPackages.push("aoidos-theme");
    }
  }

  return {
    full: false,
    reason: "affected source and consumer scope",
    rustTestPackages: [...new Set(rustTestPackages)].sort(),
    rustLintPackages: [...new Set(rustLintPackages)].sort(),
    rustFormat,
    vitestFiles: allVitest ? "all" : [...vitest].sort(),
    biomeFormatFiles: allBiome ? "all" : [...biomePaths].sort(),
    dprintFormatFiles: allDprint ? "all" : [...dprintPaths].sort(),
    biomeLintFiles: allBiome ? "all" : [...biomePaths].sort(),
    runI18nCheck,
    runLayerCheck,
    runNativeTests,
  };
}
