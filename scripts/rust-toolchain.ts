import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { delimiter, dirname, resolve } from "node:path";

export type RustToolchain = {
  channel: string;
  profile?: string;
  components: string[];
};

export function readRustToolchain(root = resolve(import.meta.dir, "..")): RustToolchain {
  const source = readFileSync(resolve(root, "rust-toolchain.toml"), "utf8");
  const channel = source.match(/channel\s*=\s*"([^"]+)"/)?.[1];
  if (!channel) throw new Error("rust-toolchain.toml 缺少 channel");

  return {
    channel,
    profile: source.match(/profile\s*=\s*"([^"]+)"/)?.[1],
    components:
      (source.match(/components\s*=\s*\[([^\]]*)\]/)?.[1] ?? "")
        .match(/"([^"]+)"/g)
        ?.map((component) => component.slice(1, -1)) ?? [],
  };
}

function toolchainInvocation(tool: string, args: string[], root?: string): string[] {
  return ["run", readRustToolchain(root).channel, tool, ...args];
}

export function cargoInvocation(args: string[], root?: string): string[] {
  return toolchainInvocation("cargo", args, root);
}

export function rustToolchainEnv(root = resolve(import.meta.dir, "..")): NodeJS.ProcessEnv {
  const channel = readRustToolchain(root).channel;
  const cargo = spawnSync("rustup", ["which", "--toolchain", channel, "cargo"], {
    cwd: root,
    encoding: "utf8",
  });
  if (cargo.error) throw cargo.error;
  if (cargo.status !== 0) {
    throw new Error(`无法定位 Rust 工具链 ${channel} 的 cargo`);
  }

  const env = { ...process.env };
  const pathKey = Object.keys(env).find((key) => key.toLowerCase() === "path") ?? "PATH";
  env[pathKey] = [dirname(cargo.stdout.trim()), env[pathKey]].filter(Boolean).join(delimiter);
  return env;
}

export function runToolchainProgram(
  program: string,
  args: string[],
  root = resolve(import.meta.dir, ".."),
): void {
  const result = spawnSync("rustup", ["run", readRustToolchain(root).channel, program, ...args], {
    cwd: root,
    env: rustToolchainEnv(root),
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

export function runCargo(args: string[], root = resolve(import.meta.dir, "..")): void {
  runToolchainProgram("cargo", args, root);
}
