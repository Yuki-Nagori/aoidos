import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import type { Page } from "playwright";
import { chromium } from "playwright";

type ThemeId = "dark" | "light" | "light-purple";
type Locale = "en" | "zh-Hans";
type Viewport = { width: number; height: number };
type Box = { x: number; y: number; width: number; height: number };
type LayoutBaseline = {
  source: { name: string; commit: string; screen: string };
  layout: {
    headerHeight: number;
    desktop: Record<
      string,
      {
        heroLeft: number;
        heroWidth: number;
        setupLeft: number;
        setupWidth: number;
        columnGap: number;
      }
    >;
    narrow: {
      width: number;
      heroLeft: number;
      heroWidth: number;
      setupLeft: number;
      setupWidth: number;
      columnGap: number;
    };
  };
};
type Fixture = {
  theme: ThemeId;
  locale: Locale;
  skin: boolean;
  keySet?: boolean;
  scriptsFail?: boolean;
  skinTokens: Record<string, string>;
  themes: { id: ThemeId; name: string; colorScheme: "dark" | "light" }[];
};
type BrowserBridge = {
  __TAURI_INTERNALS__: { invoke: (command: string) => Promise<unknown> };
  __AOIDOS_THEME_BOOTSTRAP__?: { version: number; theme: ThemeId; colorScheme: "dark" | "light" };
  __AOIDOS_LOCALE_BOOTSTRAP__?: { version: number; locale: Locale };
  __VISUAL_CALLS__?: string[];
};

const root = process.cwd();
const baseline = JSON.parse(
  await readFile(resolve(root, "tests/visual/welcome-layout.json"), "utf8"),
) as LayoutBaseline;
const accentByTheme: Record<ThemeId, string> = {
  dark: "#5cc8b4",
  light: "#1f7a68",
  "light-purple": "#705697",
};
const themes: Fixture["themes"] = [
  { id: "dark", name: "Dark", colorScheme: "dark" },
  { id: "light", name: "Light", colorScheme: "light" },
  { id: "light-purple", name: "Light Purple", colorScheme: "light" },
];
const script = {
  scriptId: "mistbell",
  title: "雾钟地窖",
  displayNames: { "zh-Hans": "雾钟地窖", en: "Mistbell Cellar" },
  attributions: "visual test fixture",
};
const sourceCss = await readFile(resolve(root, "resources/scripts/mistbell/theme.css"), "utf8");
const skinTokens = Object.fromEntries(
  [...sourceCss.matchAll(/(--[a-z][a-z0-9-]*)\s*:\s*([^;]+);/g)].map((match) => [
    match[1],
    match[2]?.trim() ?? "",
  ]),
);
const scriptNames: Record<Locale, string> = { en: "Mistbell Cellar", "zh-Hans": "雾钟地窖" };
const headlines: Record<Locale, string> = {
  en: "Perform a story with AI — one that truly begins and ends.",
  "zh-Hans": "和 AI 一起，演出一段有始有终的故事。",
};

function near(actual: number, expected: number, label: string, tolerance = 1): void {
  assert.ok(
    Math.abs(actual - expected) <= tolerance,
    `${label}: expected ${expected} ±${tolerance}px, received ${actual}px`,
  );
}

async function reservePort(): Promise<number> {
  const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response() });
  const { port } = server;
  server.stop();
  if (port === undefined) throw new Error("Could not reserve a local Vite port");
  return port;
}

async function startWebServer(port: number): Promise<Bun.Subprocess> {
  for (const script of [
    "theme:defaults:build",
    "build:theme-bootstrap",
    "build:locale-bootstrap",
  ]) {
    const result = Bun.spawnSync(["bun", "run", script], {
      cwd: root,
      stdout: "inherit",
      stderr: "inherit",
    });
    if (result.exitCode !== 0)
      throw new Error(`Could not prepare visual-test assets (${script}, exit ${result.exitCode})`);
  }

  const server = Bun.spawn(
    [
      "bun",
      "node_modules/vite/bin/vite.js",
      "--host",
      "127.0.0.1",
      "--port",
      String(port),
      "--strictPort",
    ],
    { cwd: root, stdout: "inherit", stderr: "inherit" },
  );
  const url = `http://127.0.0.1:${port}/`;
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (server.exitCode !== null)
      throw new Error(`Vite exited before starting (exit code ${server.exitCode})`);
    try {
      const response = await fetch(url);
      if (response.ok) return server;
    } catch {
      // Vite 尚未开始监听时继续短暂重试。
    }
    await Bun.sleep(200);
  }
  server.kill();
  throw new Error(`Vite did not become ready at ${url}`);
}

async function openWelcome(
  browser: Awaited<ReturnType<typeof chromium.launch>>,
  viewport: Viewport,
  fixture: Fixture,
) {
  const page = await browser.newPage({ viewport, deviceScaleFactor: 1 });
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  await page.addInitScript(
    (data: Fixture & { script: typeof script }) => {
      const bridge = window as unknown as Window & BrowserBridge;
      bridge.__AOIDOS_THEME_BOOTSTRAP__ = {
        version: 1,
        theme: data.theme,
        colorScheme: data.theme === "dark" ? "dark" : "light",
      };
      bridge.__AOIDOS_LOCALE_BOOTSTRAP__ = { version: 1, locale: data.locale };
      bridge.__VISUAL_CALLS__ = [];
      let currentTheme = data.theme;
      let currentLocale = data.locale;
      bridge.__TAURI_INTERNALS__ = {
        invoke: async (command: string, args?: Record<string, unknown>) => {
          bridge.__VISUAL_CALLS__?.push(command);
          switch (command) {
            case "llm_list_profiles":
              return { items: [] };
            case "llm_get_key_status":
              return { set: data.keySet ?? true, hint: data.keySet === false ? null : "1234" };
            case "engine_list_scripts":
              if (data.scriptsFail) throw { code: "store.io", message: "visual fixture failure" };
              return [data.script];
            case "store_get_ui_preferences":
              return { version: 1, panelPinned: false, diceMode: "manual" };
            case "theme_get_preference":
              return { version: 1, theme: currentTheme };
            case "theme_set_preference":
              currentTheme = (args?.theme as ThemeId | undefined) ?? currentTheme;
              return { version: 1, theme: currentTheme };
            case "theme_list":
              return data.themes;
            case "theme_skin_load":
              return {
                scriptId: "mistbell",
                status: data.skin ? "valid" : "missing",
                tokens: data.skin ? data.skinTokens : {},
                warnings: [],
                warningsTruncated: false,
              };
            case "locale_get_preference":
              return {
                preference: { version: 1, locale: currentLocale },
                resolvedLocale: currentLocale,
                nativeStatus: "applied",
              };
            case "locale_set_preference":
              currentLocale =
                (args?.preference as { locale?: Locale } | undefined)?.locale ?? currentLocale;
              return {
                preference: { version: 1, locale: currentLocale },
                resolvedLocale: currentLocale,
                nativeStatus: "applied",
              };
            default:
              throw new Error(`Unexpected visual fixture IPC: ${command}`);
          }
        },
      };
    },
    { ...fixture, script },
  );
  await page.goto(`http://127.0.0.1:${process.env.AOIDOS_VISUAL_PORT}/`, {
    waitUntil: "networkidle",
  });
  await page.getByRole("heading", { name: headlines[fixture.locale] }).waitFor();
  if (!fixture.scriptsFail)
    await page.getByText(scriptNames[fixture.locale], { exact: true }).waitFor();
  return { page, pageErrors };
}

async function measure(page: Page) {
  return page.evaluate(() => {
    const box = (selector: string): Box => {
      const rect = document.querySelector(selector)?.getBoundingClientRect();
      if (!rect) throw new Error(`Missing visual landmark: ${selector}`);
      return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
    };
    const root = document.documentElement;
    const body = box(".welcome-body");
    return {
      root: { width: root.clientWidth, height: root.clientHeight, scrollWidth: root.scrollWidth },
      bodyOverflow: {
        width: (document.querySelector(".welcome-body") as HTMLElement).clientWidth,
        scrollWidth: (document.querySelector(".welcome-body") as HTMLElement).scrollWidth,
      },
      theme: root.dataset.theme,
      locale: root.lang,
      colorScheme: getComputedStyle(root).colorScheme,
      accent: getComputedStyle(root).getPropertyValue("--accent").trim(),
      header: box(".topbar"),
      body,
      hero: box(".hero"),
      setup: box(".setup"),
      calls: (window as unknown as Window & BrowserBridge).__VISUAL_CALLS__ ?? [],
    };
  });
}

function assertGeometry(result: Awaited<ReturnType<typeof measure>>, viewport: Viewport): void {
  const { header, body, hero, setup, root: rendered } = result;
  assert.equal(rendered.width, viewport.width, "The app must stay inside the target window width");
  assert.equal(
    rendered.height,
    viewport.height,
    "The app must stay inside the target window height",
  );
  assert.ok(rendered.scrollWidth <= viewport.width, "The document must not overflow horizontally");
  assert.ok(
    result.bodyOverflow.scrollWidth <= result.bodyOverflow.width,
    "Welcome content must not overflow horizontally inside its scroll container",
  );
  near(header.height, baseline.layout.headerHeight, "top bar height");
  near(body.y, baseline.layout.headerHeight, "welcome body top");
  near(body.height, viewport.height - baseline.layout.headerHeight, "welcome body height");

  if (viewport.width <= baseline.layout.narrow.width) {
    const expected = baseline.layout.narrow;
    near(hero.x, expected.heroLeft, "narrow hero left");
    near(hero.width, expected.heroWidth, "narrow hero width");
    near(hero.y, baseline.layout.headerHeight + 16, "narrow hero top");
    near(setup.x, expected.setupLeft, "narrow setup left");
    near(setup.width, expected.setupWidth, "narrow setup width");
    near(setup.y, hero.y + hero.height + expected.columnGap, "narrow vertical gap");
    return;
  }

  const expected = baseline.layout.desktop[String(viewport.width)];
  assert.ok(expected, `No pinned Aoidos Art layout for ${viewport.width}px`);
  near(hero.x, expected.heroLeft, "desktop hero left");
  near(hero.width, expected.heroWidth, "desktop hero width");
  near(setup.x, expected.setupLeft, "desktop setup left");
  near(setup.width, expected.setupWidth, "desktop setup width");
  near(setup.x - (hero.x + hero.width), expected.columnGap, "desktop column gap");
  near(hero.y + hero.height / 2, setup.y + setup.height / 2, "desktop columns vertical alignment");
}

const port = await reservePort();
process.env.AOIDOS_VISUAL_PORT = String(port);
const devServer = await startWebServer(port);
let browser: Awaited<ReturnType<typeof chromium.launch>> | undefined;
try {
  browser = await chromium.launch({ headless: true });
  for (const theme of themes.map(({ id }) => id)) {
    for (const locale of ["en", "zh-Hans"] as const) {
      for (const skin of [false, true]) {
        const { page, pageErrors } = await openWelcome(
          browser,
          { width: 960, height: 640 },
          {
            theme,
            locale,
            skin,
            skinTokens,
            themes,
          },
        );
        const result = await measure(page);
        assert.equal(result.theme, theme, "The selected theme must reach the document root");
        assert.equal(result.locale, locale, "The selected locale must reach the document root");
        assert.equal(result.colorScheme, theme === "dark" ? "dark" : "light");
        assert.equal(result.accent, skin ? skinTokens["--accent"] : accentByTheme[theme]);
        assertGeometry(result, { width: 960, height: 640 });
        assert.ok(
          await page.locator(".welcome-start").isEnabled(),
          "A configured start action should be available",
        );
        assert.deepEqual(pageErrors, [], "The page must render without uncaught errors");
        assert.ok(
          !result.calls.includes("engine_open_session"),
          "Opening the app must not start a game",
        );
        await page.close();
      }
    }
  }

  for (const viewport of [
    { width: 640, height: 480 },
    { width: 1440, height: 900 },
  ]) {
    const { page, pageErrors } = await openWelcome(browser, viewport, {
      theme: "dark",
      locale: "en",
      skin: true,
      skinTokens,
      themes,
    });
    assertGeometry(await measure(page), viewport);
    assert.deepEqual(pageErrors, [], "The page must render without uncaught errors");
    await page.close();
  }

  for (const fixture of [
    { keySet: false, scriptsFail: false },
    { keySet: true, scriptsFail: true },
  ]) {
    const { page, pageErrors } = await openWelcome(
      browser,
      { width: 960, height: 640 },
      {
        theme: "dark",
        locale: "en",
        skin: true,
        skinTokens,
        themes,
        ...fixture,
      },
    );
    const warning = await page
      .locator(fixture.keySet ? '[role="alert"]' : '[role="status"]')
      .boundingBox();
    const actions = await page.locator(".start-actions").boundingBox();
    assert.ok(warning && actions, "Failure and unavailable-key states must remain visible");
    assert.ok(
      warning.y + warning.height <= actions.y || warning.y >= actions.y + actions.height,
      "Inline warnings must not overlap the start actions",
    );
    if (!fixture.keySet) assert.equal(await page.locator(".welcome-start").isDisabled(), true);
    assert.deepEqual(pageErrors, [], "Expected API failures must remain handled UI state");
    await page.close();
  }

  console.log(
    `欢迎页视觉布局检查通过：${baseline.source.commit.slice(0, 7)}，3 个主题、2 种语言、Mistbell 皮肤开关、3 个窗口和 2 种失败状态。`,
  );
} finally {
  await browser?.close();
  devServer.kill("SIGTERM");
  await devServer.exited;
}
