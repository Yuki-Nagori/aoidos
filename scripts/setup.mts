// 一次性环境配置（bun install 的 postinstall 也走这里）：按 rust-toolchain.toml 装工具链 + cargo-llvm-cov。
import { readRustToolchain, rustToolchainEnv } from "./rust-toolchain";

const run = (cmd: string[], env?: NodeJS.ProcessEnv) =>
  Bun.spawnSync(cmd, { env, stdout: "inherit", stderr: "inherit" }).success;
const ok = (cmd: string[], env?: NodeJS.ProcessEnv) =>
  Bun.spawnSync(cmd, { env, stdout: "ignore", stderr: "ignore" }).success;
const output = (cmd: string[]) => {
  const result = Bun.spawnSync(cmd, { stdout: "pipe", stderr: "ignore" });
  return result.success ? result.stdout.toString() : null;
};

// rustup 是硬前置，缺失则 bun install 连带失败，属预期。
if (!ok(["rustup", "--version"])) {
  console.error("未检测到 rustup：请先安装 https://rustup.rs，再重跑 bun install");
  process.exit(1);
}

// 工具链版本与组件列表统一从 rust-toolchain.toml 读取。
const { channel, profile, components } = readRustToolchain();

const installArgs = ["toolchain", "install", channel];
if (profile) installArgs.push("--profile", profile);
for (const component of components) installArgs.push("--component", component);

const cargo = ["rustup", "run", channel, "cargo"];
const installed = output(["rustup", "component", "list", "--toolchain", channel, "--installed"]);
const installedComponents = new Set(installed?.split(/\r?\n/).filter(Boolean) ?? []);
const missingComponents = components.filter((component) => {
  const normalized = component.replace(/-preview$/, "");
  return ![...installedComponents].some(
    (entry) => entry === normalized || entry.startsWith(`${normalized}-`),
  );
});
const cargoExists = ok(["rustup", "which", "--toolchain", channel, "cargo"]);
const toolchainEnv = () => rustToolchainEnv();
const cargoWorks = cargoExists && ok([...cargo, "--version"], toolchainEnv());

if (!cargoWorks || missingComponents.length > 0) {
  if (!cargoWorks) {
    console.log(`修复不完整的 Rust 工具链 ${channel} …`);
    Bun.spawnSync(["rustup", "toolchain", "uninstall", channel], {
      stdout: "ignore",
      stderr: "ignore",
    });
  } else {
    console.log(`安装 Rust 工具链组件 ${channel} …`);
  }
  if (!run(["rustup", ...installArgs])) {
    console.error("工具链安装失败：手动执行 rustup toolchain install 查看原因");
    process.exit(1);
  }
}
if (
  !ok(["rustup", "which", "--toolchain", channel, "cargo"]) ||
  !ok([...cargo, "--version"], toolchainEnv())
) {
  console.error(
    `工具链 ${channel} 仍不可用：请手动执行 rustup toolchain uninstall ${channel} 后重跑 bun install`,
  );
  process.exit(1);
}

// cargo-llvm-cov 是覆盖率门禁依赖，缺失则源码编译安装（一次性）。
if (
  !ok([...cargo, "llvm-cov", "--version"], toolchainEnv()) &&
  !run([...cargo, "install", "cargo-llvm-cov"], toolchainEnv())
) {
  console.error("cargo-llvm-cov 安装失败：可改用 taiki-e/install-action（见 CI 配置）");
  process.exit(1);
}

console.log("环境就绪，可以 bun run verify / tauri:dev 了。");
