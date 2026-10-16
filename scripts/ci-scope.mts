import { appendFileSync, existsSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { execFileSync, spawnSync } from "node:child_process";
import type { CargoPackage } from "./rust-test-selector";
import type { CiPlan } from "./ci-selector";
import { conservativeFullPlan, createCiPlan } from "./ci-selector";

function git(args: string[]): Buffer {
  return execFileSync("git", args, { encoding: "buffer", maxBuffer: 8 * 1024 * 1024 });
}

function nulPaths(output: Buffer): string[] {
  return output.toString("utf8").split("\0").filter(Boolean);
}

function changedPaths(): string[] | null {
  const base = process.env.AOIDOS_CI_BASE_SHA;
  const head = process.env.AOIDOS_CI_HEAD_SHA;
  if (base || head) {
    if (!base || !head || /^0+$/.test(base)) return null;
    try {
      return nulPaths(git(["diff", "--no-renames", "--name-only", "-z", base, head]));
    } catch {
      return null;
    }
  }

  try {
    const diff = nulPaths(git(["diff", "--no-renames", "--name-only", "-z", "origin/main...HEAD"]));
    const staged = nulPaths(git(["diff", "--no-renames", "--cached", "--name-only", "-z"]));
    const unstaged = nulPaths(git(["diff", "--no-renames", "--name-only", "-z"]));
    const untracked = nulPaths(git(["ls-files", "--others", "--exclude-standard", "-z"]));
    return [...new Set([...diff, ...staged, ...unstaged, ...untracked])];
  } catch {
    return null;
  }
}

function listFiles(directory: string, prefix = ""): string[] {
  return readdirSync(join(directory, prefix), { withFileTypes: true }).flatMap((entry) => {
    const path = join(prefix, entry.name).split("\\").join("/");
    return entry.isDirectory()
      ? listFiles(directory, path)
      : entry.isFile()
        ? [`${directory}/${path}`]
        : [];
  });
}

function getPlan(): { root: string; plan: CiPlan } {
  const root = resolve(
    execFileSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8" }).trim(),
  );
  const paths = changedPaths();
  let plan: CiPlan;
  if (process.env.AOIDOS_CI_PLAN) {
    plan = JSON.parse(process.env.AOIDOS_CI_PLAN) as CiPlan;
  } else {
    try {
      const metadata = JSON.parse(
        execFileSync("cargo", ["metadata", "--locked", "--no-deps", "--format-version", "1"], {
          cwd: root,
          encoding: "utf8",
          maxBuffer: 16 * 1024 * 1024,
        }),
      ) as { packages: CargoPackage[] };
      const tests = [...listFiles(join(root, "tests"))]
        .filter((path) => path.endsWith(".test.ts"))
        .map((path) => path.slice(root.length + 1));
      plan = paths
        ? createCiPlan(
            paths,
            metadata.packages,
            root,
            tests,
            paths.filter((path) => existsSync(join(root, path))),
          )
        : conservativeFullPlan("could not determine a trustworthy change set");
    } catch (error) {
      console.error("Could not load Cargo metadata; using the full CI plan.");
      if (error instanceof Error) console.error(error.message);
      plan = conservativeFullPlan("Cargo metadata unavailable");
    }
  }
  return { root, plan };
}

function run(command: string, args: string[], cwd: string): void {
  const result = spawnSync(command, args, { stdio: "inherit", cwd });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

function runBun(root: string, script: string, args: string[] = []): void {
  run("bun", ["run", script, ...args], root);
}

function runCargo(root: string, args: string[]): void {
  run("cargo", args, root);
}

function runTest(root: string, plan: CiPlan): void {
  runBun(root, "theme:defaults:check");
  if (plan.full) runCargo(root, ["test", "--locked", "--workspace"]);
  else if (plan.rustTestPackages.length > 0) {
    runCargo(root, [
      "test",
      "--locked",
      ...plan.rustTestPackages.flatMap((name) => ["--package", name]),
    ]);
  }
  if (process.argv.includes("--rust-only")) return;
  if (plan.vitestFiles === "all") run("bunx", ["vitest", "run"], root);
  else if (plan.vitestFiles.length > 0) run("bunx", ["vitest", "run", ...plan.vitestFiles], root);
}

function runFormat(root: string, plan: CiPlan): void {
  if (plan.full || plan.rustFormat) runBun(root, "format:rust:check");
  if (plan.biomeFormatFiles === "all") runBun(root, "format:biome:check");
  else if (plan.biomeFormatFiles.length > 0) {
    run("bunx", ["biome", "format", ...plan.biomeFormatFiles], root);
  }
  if (plan.dprintFormatFiles === "all") runBun(root, "format:dprint:check");
  else if (plan.dprintFormatFiles.length > 0) {
    run("bunx", ["dprint", "check", ...plan.dprintFormatFiles], root);
  }
}

function runLint(root: string, plan: CiPlan): void {
  runWebLint(root, plan);
  runLayers(root, plan);
  runClippy(root, plan);
}

function runWebLint(root: string, plan: CiPlan): void {
  if (plan.biomeLintFiles === "all")
    run("bunx", ["biome", "lint", ".", "--error-on-warnings"], root);
  else if (plan.biomeLintFiles.length > 0) {
    run("bunx", ["biome", "lint", ...plan.biomeLintFiles, "--error-on-warnings"], root);
  }
  if (plan.runI18nCheck) runBun(root, "i18n:check");
}

function runLayers(root: string, plan: CiPlan): void {
  if (plan.runLayerCheck) runBun(root, "lint:layers");
}

function runClippy(root: string, plan: CiPlan): void {
  if (plan.full) {
    runBun(root, "lint:rust");
  } else if (plan.rustLintPackages.length > 0) {
    const packages = plan.rustLintPackages.flatMap((name) => ["--package", name]);
    const enableDesktopTests = plan.rustLintPackages.includes("aoidos-native-tests");
    if (enableDesktopTests) {
      runBun(root, "build:web:assets");
      runBun(root, "build:native-fixture");
    }
    const features = enableDesktopTests
      ? ["--features", "aoidos-native-tests/desktop-session"]
      : [];
    runCargo(root, [
      "clippy",
      "--locked",
      "--all-targets",
      ...packages,
      ...features,
      "--",
      "-D",
      "warnings",
    ]);
  }
}

function writeGithubOutput(plan: CiPlan): void {
  const output = process.env.GITHUB_OUTPUT;
  if (!output) throw new Error("GITHUB_OUTPUT is not set");
  const outputs: Record<string, string> = {
    plan: JSON.stringify(plan),
    full: String(plan.full),
    has_rust_test: String(plan.full || plan.rustTestPackages.length > 0),
    has_vitest: String(plan.full || plan.vitestFiles === "all" || plan.vitestFiles.length > 0),
    has_format: String(
      plan.full ||
        plan.rustFormat ||
        plan.biomeFormatFiles === "all" ||
        plan.biomeFormatFiles.length > 0 ||
        plan.dprintFormatFiles === "all" ||
        plan.dprintFormatFiles.length > 0,
    ),
    has_lint: String(
      plan.full ||
        plan.runI18nCheck ||
        plan.runLayerCheck ||
        plan.rustLintPackages.length > 0 ||
        plan.biomeLintFiles === "all" ||
        plan.biomeLintFiles.length > 0,
    ),
    has_layers: String(plan.full || plan.runLayerCheck),
    has_desktop_clippy: String(plan.full || plan.rustLintPackages.includes("aoidos-native-tests")),
    has_app_build: String(plan.full || plan.rustTestPackages.includes("aoidos")),
    has_native: String(plan.full || plan.runNativeTests),
  };
  appendFileSync(
    output,
    `${Object.entries(outputs)
      .map(([key, value]) => `${key}=${value}`)
      .join("\n")}\n`,
  );
}

function main(): void {
  const action = process.argv[2] ?? "plan";
  const { root, plan } = getPlan();
  console.log(`CI scope: ${plan.full ? "full" : plan.reason}`);
  console.log(
    `Rust tests: ${plan.full ? "workspace" : plan.rustTestPackages.join(", ") || "none"}`,
  );
  console.log(
    `Vitest: ${plan.vitestFiles === "all" ? "all" : plan.vitestFiles.join(", ") || "none"}`,
  );

  if (action === "plan") console.log(JSON.stringify(plan, null, 2));
  else if (action === "output") writeGithubOutput(plan);
  else if (action === "test") runTest(root, plan);
  else if (action === "format") runFormat(root, plan);
  else if (action === "lint") runLint(root, plan);
  else if (action === "web-lint") runWebLint(root, plan);
  else if (action === "layers") runLayers(root, plan);
  else if (action === "clippy") runClippy(root, plan);
  else throw new Error(`Unknown CI scope action: ${action}`);
}

if (import.meta.main) main();
