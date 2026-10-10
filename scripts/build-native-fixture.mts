// 原生集成装配真实前端消费模块；产物只放测试 crate 的忽略目录。
import { exit } from "node:process";
import { cp, copyFile, mkdir, readdir, readFile } from "node:fs/promises";

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
await Bun.write(
  `${output}/product.html`,
  Bun.file("tests/rust/native-platform/webview/product.html"),
);

// Reuse the shipped CSS and icon so native checks exercise the actual product assets.
const distribution = "dist";
const html = await readFile(`${distribution}/index.html`, "utf8");
const cssPath = html.match(/href="(\/assets\/[^" ]+\.css)"/)?.[1];
if (!cssPath) throw new Error("Production HTML must link one built stylesheet");
const moduleScript = /<script type="module" crossorigin src="\/assets\/[^" ]+\.js"><\/script>/;
if (!moduleScript.test(html)) throw new Error("Production HTML must link one module entry");
const fixtureHtml = html
  .replace(moduleScript, '<script type="module" src="/main.js"></script>')
  .replace(cssPath, "/assets/product.css");
if (
  fixtureHtml.indexOf('src="/theme-bootstrap.js"') < 0 ||
  fixtureHtml.indexOf('src="/theme-bootstrap.js"') >
    fixtureHtml.indexOf('href="/assets/product.css"')
) {
  throw new Error("Product theme bootstrap must run before the stylesheet");
}
await Bun.write(`${output}/index.html`, fixtureHtml);
const cssName = cssPath.split("/").at(-1);
if (!cssName) throw new Error("Production stylesheet path is invalid");
const assets = `${output}/assets`;
await mkdir(assets, { recursive: true });
await copyFile(`${distribution}/assets/${cssName}`, `${assets}/product.css`);
await copyFile("public/icon.png", `${output}/icon.png`);
const builtStylesheets = (await readdir(`${distribution}/assets`)).filter((name) =>
  name.endsWith(".css"),
);
if (builtStylesheets.length !== 1 || builtStylesheets[0] !== cssName) {
  throw new Error("Production HTML and built stylesheet assets do not match");
}
await cp("tests/rust/native-platform/webview/hmr", "tests/rust/native-platform/gen/hmr", {
  recursive: true,
});
