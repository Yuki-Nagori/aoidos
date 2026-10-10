import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { format, resolveConfig } from "prettier";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const themesPath = resolve(root, "src-rust/aoidos-theme/assets/themes");
const outputPath = resolve(root, "src-web/styles/generated/theme-defaults.css");
const expectedTokens = new Set([
  "--ink",
  "--muted",
  "--accent",
  "--on-accent",
  "--accent-violet",
  "--accent-warm",
  "--panel",
  "--bubble-user",
  "--bubble-persona",
  "--hairline",
  "--app-bg",
  "--led-active",
  "--diff-add",
  "--diff-remove",
  "--warning",
  "--danger",
  "--record-left",
  "--ease-signature",
  "--dur-micro",
]);

function declarations(source: string): Map<string, string> {
  const result = new Map<string, string>();
  let start = 0;
  let depth = 0;
  for (let index = 0; index <= source.length; index += 1) {
    const char = source[index];
    if (char === "(") depth += 1;
    else if (char === ")") depth -= 1;
    else if ((char === ";" || index === source.length) && depth === 0) {
      const declaration = source.slice(start, index).trim();
      start = index + 1;
      if (!declaration) continue;
      const separator = declaration.indexOf(":");
      if (separator < 0) throw new Error("malformed theme declaration");
      const name = declaration.slice(0, separator).trim();
      const value = declaration.slice(separator + 1).trim();
      if (!value || result.has(name)) throw new Error(`empty or duplicate theme value: ${name}`);
      result.set(name, value);
    }
  }
  if (depth !== 0) throw new Error("malformed theme value");
  return result;
}

const files = (await readdir(themesPath)).filter((file) => file.endsWith(".css")).sort();
if (files.length === 0) throw new Error("no theme CSS files found");
const ids = new Set<string>();
const output: string[] = [];

for (const file of files) {
  const id = file.slice(0, -4);
  if (!/^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/.test(id) || ids.has(id))
    throw new Error(`invalid or duplicate theme id: ${id}`);
  ids.add(id);
  const source = (await readFile(resolve(themesPath, file), "utf8")).replace(
    /^\/\*[\s\S]*?\*\/\s*/,
    "",
  );
  const match = /^:root\s*\{([\s\S]*)\}\s*$/.exec(source);
  if (!match?.[1]) throw new Error(`theme ${id} must contain exactly one :root block`);
  const values = declarations(match[1]);
  const scheme = values.get("color-scheme");
  values.delete("color-scheme");
  if (scheme !== "light" && scheme !== "dark") throw new Error(`theme ${id} needs color-scheme`);
  if (
    values.size !== expectedTokens.size ||
    [...expectedTokens].some((token) => !values.has(token))
  )
    throw new Error(`theme ${id} must define every semantic token exactly once`);
  const selector = id === "dark" ? ':root,\n:root[data-theme="dark"]' : `:root[data-theme="${id}"]`;
  output.push(
    `${selector} {\n  color-scheme: ${scheme};\n${[...values]
      .map(([name, value]) => `  ${name}: ${value};`)
      .join("\n")}\n}`,
  );
}

const prettierOptions = (await resolveConfig(outputPath)) ?? {};
const generated = await format(`${output.join("\n\n")}\n`, {
  ...prettierOptions,
  parser: "css",
});
if (process.argv.includes("--write")) {
  await mkdir(resolve(root, "src-web/styles/generated"), { recursive: true });
  await writeFile(outputPath, generated);
} else {
  const current = await readFile(outputPath, "utf8").catch(() => "");
  if (current !== generated)
    throw new Error("generated theme CSS is stale; run bun scripts/theme-defaults.mts --write");
}
