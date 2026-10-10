import { readFile, readdir } from "node:fs/promises";
import { exit } from "node:process";

type MessageTree = { [key: string]: string | MessageTree };
const locales = ["en", "zh-Hans"] as const;
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
const en = flatten(resources.get("en")!);
const zhHans = flatten(resources.get("zh-Hans")!);
if (en.size !== zhHans.size || [...en.keys()].some((key) => !zhHans.has(key))) {
  problems.push("en and zh-Hans must contain the same message keys");
}
for (const [key, message] of en!) {
  if (
    JSON.stringify(placeholders(message)) !== JSON.stringify(placeholders(zhHans.get(key) ?? ""))
  ) {
    problems.push(`${key}: placeholder names differ between locales`);
  }
}

for (const namespace of ["native.", "budget.", "memory."]) {
  const unusedByUi = [...en.keys()].filter((key) => key.startsWith(namespace));
  for (const key of unusedByUi) {
    if (!zhHans.has(key)) problems.push(`${key}: reserved namespace is missing from zh-Hans`);
  }
}

for (const path of sourceFiles) {
  const source = await readFile(path, "utf8");
  for (const [, key] of source.matchAll(/\bt\(\s*["'`]([\w.]+)["'`]/g)) {
    if (!en.has(key!)) problems.push(`${path}: missing locale key ${key}`);
    if (key!.startsWith("budget.") || key!.startsWith("memory.")) {
      problems.push(`${path}: ${key} is reserved for a later implementation task`);
    }
  }
}

if (problems.length) {
  console.error(problems.map((problem) => `i18n: ${problem}`).join("\n"));
  exit(1);
}
console.log(`i18n: ${en.size} messages and placeholders agree across ${locales.join(", ")}`);
