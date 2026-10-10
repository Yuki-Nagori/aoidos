import { effectScope, nextTick, ref, type Ref } from "vue";
import { afterEach, expect, it, vi } from "vitest";
import { useTheme } from "../../../src-web/composables/useTheme";
import type { Skin, ThemeId } from "../../../src-web/api/theme";
import {
  getThemePreference,
  getThemes,
  loadSkin,
  setThemePreference,
} from "../../../src-web/api/theme";

vi.mock("../../../src-web/api/theme", () => ({
  getThemePreference: vi.fn(),
  getThemes: vi.fn(),
  loadSkin: vi.fn(),
  setThemePreference: vi.fn(),
}));

const emptySkin = (scriptId = "mistbell"): Skin => ({
  scriptId,
  sourceHash: "a".repeat(64),
  status: "valid",
  tokens: {},
  warnings: [],
  warningsTruncated: false,
});

const skinWithAccent = (): Skin => ({
  ...emptySkin(),
  tokens: { "--accent": "#abcdef12" },
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

function setup(script = "mistbell", response = emptySkin(), configure?: () => void) {
  const sheets: string[] = [];
  class Sheet {
    replaceSync(value: string): void {
      sheets.push(value);
    }
  }
  vi.stubGlobal("CSSStyleSheet", Sheet);
  Object.defineProperty(document, "adoptedStyleSheets", {
    configurable: true,
    writable: true,
    value: [],
  });
  window.__AOIDOS_THEME_BOOTSTRAP__ = { theme: "dark" };
  vi.mocked(getThemePreference).mockResolvedValue({ version: 1, theme: "dark" });
  vi.mocked(getThemes).mockResolvedValue([
    { id: "dark", name: "深色", colorScheme: "dark" },
    { id: "light", name: "浅色", colorScheme: "light" },
    { id: "light-purple", name: "浅紫", colorScheme: "light" },
  ]);
  vi.mocked(setThemePreference).mockImplementation(async (theme) => ({ version: 1, theme }));
  vi.mocked(loadSkin).mockResolvedValue(response);
  configure?.();
  const id: Ref<string> = ref(script);
  const scope = effectScope();
  const theme = scope.run(() => useTheme(id))!;
  return { id, scope, theme, sheets };
}

afterEach(() => {
  document.documentElement.removeAttribute("data-script");
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.style.colorScheme = "";
  Object.defineProperty(document, "adoptedStyleSheets", { configurable: true, value: [] });
  delete window.__AOIDOS_THEME_BOOTSTRAP__;
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

it("applies only validated current-script constants and removes its sheet on disposal", async () => {
  const { scope, theme, sheets } = setup("mistbell", skinWithAccent());
  await flush();
  expect(document.documentElement.dataset.script).toBe("mistbell");
  expect(sheets[0]).toContain(':root[data-script="mistbell"]{--accent:#abcdef12}');
  expect(theme.warningCount.value).toBe(0);
  expect(document.adoptedStyleSheets).toHaveLength(1);
  scope.stop();
  expect(document.adoptedStyleSheets).toEqual([]);
  expect(document.documentElement.dataset.script).toBeUndefined();
});

it("coalesces quick preference changes and applies only the confirmed final value", async () => {
  const first = deferred<{ version: 1; theme: ThemeId }>();
  vi.mocked(setThemePreference).mockReturnValueOnce(first.promise);
  const { theme } = setup();
  await flush();
  const one = theme.setTheme("light");
  const two = theme.setTheme("dark");
  expect(theme.theme.value).toBe("dark");
  first.resolve({ version: 1, theme: "light" });
  await Promise.all([one, two]);
  expect(setThemePreference).toHaveBeenNthCalledWith(1, "light");
  expect(setThemePreference).toHaveBeenNthCalledWith(2, "dark");
  expect(theme.theme.value).toBe("dark");
  expect(theme.saving.value).toBe(false);
});

it("restores the last committed preference when the newest coalesced save fails", async () => {
  const first = deferred<{ version: 1; theme: ThemeId }>();
  const second = deferred<{ version: 1; theme: ThemeId }>();
  vi.mocked(setThemePreference)
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise);
  const { theme } = setup("other");
  await flush();
  const saveA = theme.setTheme("light");
  const saveB = theme.setTheme("light-purple");
  first.resolve({ version: 1, theme: "light" });
  await flush();
  vi.mocked(getThemePreference).mockResolvedValueOnce({ version: 1, theme: "light" });
  second.reject(new Error("private storage error"));
  await Promise.all([saveA, saveB]);

  expect(theme.theme.value).toBe("light");
  expect(document.documentElement.dataset.theme).toBe("light");
  expect(theme.themeError.value).toContain("主题保存失败");
});

it("reconciles a rejected save with the preference persisted by the backend", async () => {
  const { theme } = setup();
  await flush();
  vi.mocked(setThemePreference).mockRejectedValueOnce(new Error("lost response"));
  vi.mocked(getThemePreference).mockResolvedValueOnce({ version: 1, theme: "light" });

  await theme.setTheme("light");

  expect(theme.theme.value).toBe("light");
  expect(document.documentElement.dataset.theme).toBe("light");
  expect(theme.themeError.value).toContain("主题保存失败");
});

it("continues with the newest preference when an earlier save fails", async () => {
  const first = deferred<{ version: 1; theme: ThemeId }>();
  const second = deferred<{ version: 1; theme: ThemeId }>();
  vi.mocked(setThemePreference)
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise);
  const { theme } = setup("other");
  await flush();
  const saveA = theme.setTheme("light");
  const saveB = theme.setTheme("light-purple");
  first.reject(new Error("private transient error"));
  await flush();
  second.resolve({ version: 1, theme: "light-purple" });
  await Promise.all([saveA, saveB]);

  expect(theme.theme.value).toBe("light-purple");
  expect(theme.themeError.value).toBe("");
});

it("keeps the last confirmed theme when save recovery cannot read a valid preference", async () => {
  const { scope, theme } = setup("other");
  await flush();
  await theme.setTheme("light");
  vi.mocked(setThemePreference).mockRejectedValue(new Error("save rejected"));
  vi.mocked(getThemePreference).mockRejectedValueOnce(new Error("read rejected"));
  await theme.setTheme("light-purple");
  expect(theme.theme.value).toBe("light");
  vi.mocked(getThemePreference).mockResolvedValueOnce({ version: 2 as 1, theme: "dark" });
  await theme.setTheme("light-purple");
  expect(theme.theme.value).toBe("light");
  expect(theme.themeError.value).toContain("主题保存失败");
  vi.mocked(setThemePreference).mockImplementation(async (next) => ({ version: 1, theme: next }));
  scope.stop();
});

it("falls back when the preference recovered after a rejected save has been retired", async () => {
  const { scope, theme } = setup("other");
  await flush();
  vi.mocked(setThemePreference).mockRejectedValueOnce(new Error("save rejected"));
  vi.mocked(getThemePreference).mockResolvedValueOnce({ version: 1, theme: "retired" });
  await theme.setTheme("light");
  expect(theme.theme.value).toBe("dark");
  expect(document.documentElement.style.colorScheme).toBe("dark");
  expect(theme.themeError.value).toContain("所选主题当前不可用");
  scope.stop();
});

it("preserves a recovered preference while the theme catalog is unavailable", async () => {
  const { scope, theme } = setup("other", emptySkin(), () => {
    vi.mocked(getThemes).mockResolvedValueOnce([]);
  });
  await flush();
  vi.mocked(setThemePreference).mockRejectedValueOnce(new Error("save rejected"));
  vi.mocked(getThemePreference).mockResolvedValueOnce({ version: 1, theme: "light" });
  await theme.setTheme("light");
  expect(theme.theme.value).toBe("light");
  expect(theme.themeError.value).toContain("主题保存失败");
  scope.stop();
});

it("does not apply save recovery after disposal or over a newer queued choice", async () => {
  const { scope, theme } = setup("other");
  await flush();
  const recovered = deferred<{ version: 1; theme: ThemeId }>();
  vi.mocked(setThemePreference).mockRejectedValueOnce(new Error("save rejected"));
  vi.mocked(getThemePreference).mockReturnValueOnce(recovered.promise);
  const old = theme.setTheme("light");
  await flush();
  const newest = theme.setTheme("light-purple");
  recovered.resolve({ version: 1, theme: "light" });
  await Promise.all([old, newest]);
  expect(theme.theme.value).toBe("light-purple");
  expect(theme.themeError.value).toBe("");

  const disposedRecovery = deferred<{ version: 1; theme: ThemeId }>();
  vi.mocked(setThemePreference).mockRejectedValueOnce(new Error("save rejected"));
  vi.mocked(getThemePreference).mockReturnValueOnce(disposedRecovery.promise);
  const disposed = theme.setTheme("light");
  await flush();
  scope.stop();
  disposedRecovery.resolve({ version: 1, theme: "light" });
  await disposed;
  expect(theme.theme.value).toBe("light-purple");
  expect(theme.themeError.value).toBe("");
});

it("keeps the confirmed preference and reports a failed save", async () => {
  const { theme } = setup();
  await flush();
  vi.mocked(setThemePreference).mockRejectedValueOnce(new Error("private failure"));
  await theme.setTheme("light");
  expect(theme.theme.value).toBe("dark");
  expect(theme.themeError.value).toContain("主题保存失败");
});

it("applies a recovered startup preference to the document theme and color scheme", async () => {
  vi.mocked(getThemePreference).mockResolvedValueOnce({ version: 1, theme: "light" });
  const { theme } = setup();
  await flush();
  expect(theme.theme.value).toBe("light");
  expect(document.documentElement.dataset.theme).toBe("light");
  expect(document.documentElement.style.colorScheme).toBe("light");
});

it("does not let a delayed startup read overwrite a newer saved choice", async () => {
  const initial = deferred<{ version: 1; theme: ThemeId }>();
  vi.mocked(getThemePreference).mockReturnValueOnce(initial.promise);
  const { theme } = setup();
  await flush();
  await theme.setTheme("light-purple");
  initial.resolve({ version: 1, theme: "dark" });
  await flush();
  expect(theme.theme.value).toBe("light-purple");
  expect(document.documentElement.dataset.theme).toBe("light-purple");
  expect(document.documentElement.style.colorScheme).toBe("light");
});

it("keeps a removed but valid preference and temporarily displays the safe default", async () => {
  vi.mocked(getThemePreference).mockResolvedValueOnce({ version: 1, theme: "retired-theme" });
  const { theme } = setup();
  await flush();
  expect(theme.theme.value).toBe("dark");
  expect(document.documentElement.dataset.theme).toBe("dark");
  expect(theme.themeError.value).toContain("当前不可用");
});

it("keeps the bootstrap color scheme when theme discovery returns no entries", async () => {
  vi.mocked(getThemes).mockResolvedValueOnce([]);
  const preference = deferred<{ version: 1; theme: ThemeId }>();
  document.documentElement.dataset.theme = "dark";
  const { theme } = setup("mistbell", emptySkin(), () => {
    vi.mocked(getThemePreference).mockReturnValueOnce(preference.promise);
  });
  await flush();
  expect(theme.theme.value).toBe("dark");
  expect(document.documentElement.dataset.theme).toBe("dark");
  expect(document.documentElement.style.colorScheme).toBe("");
});

it("ignores stale skin loads when scripts change and reloads the new registered script", async () => {
  const old = deferred<Skin>();
  const { id, scope, theme } = setup("other", emptySkin(), () => {
    vi.mocked(loadSkin).mockReturnValueOnce(old.promise).mockResolvedValueOnce(emptySkin());
  });
  id.value = "mistbell";
  await nextTick();
  id.value = "other";
  await nextTick();
  id.value = "mistbell";
  await flush();
  old.resolve(skinWithAccent());
  await flush();
  expect(loadSkin).toHaveBeenCalledTimes(2);
  expect(theme.skinError.value).toBe("");
  scope.stop();
});

it("serializes skin loads and keeps only the latest script target while one is pending", async () => {
  const first = deferred<Skin>();
  const second = deferred<Skin>();
  let active = 0;
  let maxActive = 0;
  vi.mocked(loadSkin)
    .mockImplementationOnce(async () => {
      active += 1;
      maxActive = Math.max(maxActive, active);
      const result = await first.promise;
      active -= 1;
      return result;
    })
    .mockImplementationOnce(async () => {
      active += 1;
      maxActive = Math.max(maxActive, active);
      const result = await second.promise;
      active -= 1;
      return result;
    });
  const { id, scope } = setup("mistbell");
  await flush();
  expect(loadSkin).toHaveBeenCalledTimes(1);

  id.value = "other";
  await nextTick();
  id.value = "mistbell";
  await nextTick();
  expect(loadSkin).toHaveBeenCalledTimes(1);

  first.resolve(emptySkin());
  await flush();
  expect(loadSkin).toHaveBeenCalledTimes(2);
  second.resolve(skinWithAccent());
  await flush();

  expect(maxActive).toBe(1);
  scope.stop();
});

it("allows disabling and restoring the cached skin without re-enabling on theme changes", async () => {
  const { scope, theme } = setup("mistbell", skinWithAccent());
  await flush();
  expect(document.adoptedStyleSheets).toHaveLength(1);
  theme.toggleSkin();
  expect(theme.disabled.value).toBe(true);
  expect(document.adoptedStyleSheets).toHaveLength(0);
  await theme.setTheme("light");
  expect(document.documentElement.dataset.script).toBeUndefined();
  theme.toggleSkin();
  expect(theme.disabled.value).toBe(false);
  expect(document.adoptedStyleSheets).toHaveLength(1);
  scope.stop();
});

it("rejects unsafe CSS names and values, and falls back when CSSOM is unavailable", async () => {
  const invalid = {
    ...emptySkin(),
    tokens: { "--accent};body{color:red": "#fff;display:none" },
  };
  const first = setup("mistbell", invalid);
  await flush();
  expect(first.theme.skinError.value).toContain("不支持");
  first.scope.stop();

  const second = setup("mistbell", skinWithAccent());
  vi.stubGlobal("CSSStyleSheet", undefined);
  await flush();
  expect(second.theme.skinError.value).toContain("不支持剧本皮肤");
  second.scope.stop();
});

it("reports a stylesheet application failure without leaking the candidate sheet", async () => {
  const { scope, theme } = setup("mistbell", skinWithAccent(), () => {
    vi.stubGlobal(
      "CSSStyleSheet",
      class {
        replaceSync(): never {
          throw new Error("invalid stylesheet");
        }
      },
    );
  });
  await flush();
  expect(theme.skinError.value).toContain("无法应用");
  expect(document.adoptedStyleSheets).toEqual([]);
  expect(document.documentElement.dataset.script).toBeUndefined();
  scope.stop();
});

it("retries a skin load when it is enabled again after the initial load failed", async () => {
  const { scope, theme } = setup("mistbell", skinWithAccent(), () => {
    vi.mocked(loadSkin)
      .mockRejectedValueOnce(new Error("temporary read failure"))
      .mockResolvedValueOnce(skinWithAccent());
  });
  await flush();
  expect(theme.skinError.value).toContain("读取失败");

  theme.toggleSkin();
  theme.toggleSkin();
  await flush();

  expect(loadSkin).toHaveBeenCalledTimes(2);
  expect(theme.skinError.value).toBe("");
  expect(document.adoptedStyleSheets).toHaveLength(1);
  scope.stop();
});

it("reports skin and preference read failures and bootstrap recovery", async () => {
  const { scope, theme } = setup("mistbell", emptySkin(), () => {
    vi.mocked(loadSkin).mockRejectedValueOnce(new Error("private path"));
    vi.mocked(getThemePreference).mockRejectedValueOnce(new Error("private db"));
    window.__AOIDOS_THEME_BOOTSTRAP__ = {
      version: 1,
      theme: "dark",
      fallbackReason: "storageUnavailable",
    };
  });
  expect(theme.themeError.value).toContain("已使用深色默认主题");
  await flush();
  expect(theme.themeError.value).toContain("偏好不可用");
  expect(theme.skinError.value).toContain("读取失败");
  scope.stop();
});

it("keeps theme save diagnostics independent from later skin success", async () => {
  const { id, theme } = setup("other");
  await flush();
  vi.mocked(setThemePreference).mockRejectedValueOnce(new Error("private storage error"));
  await theme.setTheme("light");
  expect(theme.themeError.value).toContain("主题保存失败");

  id.value = "mistbell";
  await nextTick();
  await flush();
  expect(theme.themeError.value).toContain("主题保存失败");
  expect(theme.skinError.value).toBe("");
});

it("uses the default theme and does not update state after disposal", async () => {
  window.__AOIDOS_THEME_BOOTSTRAP__ = { theme: "invalid" };
  const preference = deferred<{ version: 1; theme: ThemeId }>();
  vi.mocked(getThemePreference).mockReturnValueOnce(preference.promise);
  const { scope, theme } = setup("other");
  expect(theme.theme.value).toBe("dark");
  scope.stop();
  theme.toggleSkin();
  preference.resolve({ version: 1, theme: "light" });
  await flush();
  expect(theme.theme.value).toBe("dark");
  await theme.setTheme("light");
  expect(setThemePreference).not.toHaveBeenCalled();
});

it("stops an in-flight preference update when its scope is disposed", async () => {
  const save = deferred<{ version: 1; theme: ThemeId }>();
  vi.mocked(setThemePreference).mockReturnValueOnce(save.promise);
  const { scope, theme } = setup();
  await flush();
  const request = theme.setTheme("light");
  scope.stop();
  save.resolve({ version: 1, theme: "light" });
  await request;
  expect(theme.theme.value).toBe("dark");
  expect(theme.saving.value).toBe(true);
});

it("applies the confirmed theme before the catalog arrives, then updates its color scheme", async () => {
  const catalog = deferred<Awaited<ReturnType<typeof getThemes>>>();
  const { scope, theme } = setup("other", emptySkin(), () => {
    vi.mocked(getThemes).mockReturnValueOnce(catalog.promise);
  });
  await flush();
  await theme.setTheme("light");
  expect(document.documentElement.dataset.theme).toBe("light");
  expect(document.documentElement.style.colorScheme).toBe("");

  catalog.resolve([{ id: "light", name: "浅色", colorScheme: "light" }]);
  await flush();
  expect(document.documentElement.style.colorScheme).toBe("light");
  scope.stop();
});

it("clears a bootstrap fallback message when the saved preference is recovered", async () => {
  const { scope, theme } = setup("other", emptySkin(), () => {
    window.__AOIDOS_THEME_BOOTSTRAP__ = {
      version: 1,
      theme: "dark",
      fallbackReason: "storageUnavailable",
    };
  });
  expect(theme.themeError.value).toContain("已使用深色默认主题");
  await flush();
  expect(theme.themeError.value).toBe("");
  scope.stop();
});

it("accepts a valid bootstrap theme and leaves color scheme unchanged without its descriptor", async () => {
  const { scope, theme } = setup("other", emptySkin(), () => {
    window.__AOIDOS_THEME_BOOTSTRAP__ = { theme: "light-purple" };
    vi.mocked(getThemePreference).mockResolvedValue({ version: 1, theme: "light-purple" });
    vi.mocked(getThemes).mockResolvedValue([]);
  });
  await flush();
  expect(theme.theme.value).toBe("light-purple");
  expect(document.documentElement.style.colorScheme).toBe("");
  scope.stop();
});

it("does not replace a bootstrap warning when asynchronous reads fail", async () => {
  const { scope, theme } = setup("other", emptySkin(), () => {
    window.__AOIDOS_THEME_BOOTSTRAP__ = {
      version: 1,
      theme: "dark",
      fallbackReason: "storageUnavailable",
    };
    vi.mocked(getThemePreference).mockRejectedValueOnce(new Error("storage unavailable"));
    vi.mocked(getThemes).mockRejectedValueOnce(new Error("catalog unavailable"));
  });
  await flush();
  expect(theme.themeError.value).toContain("偏好不可用");
  scope.stop();
});

it("ignores failures from obsolete skin requests", async () => {
  const old = deferred<Skin>();
  const { id, scope, theme } = setup("mistbell", emptySkin(), () => {
    vi.mocked(loadSkin).mockReturnValueOnce(old.promise).mockResolvedValueOnce(emptySkin());
  });
  id.value = "other";
  await nextTick();
  old.reject(new Error("obsolete read failure"));
  await flush();
  expect(theme.skinError.value).toBe("");
  scope.stop();
});
