import { resolve } from "node:path";
import { runToolchainProgram } from "./rust-toolchain";

const [tool, ...args] = Bun.argv.slice(2);
if (tool !== "tauri") {
  console.error("用法：bun scripts/toolchain.mts tauri <参数…>");
  process.exit(2);
}

runToolchainProgram(process.execPath, [
  resolve(import.meta.dir, "../node_modules/@tauri-apps/cli/tauri.js"),
  ...args,
]);
