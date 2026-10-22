import assert from "node:assert/strict";
import { mkdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import type { Page } from "playwright";
import { chromium } from "playwright";

type ThemeId = "dark" | "light" | "light-purple";
type Locale = "en" | "zh-Hans";
type Viewport = { width: number; height: number };
type Box = { x: number; y: number; width: number; height: number };
type LayoutBaseline = {
  source: { name: string; commit: string; screens: string[] };
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
    settings: {
      top: number;
      right: number;
      desktopWidth: number;
      desktopNavWidth: number;
      narrowMaxWidth: number;
      narrowNavWidth: number;
    };
  };
};
type Fixture = {
  theme: ThemeId;
  locale: Locale;
  skin: boolean;
  activeSession?: boolean;
  keySet?: boolean;
  scriptsFail?: boolean;
  skinTokens: Record<string, string>;
  themes: { id: ThemeId; name: string; colorScheme: "dark" | "light" }[];
};
type BrowserBridge = {
  __TAURI_INTERNALS__: {
    invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
    transformCallback: (callback: (...args: unknown[]) => void, once?: boolean) => number;
  };
  __AOIDOS_THEME_BOOTSTRAP__?: { version: number; theme: ThemeId; colorScheme: "dark" | "light" };
  __AOIDOS_LOCALE_BOOTSTRAP__?: { version: number; locale: Locale };
  __VISUAL_CALLS__?: string[];
};

const root = process.cwd();
const captureDir = process.env.AOIDOS_VISUAL_CAPTURE_DIR;
if (captureDir) await mkdir(captureDir, { recursive: true });

async function captureVisual(page: Page, name: string): Promise<void> {
  if (!captureDir) return;
  await page.screenshot({ path: resolve(captureDir, name), fullPage: true });
}

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
  suggestedActions: { "zh-Hans": "这是什么地方？", en: "Where am I?" },
  attributions:
    "# Mistbell License\n\nReleased under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).\n\n# SRD Attribution\n\n## Scope\n\n- Preserve attribution.\n- Keep source references visible.",
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
      const callbacks = new Map<number, (...args: unknown[]) => void>();
      let nextCallbackId = 1;
      const session = {
        sessionId: "00000000-0000-4000-8000-000000000001",
        memoryRunId: "00000000-0000-4000-8000-000000000002",
        profileId: "visual-profile",
        scriptId: "mistbell",
        title: "雾钟地窖",
      };
      const phase = {
        sessionId: session.sessionId,
        stateEpoch: "00000000-0000-4000-8000-000000000003",
        phaseRevision: 1,
        historyRevision: 1,
        phase: "idle",
        scene: {
          sceneId: "threshold",
          path: [{ kind: "scene", id: "threshold", title: "塔门与石阶" }],
        },
        needsRecovery: false,
        resumeRequired: false,
        seq: { phaseChanged: 0, sceneAdvanced: 0, operationDone: 0, operationFailed: 0 },
      };
      const records = {
        sessionId: session.sessionId,
        items: [
          {
            recordSeq: 2,
            kind: "narration",
            createdAt: "2026-10-19T00:00:00Z",
            body: {
              text: "塔门后的呼吸声停了一瞬。",
              turnId: "00000000-0000-4000-8000-000000000004",
              outcome: "completed",
              finishReason: "stop",
            },
            turnId: "00000000-0000-4000-8000-000000000004",
            outcome: "completed",
          },
          {
            recordSeq: 1,
            kind: "playerSpeech",
            createdAt: "2026-10-19T00:00:00Z",
            body: { playerId: "player", text: "我先观察门缝。", mode: "inCharacter" },
          },
        ],
        lastSeq: 0,
        lastRecordSeq: 2,
        viewEpoch: "00000000-0000-4000-8000-000000000005",
        needsRecovery: false,
      };
      const memoryParameters = {
        algorithm: "segmented",
        baseStrength: 1_000_000,
        reinforcementGain: 120_000,
        reinforcementCap: 600_000,
        strengthCap: 1_000_000,
        halfLifeRounds: 24,
        repeatWindowRounds: 6,
        repeatLimit: 2,
        revivalCooldownRounds: 12,
        activeCapacity: 64,
        rateTableVersion: 1,
        decayRate: 993_092,
      };
      const question = {
        questionId: "visual-question",
        runId: session.memoryRunId,
        questionKey: Array(32).fill(1),
        revision: 1,
        draft: {
          version: 1,
          identity: { kind: "entityAssociation", eventSlot: 1, sources: [] },
          historyRevision: 1,
          entityRevision: 1,
          evidenceRevision: 1,
          mentionedName: "洛苇",
          candidates: [{ kind: "entity", entityId: "keeper", displayName: "洛苇" }],
        },
        status: "pending",
        answer: null,
      };
      const memoryEntry = {
        entryId: "visual-entry",
        currentVersionId: "visual-version",
        stateRevision: 1,
        state: "active",
      };
      const transformCallback = (callback: (...args: unknown[]) => void) => {
        const id = nextCallbackId++;
        callbacks.set(id, callback);
        return id;
      };
      bridge.__TAURI_INTERNALS__ = {
        transformCallback,
        invoke: async (command: string, args?: Record<string, unknown>) => {
          bridge.__VISUAL_CALLS__?.push(command);
          switch (command) {
            case "plugin:event|listen":
              return nextCallbackId++;
            case "plugin:event|unlisten":
              callbacks.delete(args?.eventId as number);
              return null;
            case "llm_list_profiles":
              return { items: [] };
            case "llm_save_profile":
              return args?.profile;
            case "llm_get_key_status":
              return { set: data.keySet ?? true, hint: data.keySet === false ? null : "1234" };
            case "engine_list_scripts":
              if (data.scriptsFail) throw { code: "store.io", message: "visual fixture failure" };
              return [data.script];
            case "engine_open_session":
              if (!data.activeSession)
                throw new Error("Unexpected session open in welcome fixture");
              return session;
            case "engine_get_phase":
              return phase;
            case "engine_get_record_view":
              return records;
            case "engine_get_scene_presence":
              return {
                sessionId: session.sessionId,
                sceneId: "threshold",
                presenceRevision: "visual-presence-v1",
                characters: [
                  { id: "keeper", kind: "character", scope: "global", canonicalName: "洛苇" },
                ],
              };
            case "store_get_ui_preferences":
              return { version: 1, panelPinned: false, diceMode: "manual" };
            case "store_set_ui_preferences":
              return {
                version: 1,
                panelPinned: Boolean(args?.panelPinned),
                diceMode: args?.diceMode === "auto" ? "auto" : "manual",
              };
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
            case "budget_get_settings":
              return { runId: session.memoryRunId, cnyLimit: "20", usdLimit: "5", revision: 1 };
            case "budget_get_deepseek_price_templates":
              return [
                {
                  currency: "CNY",
                  inputUncached: "1",
                  inputCached: "0.02",
                  inputCachedWrite: "1",
                  output: "4",
                  checkedAt: "2026-10-21T00:00:00Z",
                  sourceUrl: "https://api-docs.deepseek.com/zh-cn/quick_start/pricing/",
                },
                {
                  currency: "USD",
                  inputUncached: "0.15",
                  inputCached: "0.003",
                  inputCachedWrite: "0.15",
                  output: "0.6",
                  checkedAt: "2026-10-21T00:00:00Z",
                  sourceUrl: "https://api-docs.deepseek.com/quick_start/pricing/",
                },
              ];
            case "budget_get_selected_price":
            case "budget_get_price":
              return { price: null };
            case "budget_get_provider_balance":
              return {
                providerId: "deepseek",
                queriedAt: "2026-10-19T00:00:00Z",
                stale: false,
                status: "available",
                accounts: [{ currency: "CNY", total: "10.00", granted: "0", toppedUp: "10.00" }],
              };
            case "budget_get_period":
              return {
                runId: session.memoryRunId,
                items: [
                  {
                    providerId: "deepseek",
                    currency: "CNY",
                    modelId: "deepseek-flash",
                    settledAmount: "1.25",
                    reservedAmount: "0",
                    knownTokens: 2400,
                    incompleteRequests: 0,
                    requestCount: 1,
                  },
                ],
                warnings: [],
              };
            case "budget_get_monthly":
              return { month: "2026-10", timeZone: "Asia/Shanghai", items: [] };
            case "budget_get_suggested_limits":
              return {
                cny: "20",
                usd: "5",
                referenceInputTokens: 1000000,
                referenceOutputTokens: 100000,
              };
            case "budget_list_requests":
              return { revision: "visual-ledger-v1", items: [] };
            case "memory_get_app_config":
              return {
                configVersion: 1,
                algorithmVersion: 1,
                parameters: memoryParameters,
                defaultParameters: memoryParameters,
              };
            case "memory_get_script_config":
              return {
                scriptId: "mistbell",
                configVersion: 1,
                appConfigVersion: 1,
                algorithmVersion: 1,
                overrides: {},
                inherited: memoryParameters,
                effective: memoryParameters,
              };
            case "memory_get_active_batch":
              return null;
            case "memory_list_entries":
              return { entries: [memoryEntry], nextCursor: null, historyRevision: 1 };
            case "memory_list_questions":
              return { questions: [question], nextCursor: null, historyRevision: 1 };
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

async function measureSettings(page: Page) {
  return page.evaluate(() => {
    const dialog = document.querySelector(".settings-dialog");
    const nav = document.querySelector(".settings-nav");
    const content = document.querySelector(".settings-content");
    if (
      !(dialog instanceof HTMLElement) ||
      !(nav instanceof HTMLElement) ||
      !(content instanceof HTMLElement)
    )
      throw new Error("Settings dialog landmarks are missing");
    return {
      dialog: (() => {
        const { x, y, width, height } = dialog.getBoundingClientRect();
        return { x, y, width, height };
      })(),
      nav: (() => {
        const { width } = nav.getBoundingClientRect();
        const heading = nav.querySelector<HTMLElement>(".settings-nav-heading");
        const title = heading?.querySelector<HTMLElement>("h2");
        if (!heading || !title) throw new Error("Settings navigation title is missing");
        const colorProbe = document.createElement("span");
        colorProbe.style.color = getComputedStyle(document.documentElement)
          .getPropertyValue("--muted")
          .trim();
        document.body.append(colorProbe);
        const mutedColor = getComputedStyle(colorProbe).color;
        colorProbe.remove();
        return {
          width,
          scrollWidth: nav.scrollWidth,
          clientWidth: nav.clientWidth,
          gap: getComputedStyle(nav).rowGap,
          padding: getComputedStyle(nav).padding,
          headingPadding: getComputedStyle(heading).padding,
          titleFontSize: getComputedStyle(title).fontSize,
          titleColor: getComputedStyle(title).color,
          titleFontWeight: getComputedStyle(title).fontWeight,
          mutedColor,
        };
      })(),
      content: { scrollWidth: content.scrollWidth, clientWidth: content.clientWidth },
      visibleSections: Array.from(
        document.querySelectorAll<HTMLElement>(".settings-section"),
      ).filter((section) => getComputedStyle(section).display !== "none").length,
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
        if (theme === "dark" && locale === "en" && !skin)
          await captureVisual(page, "welcome-dark-en.png");
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
    await captureVisual(page, `welcome-${viewport.width}x${viewport.height}.png`);
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
    assert.equal(
      await page.evaluate(() =>
        (window as unknown as BrowserBridge).__VISUAL_CALLS__?.includes("llm_get_key_status"),
      ),
      false,
      "Startup must not probe the OS credential store",
    );
    if (!fixture.keySet) {
      assert.equal(await page.locator(".welcome-start").isDisabled(), false);
      await page.locator(".model-settings").click();
      const dialog = page.getByRole("dialog");
      const pricePreview = dialog.locator(".settings-price-preview");
      await pricePreview.getByText("Built-in reference price · per million tokens").waitFor();
      assert.match(await pricePreview.innerText(), /Uncached input\s+1/);
      assert.match(await pricePreview.innerText(), /Checked: 2026-10-21/);
      await dialog.getByRole("button", { name: "Retry" }).click();
      await dialog.getByText("Key is not set").waitFor();
      await dialog.getByRole("button", { name: "Close" }).click();
      assert.equal(await page.locator(".welcome-start").isDisabled(), true);
      assert.equal(
        await page.evaluate(() =>
          (window as unknown as BrowserBridge).__VISUAL_CALLS__?.includes("llm_get_key_status"),
        ),
        true,
        "An explicit key-status check must query the credential store",
      );
    }
    assert.deepEqual(pageErrors, [], "Expected API failures must remain handled UI state");
    await page.close();
  }

  for (const viewport of [
    { width: 960, height: 640 },
    { width: 700, height: 640 },
    { width: 640, height: 480 },
  ]) {
    const { page, pageErrors } = await openWelcome(browser, viewport, {
      theme: "dark",
      locale: "en",
      skin: true,
      skinTokens,
      themes,
      activeSession: true,
    });
    await page.locator(".welcome-start").click();
    await page.locator(".game-stage").waitFor();
    await page.locator('.game-topbar button[aria-label="Open dialogue panel"]').click();
    await page.locator(".dialogue-panel.dialogue-panel-open").waitFor();
    await page.getByRole("button", { name: "Pin dialogue panel" }).click();
    await page.locator('.game-stage[data-panel-state="pinned"]').waitFor();
    assert.equal(await page.locator(".game-stage").getAttribute("data-panel-state"), "pinned");
    await page.waitForFunction(() => {
      const panel = document.querySelector<HTMLElement>(".dialogue-panel");
      if (!panel) return false;
      const style = getComputedStyle(panel);
      return style.opacity === "1" && new DOMMatrixReadOnly(style.transform).m41 === 0;
    });
    const composer = page.locator(".game-composer-input");
    await composer.waitFor();
    assert.equal(
      await page.locator(".dialogue-panel .select-menu").count(),
      1,
      "The gameplay panel keeps recipient selection in the composer, not dice preferences",
    );
    assert.equal(await composer.locator(".game-composer-key-hints kbd").count(), 2);
    assert.equal(await composer.locator(".game-composer-submit-icon").count(), 1);
    const composerLayout = await composer.evaluate((element) => {
      const input = element.getBoundingClientRect();
      const text = element.querySelector("textarea")?.getBoundingClientRect();
      const submit = element.querySelector(".game-composer-submit")?.getBoundingClientRect();
      return {
        width: input.width,
        scrollWidth: element.scrollWidth,
        textInside: Boolean(text && text.left >= input.left && text.right <= input.right),
        submitInside: Boolean(submit && submit.left >= input.left && submit.right <= input.right),
      };
    });
    assert.ok(composerLayout.width > 0, "The action composer must be visible in an active run");
    assert.ok(
      composerLayout.scrollWidth <= composerLayout.width,
      "The action composer must not overflow its input card horizontally",
    );
    assert.ok(composerLayout.textInside, "The draft input stays inside the shared composer card");
    assert.ok(
      composerLayout.submitInside,
      "The submit action stays inside the shared composer card",
    );
    await composer.locator("textarea").fill("/o");
    const commandMenu = page.getByRole("listbox", { name: "Command suggestions" });
    await commandMenu.waitFor();
    assert.equal(await commandMenu.getByRole("option").count(), 8);
    assert.equal(
      await commandMenu.getByRole("option", { name: /\/ooc/ }).getAttribute("aria-selected"),
      "true",
    );
    assert.equal(
      await commandMenu.getByRole("option", { name: /\/stop/ }).getAttribute("aria-disabled"),
      "true",
    );
    if (viewport.width === 960) await captureVisual(page, "gameplay-slash-menu.png");
    if (viewport.height === 480) {
      const textarea = composer.locator("textarea");
      const enabledCount = await commandMenu
        .locator('[role="option"][aria-disabled="false"]')
        .count();
      for (let index = 1; index < enabledCount; index += 1) {
        await textarea.press("ArrowDown");
      }
      const activeId = await textarea.getAttribute("aria-activedescendant");
      assert.ok(activeId, "Keyboard navigation exposes an active option to assistive technology");
      const activeLayout = await page.evaluate((optionId) => {
        const menu = document.getElementById(`${optionId}`)?.closest<HTMLElement>("[role=listbox]");
        const option = document.getElementById(optionId);
        if (!menu || !option) return null;
        const menuRect = menu.getBoundingClientRect();
        const optionRect = option.getBoundingClientRect();
        return {
          controlsId: document.querySelector(".game-composer-text")?.getAttribute("aria-controls"),
          menuId: menu.id,
          top: optionRect.top >= menuRect.top,
          bottom: optionRect.bottom <= menuRect.bottom,
        };
      }, activeId);
      assert.ok(activeLayout, "The active command belongs to the visible listbox");
      assert.equal(activeLayout.controlsId, activeLayout.menuId);
      assert.ok(activeLayout.top && activeLayout.bottom, "The selected command scrolls into view");
    }
    await composer.locator("textarea").fill("");
    assert.equal(
      await page.locator(".game-topbar .title").innerText(),
      scriptNames.en,
      "An active run title follows the selected interface language",
    );
    if (viewport.width === 960) await captureVisual(page, "gameplay-active.png");
    await page.locator('.topbar button[aria-label="Settings"]').click();
    await page.getByRole("dialog").waitFor();
    const modelChip = page.locator(".game-model-chip");
    await modelChip.waitFor({ state: "attached" });
    if (await modelChip.isVisible()) {
      assert.equal(
        await modelChip
          .locator(".sep")
          .first()
          .evaluate((element) => getComputedStyle(element).width),
        "1px",
        "Provider and model names need a visible design separator",
      );
    }

    const layout = await measureSettings(page);
    const narrow = viewport.width <= baseline.layout.settings.narrowMaxWidth;
    near(
      layout.dialog.x,
      narrow
        ? baseline.layout.settings.right
        : viewport.width - baseline.layout.settings.desktopWidth - baseline.layout.settings.right,
      "settings drawer left",
    );
    near(layout.dialog.y, baseline.layout.settings.top, "settings drawer top");
    near(
      layout.dialog.width,
      narrow
        ? viewport.width - baseline.layout.settings.right * 2
        : baseline.layout.settings.desktopWidth,
      "settings drawer width",
    );
    near(
      layout.nav.width,
      narrow ? baseline.layout.settings.narrowNavWidth : baseline.layout.settings.desktopNavWidth,
      "settings navigation width",
    );
    assert.equal(layout.nav.gap, "2px", "Settings navigation uses the source vertical spacing");
    assert.equal(layout.nav.padding, "12px 8px", "Settings navigation uses the source inset");
    assert.equal(layout.nav.headingPadding, "8px 10px 4px");
    assert.equal(layout.nav.titleFontSize, "10.5px");
    assert.equal(layout.nav.titleColor, layout.nav.mutedColor);
    assert.equal(layout.nav.titleFontWeight, "400");
    assert.ok(
      layout.nav.scrollWidth <= layout.nav.clientWidth,
      "Settings navigation must not clip horizontally",
    );
    assert.ok(
      layout.content.scrollWidth <= layout.content.clientWidth,
      "Settings section must not overflow horizontally",
    );

    const sections = [
      { id: "appearance", target: "appearance" },
      { id: "api", target: "api" },
      { id: "gameplay", target: "gameplay" },
      { id: "billing", target: "billing" },
      { id: "price", target: "billing" },
      { id: "balance", target: "billing" },
      { id: "memory", target: "memory" },
      { id: "memory-list", target: "memory-query" },
      { id: "memory-pending", target: "memory-query" },
      { id: "shortcuts", target: "shortcuts" },
      { id: "scenario", target: "scenario" },
    ];
    for (const [index, section] of sections.entries()) {
      const button = page.locator(".settings-nav button").nth(index);
      await button.click();
      assert.equal(await button.getAttribute("aria-current"), "page");
      if (section.id === "billing") {
        await page.locator("#billing .billing-summary-card").first().waitFor();
        assert.equal(
          await page.locator("#billing .settings-section-heading h3").innerText(),
          "Run cost limits and model price",
          "Billing uses the design's section title without a duplicate panel heading",
        );
        assert.equal(await page.locator("#billing .billing-panel > h4").count(), 0);
        const divider = page.locator("#billing hr").first();
        const borderColor = await divider.evaluate(
          (element) => getComputedStyle(element).borderTopColor,
        );
        const hairline = await page
          .locator("html")
          .evaluate((element) => getComputedStyle(element).getPropertyValue("--hairline").trim());
        assert.equal(borderColor, hairline, "Billing dividers must use the theme hairline token");
      }
      if (section.id === "gameplay") {
        assert.equal(await page.locator("#gameplay .select-menu").count(), 0);
        assert.equal(await page.locator("#gameplay .settings-subheading").innerText(), "Run");
        assert.equal(
          await page.locator("#gameplay .settings-session-note").innerText(),
          "Reopening a cycle generates nothing on its own; a new cycle keeps cycle marks.",
        );
        assert.deepEqual(
          await page
            .locator("#gameplay .settings-actions button")
            .first()
            .evaluate((element) => ({
              height: getComputedStyle(element).height,
              fontSize: getComputedStyle(element).fontSize,
              borderRadius: getComputedStyle(element).borderRadius,
            })),
          { height: "30px", fontSize: "12px", borderRadius: "8px" },
        );
      }
      if (section.id === "price") {
        const form = page.locator("#billing .billing-price-form");
        await form.waitFor();
        assert.equal(
          await page.locator("#billing .settings-section-heading h3").innerText(),
          "Register a new immutable price",
          "Price registration uses the design's page title once",
        );
        assert.equal(
          await form.locator(".billing-price-base-grid .billing-price-field").count(),
          2,
        );
        assert.equal(
          await form
            .locator(".billing-price-rates-grid")
            .first()
            .locator(".billing-price-field")
            .count(),
          3,
        );
        const rateColumns = await form
          .locator(".billing-price-rates-grid")
          .first()
          .evaluate((element) => {
            const columns = Array.from(element.children).map((child) =>
              Math.round(child.getBoundingClientRect().left),
            );
            return new Set(columns).size;
          });
        assert.equal(
          rateColumns,
          viewport.width <= 720 ? 2 : 3,
          `Price rates must follow the design's ${viewport.width <= 720 ? "two" : "three"}-column layout at ${viewport.width}px`,
        );
        const overflows = await form.evaluate((element) => {
          const right = element.getBoundingClientRect().right;
          return Array.from(element.querySelectorAll("input")).some(
            (input) => input.getBoundingClientRect().right > right + 1,
          );
        });
        assert.equal(overflows, false, "Price inputs must stay inside the form grid");
      }
      if (section.id === "scenario") {
        const credits = page.locator("#scenario .scenario-attribution-document");
        assert.equal(await credits.count(), 2, "Each bundled notice should have its own card");
        assert.equal(await page.locator("#scenario .scenario-attributions pre").count(), 0);
        assert.equal(await page.locator("#scenario .scenario-resource-table tr").count(), 2);
        const resourceText =
          (await page.locator("#scenario .scenario-resource-table").textContent()) ?? "";
        assert.match(resourceText, /theme\.css/);
        assert.match(resourceText, /4 tokens/);
        await page
          .locator('#scenario a[href="https://creativecommons.org/licenses/by/4.0/"]')
          .waitFor();
      }
      if (section.id === "api") {
        const currencyCard = page.locator("#api .settings-currency-card");
        const priceLayout = await currencyCard.evaluate((element) => {
          const heading = element.querySelector<HTMLElement>(".settings-currency-heading");
          const select = element.querySelector<HTMLElement>(".select-menu");
          const preview = element.querySelector<HTMLElement>(".settings-price-preview");
          const grid = preview?.querySelector<HTMLElement>("dl");
          if (!heading || !select || !preview || !grid)
            throw new Error("API price controls are missing");
          return {
            currencyDisplay: getComputedStyle(element).display,
            currencyWidth: element.getBoundingClientRect().width,
            headingColumns: getComputedStyle(heading).gridTemplateColumns.split(" ").length,
            selectWidth: select.getBoundingClientRect().width,
            previewWidth: preview.getBoundingClientRect().width,
            previewScrollWidth: preview.scrollWidth,
            priceColumns: getComputedStyle(grid).gridTemplateColumns.split(" ").length,
            rows: Array.from(grid.children).map((row) => {
              const rect = row.getBoundingClientRect();
              const label = row.querySelector<HTMLElement>("dt")!.getBoundingClientRect();
              const value = row.querySelector<HTMLElement>("dd")!.getBoundingClientRect();
              return {
                left: rect.left,
                right: rect.right,
                labelLeft: label.left,
                labelRight: label.right,
                valueLeft: value.left,
                valueRight: value.right,
              };
            }),
          };
        });
        if (viewport.width <= 720) {
          assert.equal(priceLayout.currencyDisplay, "grid");
          assert.equal(priceLayout.headingColumns, 1);
          assert.equal(priceLayout.priceColumns, 1);
        }
        assert.ok(
          priceLayout.selectWidth > 0 && priceLayout.selectWidth <= priceLayout.currencyWidth,
        );
        assert.ok(
          priceLayout.previewScrollWidth <= priceLayout.previewWidth,
          `Price preview must not overflow: ${JSON.stringify(priceLayout)}`,
        );
        assert.ok(
          priceLayout.rows.every(
            (row) =>
              row.labelLeft >= row.left - 1 &&
              row.labelRight <= row.right + 1 &&
              row.valueLeft >= row.left - 1 &&
              row.valueRight <= row.right + 1 &&
              row.labelRight <= row.valueLeft + 1,
          ),
          "API price labels and values must not overlap or escape their grid row",
        );
        const trigger = page.locator("#api .select-menu-trigger").first();
        const selectedValue = await trigger.innerText();
        await trigger.click();
        const listbox = page.locator('#api [role="listbox"]').first();
        await listbox.waitFor();
        const listboxBox = await listbox.boundingBox();
        assert.ok(listboxBox, "The custom selector must render its listbox");
        assert.ok(listboxBox.x >= 0 && listboxBox.x + listboxBox.width <= viewport.width);
        assert.ok(listboxBox.y >= 0 && listboxBox.y + listboxBox.height <= viewport.height);
        const activeBefore = await trigger.getAttribute("aria-activedescendant");
        await trigger.press("ArrowDown");
        assert.notEqual(
          await trigger.getAttribute("aria-activedescendant"),
          activeBefore,
          "ArrowDown must move the active listbox option",
        );
        await trigger.press("Escape");
        await listbox.waitFor({ state: "detached" });
        assert.equal(await trigger.innerText(), selectedValue, "Escape must not change selection");
        assert.equal(await trigger.evaluate((element) => document.activeElement === element), true);
      }
      if (section.id === "balance") {
        await page.getByRole("button", { name: /query deepseek balance/i }).click();
        await page.locator("#billing .billing-balance-card").waitFor();
      }
      if (section.id === "memory") {
        await page.locator("#memory input[type=number]").first().waitFor();
        assert.equal(await page.locator("#memory details").count(), 0);
        assert.equal(await page.locator("#memory .memory-settings-group").count(), 2);
        assert.equal(await page.locator('#memory input[name="memory-app-algorithm"]').count(), 3);
        assert.equal(
          await page
            .locator('#memory input[name="memory-app-algorithm"][value="segmented"]')
            .isChecked(),
          true,
          "The memory fixture mirrors the design's segmented algorithm state",
        );
        assert.deepEqual(
          await page
            .locator("#memory .memory-settings-group")
            .first()
            .locator(".memory-number-field > span")
            .allTextContents(),
          [
            "Base strength (1,000,000 = 1)",
            "Reinforcement gain",
            "Total reinforcement cap",
            "Strength cap",
            "Half-life rounds",
            "Repeat window rounds",
            "Maximum reinforcements in a window",
            "Revival cooldown rounds",
            "Active memory capacity",
          ],
          "Memory fields follow the design order for the selected algorithm",
        );
        const memoryColumns = await page
          .locator("#memory .memory-parameter-grid")
          .first()
          .evaluate((element) => {
            const columns = Array.from(element.children).map((child) =>
              Math.round(child.getBoundingClientRect().left),
            );
            return new Set(columns).size;
          });
        assert.equal(
          memoryColumns,
          2,
          "Memory parameters remain two columns at every design viewport",
        );
      }
      if (section.id === "memory-list")
        await page.locator('#memory-query button[aria-controls^="memory-entry-"]').waitFor();
      if (section.id === "memory-pending") await page.locator("#memory-query article").waitFor();
      if (section.id === "appearance") {
        assert.equal(
          await page.locator("select").count(),
          0,
          "App selectors must use the custom menu",
        );
        const themeCards = page.locator(".settings-theme-card");
        assert.equal(
          await themeCards.count(),
          themes.length,
          "Theme cards follow the Rust catalog",
        );
        assert.equal(await page.locator('.settings-theme-card[aria-pressed="true"]').count(), 1);
        const previews = await page.locator(".settings-theme-preview").evaluateAll((elements) =>
          elements.map((element) => ({
            accent: getComputedStyle(element).getPropertyValue("--accent").trim(),
            height: getComputedStyle(element).height,
            cardInk: getComputedStyle(element.parentElement!).color,
            labelInk: getComputedStyle(
              element.parentElement!.querySelector(".settings-theme-name")!,
            ).color,
            barWidthRatio:
              element.children[1]!.getBoundingClientRect().width /
              element.children[0]!.getBoundingClientRect().width,
            barColorsDistinct:
              getComputedStyle(element.children[0]!).backgroundColor !==
              getComputedStyle(element.children[1]!).backgroundColor,
          })),
        );
        assert.equal(
          new Set(previews.map(({ accent }) => accent)).size,
          themes.length,
          "Every theme card previews its own accent",
        );
        assert.ok(
          previews.every(({ height }) => height === "44px"),
          "Theme previews are 44px high",
        );
        assert.ok(
          previews.every(
            ({ barWidthRatio, barColorsDistinct }) =>
              Math.abs(barWidthRatio - 0.6) < 0.03 && barColorsDistinct,
          ),
          "Theme preview bars use 40%/24% widths and distinct accent tokens",
        );
        assert.equal(
          new Set(previews.map(({ cardInk, labelInk }) => `${cardInk}/${labelInk}`)).size,
          1,
          "Card frames and labels retain the selected application theme",
        );
        const themeColumns = await themeCards.evaluateAll(
          (cards) =>
            new Set(cards.map((card) => Math.round(card.getBoundingClientRect().left))).size,
        );
        assert.equal(
          themeColumns,
          viewport.width <= 720 ? 2 : 3,
          `Theme cards use the design grid at ${viewport.width}px`,
        );
        await page.locator('.settings-theme-card[aria-label="Light Purple"]').click();
        await page.waitForFunction(() => document.documentElement.dataset.theme === "light-purple");
        await page.locator('.settings-theme-card[aria-label="Dark"]').click();
        await page.waitForFunction(() => document.documentElement.dataset.theme === "dark");
      }
      if (section.id === "shortcuts") {
        await page.locator("#shortcuts .shortcut-row").first().waitFor();
        assert.equal(await page.locator("#shortcuts .shortcut-search").count(), 0);
        assert.ok(await page.locator("#shortcuts .shortcut-lock").count());
        await page.getByRole("button", { name: /view cheat sheet|查看速查表/i }).click();
        const shortcutDialog = page.getByRole("dialog", { name: /keyboard shortcuts|快捷键/i });
        await shortcutDialog.waitFor();
        const dialogBox = await shortcutDialog.boundingBox();
        assert.ok(dialogBox, "The shortcut reference should have a measurable dialog frame");
        assert.ok(dialogBox.x >= 16 && dialogBox.y >= 16);
        assert.ok(dialogBox.x + dialogBox.width <= viewport.width - 16);
        assert.ok(dialogBox.y + dialogBox.height <= viewport.height - 16);
        const shortcutLayout = await shortcutDialog.evaluate((element) => {
          const groups = element.querySelector<HTMLElement>(".shortcut-groups-dialog");
          if (!groups) throw new Error("Shortcut groups are missing");
          return {
            dialogWidth: element.clientWidth,
            dialogScrollWidth: element.scrollWidth,
            groupsWidth: groups.clientWidth,
            groupsScrollWidth: groups.scrollWidth,
            columns: getComputedStyle(groups).gridTemplateColumns.split(" ").length,
          };
        });
        assert.ok(shortcutLayout.dialogScrollWidth <= shortcutLayout.dialogWidth);
        assert.ok(shortcutLayout.groupsScrollWidth <= shortcutLayout.groupsWidth);
        assert.equal(
          shortcutLayout.columns,
          viewport.width <= 720 ? 1 : 2,
          "The shortcut reference must switch to one column in the narrow drawer",
        );
        assert.ok(await shortcutDialog.locator('input[type="search"]').count());
        assert.ok(await shortcutDialog.locator(".shortcut-row .tag").count());
        await shortcutDialog.locator('input[type="search"]').fill("Send");
        assert.equal(await shortcutDialog.locator(".shortcut-row").count(), 1);
        await shortcutDialog.press("Escape");
        await shortcutDialog.waitFor({ state: "detached" });
      }
      const sectionLayout = await measureSettings(page);
      assert.equal(
        sectionLayout.visibleSections,
        1,
        `Only the ${section.id} section should be visible`,
      );
      assert.equal(await button.getAttribute("aria-controls"), section.target);
      assert.equal(await page.locator(`#${section.target}`).isVisible(), true);
      if (section.id === "billing")
        assert.ok(await page.locator("#billing .billing-summary-card").count());
      if (section.id === "price")
        assert.ok(await page.locator("#billing .billing-price-form").count());
      if (section.id === "balance")
        assert.ok(await page.locator("#billing .billing-balance-card").count());
      if (section.id === "memory")
        assert.ok(await page.locator("#memory input[type=number]").count());
      if (section.id === "memory-list")
        assert.ok(
          await page.locator('#memory-query button[aria-controls^="memory-entry-"]').count(),
        );
      if (section.id === "memory-pending")
        assert.ok(await page.locator("#memory-query article").count());
      assert.ok(
        sectionLayout.content.scrollWidth <= sectionLayout.content.clientWidth,
        `${section.id} must not overflow horizontally at ${viewport.width}px (scrollWidth=${sectionLayout.content.scrollWidth}, clientWidth=${sectionLayout.content.clientWidth})`,
      );
      if (viewport.width === 960) await captureVisual(page, `settings-${section.id}.png`);
    }
    assert.deepEqual(pageErrors, [], "Settings sections must render without uncaught errors");
    await page.close();
  }

  console.log(
    `欢迎页与设置视觉布局检查通过：${baseline.source.commit.slice(0, 7)}，3 个主题、2 种语言、Mistbell 皮肤开关、3 个欢迎页窗口、3 个真实会话设置窗口、11 个分区（含账单 / 记忆数据 / 快捷键）和 2 种失败状态。`,
  );
} finally {
  await browser?.close();
  devServer.kill("SIGTERM");
  await devServer.exited;
}
