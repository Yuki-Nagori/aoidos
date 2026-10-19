import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";

/**
 * 设计目录（theming.md 首版 token 目录）与 app.css 的互校，防三处漂移：
 * 语义真源主题完整性、@theme inline 映射一一对应、直接消费项不进映射。
 * 另钉 Tailwind 原语层（white / black 静态发射）；Rust 目录测试独立验证 CSS
 * 文件发现和 token 完整性。
 */

interface CatalogEntry {
  /** 语义名称，如 --ink */
  name: string;
  /** 进入 @theme inline 映射（如 --color-ink）；空串 = 组件直接消费不进映射 */
  mapping: string;
}

// theming.md「首版 token 目录」：C01–C15 / C19 / S01 / M01 / M02。
// C 域与 --app-bg 随主题切换；S / M 域在内置主题之间保持一致。
const CATALOG: CatalogEntry[] = [
  { name: "--ink", mapping: "--color-ink" },
  { name: "--muted", mapping: "--color-muted" },
  { name: "--accent", mapping: "--color-accent" },
  { name: "--on-accent", mapping: "--color-on-accent" },
  { name: "--accent-violet", mapping: "--color-accent-violet" },
  { name: "--accent-warm", mapping: "--color-accent-warm" },
  { name: "--panel", mapping: "--color-panel" },
  { name: "--bubble-user", mapping: "--color-bubble-user" },
  { name: "--bubble-persona", mapping: "--color-bubble-persona" },
  { name: "--hairline", mapping: "--color-hairline" },
  { name: "--app-bg", mapping: "" },
  { name: "--led-active", mapping: "--color-led-active" },
  { name: "--diff-add", mapping: "--color-diff-add" },
  { name: "--diff-remove", mapping: "--color-diff-remove" },
  { name: "--warning", mapping: "--color-warning" },
  { name: "--danger", mapping: "--color-danger" },
  { name: "--record-left", mapping: "--spacing-record-left" },
  { name: "--ease-signature", mapping: "" },
  { name: "--dur-micro", mapping: "" },
];

const css = readFileSync(resolve(__dirname, "../../src-web/app.css"), "utf-8");
const themeCss = readFileSync(
  resolve(__dirname, "../../src-web/styles/generated/theme-defaults.css"),
  "utf-8",
);

/** 花括号配平截取选择器规则块正文；找不到时报错而不是静默返回空。 */
function blockOf(selector: string): string {
  const start = themeCss.indexOf(selector);
  if (start < 0) throw new Error(`选择器缺失：${selector}`);
  const open = themeCss.indexOf("{", start);
  let depth = 0;
  for (let i = open; i < themeCss.length; i += 1) {
    if (themeCss[i] === "{") depth += 1;
    else if (themeCss[i] === "}") {
      depth -= 1;
      if (depth === 0) return themeCss.slice(open + 1, i);
    }
  }
  throw new Error(`规则块未闭合：${selector}`);
}

/** 块内的 custom properties → 值映射（同名后者覆盖前者，正常情况不重名）。 */
function varsOf(block: string): Map<string, string> {
  const vars = new Map<string, string>();
  for (const match of block.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
    const [, name, value] = match;
    if (name !== undefined && value !== undefined) {
      vars.set(name, value.trim());
    }
  }
  return vars;
}

const themeDirectory = resolve(__dirname, "../../src-rust/aoidos-theme/assets/themes");
const themeNames = readdirSync(themeDirectory)
  .filter((name) => name.endsWith(".css"))
  .map((name) => name.slice(0, -".css".length));
const themes = new Map(
  themeNames.map((name) => [name, varsOf(blockOf(`:root[data-theme="${name}"]`))]),
);
const defaultTheme = themeCss.match(/:root,\s*:root\[data-theme="([a-z0-9-]+)"\]\s*\{/)?.[1];

function themeInlineBlock(): string {
  const start = css.indexOf("@theme inline {");
  const end = css.indexOf("}", start);
  return css.slice(start, end);
}

describe("token 目录与 app.css 互校", () => {
  it("锁定文档滚动，避免 WKWebView 整页橡皮筋回弹", () => {
    const documentRule = css.match(/html,\s*body\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(documentRule).toMatch(/width:\s*100%/);
    expect(documentRule).toMatch(/height:\s*100%/);
    expect(documentRule).toMatch(/margin:\s*0/);
    expect(documentRule).toMatch(/overflow:\s*hidden/);
    expect(documentRule).toMatch(/overscroll-behavior:\s*none/);
  });

  it("默认 CSS 为每个内置主题提供完整 token", () => {
    for (const [themeName, vars] of themes) {
      for (const { name } of CATALOG) {
        expect(vars.has(name), `${themeName} 缺少 ${name}`).toBe(true);
      }
    }
  });

  it("主题选择会改变 ink 值", () => {
    expect(new Set([...themes.values()].map((vars) => vars.get("--ink"))).size).toBeGreaterThan(1);
  });

  it("目录映射项与 @theme inline 一一对应", () => {
    const theme = themeInlineBlock();
    for (const { mapping } of CATALOG) {
      if (mapping === "") continue;
      expect(theme, `${mapping} 缺失`).toContain(`${mapping}: var(`);
    }
    // 映射层不出现目录之外的别名（防私增）
    const aliases = [...theme.matchAll(/(--[\w-]+):\s*var\(/g)].map((m) => m[1]);
    const expected = CATALOG.map((e) => e.mapping).filter((m) => m !== "");
    expect([...aliases].sort()).toEqual([...new Set(expected)].sort());
  });

  it("直接消费项（app-bg / ease / dur）不进 @theme", () => {
    const theme = themeInlineBlock();
    expect(theme).not.toContain("--color-app-bg");
    expect(theme).not.toContain("ease-signature");
    expect(theme).not.toContain("--dur-micro");
  });

  it("生成选择器覆盖所有内置主题，并保持深色为默认主题", () => {
    expect(defaultTheme).toBe("dark");
    expect(themeNames).toContain(defaultTheme);
    const selectors = [...themeCss.matchAll(/:root\[data-theme="([a-z0-9-]+)"\]\s*\{/g)].map(
      (match) => match[1],
    );
    expect([...selectors].sort()).toEqual([...themeNames].sort());
  });

  it("清默认色板，且 white / black 原语经 @theme static 强制发射", () => {
    expect(css).toMatch(/--color-\*:\s*initial/);
    const staticBlock = css.slice(
      css.indexOf("@theme static"),
      css.indexOf("}", css.indexOf("@theme static")),
    );
    expect(staticBlock).toContain("--color-white: #ffffff");
    expect(staticBlock).toContain("--color-black: #000000");
  });

  it("为减少动态效果的用户禁用记录区平滑滚动", () => {
    expect(css).toMatch(
      /prefers-reduced-motion:\s*reduce[\s\S]*?\.record-feed\s*\{[\s\S]*?scroll-behavior:\s*auto/,
    );
    expect(css).not.toMatch(/\.record-feed\s*\{[^}]*scroll-behavior:\s*smooth/);
  });
});
