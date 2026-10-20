import { readFile, readdir } from "node:fs/promises";
import { exit } from "node:process";
import { NodeTypes, parse as parseTemplate } from "@vue/compiler-dom";
import { parse as parseSfc } from "@vue/compiler-sfc";
import * as ts from "typescript";

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
const usedKeys = new Set<string>();
const reservedNamespaces = ["native.", "budget.", "memory."];
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

function staticTranslationKeys(source: string, path: string): string[] {
  const file = ts.createSourceFile(path, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const keys: string[] = [];
  const conditionalKeys = (node: ts.Node): string[] => {
    if (ts.isStringLiteralLike(node)) return [node.text];
    if (ts.isConditionalExpression(node)) {
      return [...conditionalKeys(node.whenTrue), ...conditionalKeys(node.whenFalse)];
    }
    if (
      ts.isParenthesizedExpression(node) ||
      ts.isAsExpression(node) ||
      ts.isTypeAssertionExpression(node) ||
      ts.isSatisfiesExpression(node)
    ) {
      return conditionalKeys(node.expression);
    }
    return [];
  };
  const visit = (node: ts.Node): void => {
    if (
      ts.isCallExpression(node) &&
      ts.isIdentifier(node.expression) &&
      node.expression.text === "t"
    ) {
      const argument = node.arguments[0];
      if (argument && ts.isStringLiteralLike(argument)) keys.push(argument.text);
      else if (argument && ts.isConditionalExpression(argument)) {
        keys.push(...conditionalKeys(argument));
      }
    }
    if (
      ts.isPropertyAssignment(node) &&
      (ts.isIdentifier(node.name) || ts.isStringLiteral(node.name)) &&
      (node.name.text === "key" || node.name.text.endsWith("Key")) &&
      ts.isStringLiteralLike(node.initializer) &&
      en.has(node.initializer.text)
    ) {
      keys.push(node.initializer.text);
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  return keys;
}

function templateTranslationKeys(template: string, path: string): string[] {
  const root = parseTemplate(template, { comments: false });
  const keys: string[] = [];

  const visit = (node: unknown): void => {
    if (typeof node !== "object" || node === null) return;
    const current = node as {
      type?: number;
      content?: unknown;
      children?: unknown[];
      props?: unknown[];
      exp?: { content: string };
      arg?: { content: string };
      condition?: { content: string };
      source?: { content: string };
      branches?: unknown[];
    };

    if (current.type === NodeTypes.SIMPLE_EXPRESSION) {
      keys.push(...staticTranslationKeys((current.content as string) ?? "", path));
      return;
    }
    if (current.type === NodeTypes.DIRECTIVE) {
      visit(current.exp);
      visit(current.arg);
      return;
    }
    if (current.type === NodeTypes.IF_BRANCH) {
      visit(current.condition);
      current.children?.forEach(visit);
      return;
    }
    if (current.type === NodeTypes.FOR) {
      visit(current.source);
      current.children?.forEach(visit);
      return;
    }
    if (current.type === NodeTypes.ELEMENT) current.props?.forEach(visit);
    if (current.type === NodeTypes.IF) current.branches?.forEach(visit);
    if (current.type === NodeTypes.ROOT || current.type === NodeTypes.ELEMENT) {
      current.children?.forEach(visit);
    }
    if (current.type === NodeTypes.INTERPOLATION || current.type === NodeTypes.TEXT_CALL) {
      visit(current.content);
    }
    if (current.type === NodeTypes.COMPOUND_EXPRESSION) current.children?.forEach(visit);
  };

  root.children.forEach(visit);
  return keys;
}

function recordTranslationKeys(keys: string[], path: string): void {
  for (const key of keys) {
    if (!en.has(key)) problems.push(`${path}: missing locale key ${key}`);
    else usedKeys.add(key);
    if (reservedNamespaces.some((namespace) => key.startsWith(namespace))) {
      problems.push(`${path}: ${key} is reserved for a later implementation task`);
    }
  }
}

for (const path of sourceFiles) {
  const source = await readFile(path, "utf8");
  if (path.endsWith(".ts")) {
    recordTranslationKeys(staticTranslationKeys(source, path), path);
    continue;
  }

  const { descriptor, errors } = parseSfc(source, { filename: path });
  for (const error of errors) problems.push(`${path}: invalid Vue component (${error.message})`);
  for (const block of [descriptor.script, descriptor.scriptSetup]) {
    if (block) recordTranslationKeys(staticTranslationKeys(block.content, path), path);
  }
  if (descriptor.template?.ast) {
    recordTranslationKeys(templateTranslationKeys(descriptor.template.content, path), path);
  }
}

for (const key of en.keys()) {
  if (!reservedNamespaces.some((namespace) => key.startsWith(namespace)) && !usedKeys.has(key)) {
    problems.push(`${key}: unused locale key`);
  }
}

if (problems.length) {
  console.error(problems.map((problem) => `i18n: ${problem}`).join("\n"));
  exit(1);
}
console.log(`i18n: ${en.size} messages and placeholders agree across ${locales.join(", ")}`);
