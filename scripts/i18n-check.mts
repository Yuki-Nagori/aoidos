import { readFile, readdir } from "node:fs/promises";
import { exit } from "node:process";

type MessageTree = { [key: string]: string | MessageTree };
const locales = ["zh-Hans", "en"] as const;
const resources = new Map<string, MessageTree>();

for (const locale of locales) {
  resources.set(
    locale,
    JSON.parse(await readFile(`locales/${locale}.json`, "utf8")) as MessageTree,
  );
}

function flatten(tree: MessageTree, prefix = ""): Map<string, string> {
  const values = new Map<string, string>();
  for (const [key, value] of Object.entries(tree)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "string") values.set(path, value);
    else for (const [child, message] of flatten(value, path)) values.set(child, message);
  }
  return values;
}

function placeholders(message: string): string[] {
  return [...message.matchAll(/\{([\w]+)\}/g)].map((match) => match[1]!).sort();
}

const sourceFiles: string[] = [];
async function collect(directory: string): Promise<void> {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = `${directory}/${entry.name}`;
    if (entry.isDirectory()) await collect(path);
    else if (entry.name.endsWith(".ts") || entry.name.endsWith(".vue")) sourceFiles.push(path);
  }
}
await collect("src-web");

const problems: string[] = [];
const [zh, en] = locales.map((locale) => flatten(resources.get(locale)!));
if (zh!.size !== en!.size || [...zh!.keys()].some((key) => !en!.has(key))) {
  problems.push("zh-Hans and en must contain the same message keys");
}
for (const [key, message] of zh!) {
  if (JSON.stringify(placeholders(message)) !== JSON.stringify(placeholders(en!.get(key) ?? ""))) {
    problems.push(`${key}: placeholder names differ between locales`);
  }
}

for (const namespace of ["native.", "budget.", "memory."]) {
  const unusedByUi = [...zh!.keys()].filter((key) => key.startsWith(namespace));
  for (const key of unusedByUi) {
    if (!en!.has(key)) problems.push(`${key}: reserved namespace is missing from en`);
  }
}

for (const path of sourceFiles) {
  const source = await readFile(path, "utf8");
  for (const [, key] of source.matchAll(/\bt\(\s*["'`]([\w.]+)["'`]/g)) {
    if (!zh!.has(key!)) problems.push(`${path}: missing locale key ${key}`);
    if (key!.startsWith("budget.") || key!.startsWith("memory.")) {
      problems.push(`${path}: ${key} is reserved for a later implementation task`);
    }
  }
}

if (problems.length) {
  console.error(problems.map((problem) => `i18n: ${problem}`).join("\n"));
  exit(1);
}
console.log(`i18n: ${zh!.size} messages and placeholders agree across ${locales.join(", ")}`);
