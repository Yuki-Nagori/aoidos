// 原生集成装配真实前端消费模块；产物只放测试 crate 的忽略目录。
import { exit } from "node:process";
import { mkdir } from "node:fs/promises";

const output = "tests/rust/native-platform/gen/webview";
await mkdir(output, { recursive: true });
const result = await Bun.build({
  entrypoints: [
    "tests/rust/native-platform/webview/main.ts",
    "tests/rust/native-platform/webview/product.ts",
  ],
  outdir: output,
  target: "browser",
  format: "esm",
});
if (!result.success) {
  for (const diagnostic of result.logs) console.error(diagnostic);
  exit(1);
}
const bootstrap = await Bun.build({
  entrypoints: ["src-web/theme-bootstrap.ts"],
  outdir: output,
  naming: "theme-bootstrap.js",
  target: "browser",
  format: "iife",
});
if (!bootstrap.success) {
  for (const diagnostic of bootstrap.logs) console.error(diagnostic);
  exit(1);
}
await Bun.write(`${output}/index.html`, Bun.file("tests/rust/native-platform/webview/index.html"));
await Bun.write(
  `${output}/product.html`,
  Bun.file("tests/rust/native-platform/webview/product.html"),
);
