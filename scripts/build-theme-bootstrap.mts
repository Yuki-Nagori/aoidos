import { exit } from "node:process";

const result = await Bun.build({
  entrypoints: ["src-web/theme-bootstrap.ts"],
  outdir: "public",
  naming: "theme-bootstrap.js",
  target: "browser",
  format: "iife",
});

if (!result.success) {
  for (const diagnostic of result.logs) console.error(diagnostic);
  exit(1);
}
