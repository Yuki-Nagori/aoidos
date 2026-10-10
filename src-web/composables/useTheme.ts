import { onScopeDispose, ref, watch, type Ref } from "vue";
import {
  getThemePreference,
  getThemes,
  loadSkin,
  setThemePreference,
  type Skin,
  type ThemeId,
  type ThemeInfo,
} from "../api/theme";

// Rust owns the semantic allowlist; the UI only prevents a returned name escaping CSS syntax.
const SAFE_TOKEN_NAME = /^--[a-z][a-z0-9-]*$/;
type ThemeError =
  | "storageUnavailable"
  | "invalidPreference"
  | "themeUnavailable"
  | "saveFailed"
  | "readFailed"
  | "catalogUnavailable";
type SkinError = "invalidValue" | "unsupportedWebView" | "applyFailed" | "readFailed";

/** 主题只在 Rust 持久化确认后切换；剧本样式表由本 composable 独占并负责清退。 */
export function useTheme(scriptId: Ref<string>) {
  const bootstrap = window.__AOIDOS_THEME_BOOTSTRAP__;
  const theme = ref<ThemeId>(
    bootstrap?.theme && /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/.test(bootstrap.theme)
      ? bootstrap.theme
      : "dark",
  );
  const themes = ref<ThemeInfo[]>([]);
  const saving = ref(false);
  const disabled = ref(false);
  const themeError = ref<ThemeError | "">("");
  const skinError = ref<SkinError | "">("");
  const fallbackMessages = {
    storageUnavailable: "storageUnavailable",
    invalidPreference: "invalidPreference",
    themeUnavailable: "themeUnavailable",
  } as const;
  const bootstrapFallback =
    bootstrap?.version === 1 && bootstrap.fallbackReason
      ? fallbackMessages[bootstrap.fallbackReason]
      : undefined;
  themeError.value = bootstrapFallback ?? "";
  const warningCount = ref(0);
  let confirmedTheme = theme.value;
  let desired: ThemeId | undefined;
  let saveTask: Promise<void> | undefined;
  let preferenceRevision = 0;
  let startupPreference: ThemeId | undefined;
  let generation = 0;
  let desiredSkin: { script: string; epoch: number } | undefined;
  let skinTask: Promise<void> | undefined;
  let cache: Skin | undefined;
  let disposed = false;
  let sheet: CSSStyleSheet | undefined;

  function applyConfirmedTheme(next: ThemeId): void {
    confirmedTheme = next;
    theme.value = next;
    document.documentElement.dataset.theme = next;
    const descriptor = themes.value.find((item) => item.id === next);
    if (descriptor) document.documentElement.style.colorScheme = descriptor.colorScheme;
  }

  function applyStartupPreference(next: ThemeId): void {
    if (!themes.value.length) return;
    if (!themes.value.some((item) => item.id === next)) {
      applyConfirmedTheme("dark");
      themeError.value = fallbackMessages.themeUnavailable;
      return;
    }
    applyConfirmedTheme(next);
    if (bootstrapFallback) themeError.value = "";
  }

  async function recoverThemeAfterSaveFailure(): Promise<void> {
    let confirmed = confirmedTheme;
    try {
      const saved = await getThemePreference();
      if (saved.version === 1) confirmed = saved.theme;
    } catch {
      // Fall back to the last response the application successfully confirmed.
    }
    if (disposed || desired) return;
    const available =
      themes.value.length === 0 || themes.value.some((descriptor) => descriptor.id === confirmed);
    applyConfirmedTheme(available ? confirmed : "dark");
    themeError.value = available ? "saveFailed" : fallbackMessages.themeUnavailable;
  }

  function clearSheet(): void {
    if (sheet)
      document.adoptedStyleSheets = document.adoptedStyleSheets.filter((item) => item !== sheet);
    sheet = undefined;
    delete document.documentElement.dataset.script;
  }

  function applySkin(skin: Skin): void {
    clearSheet();
    const entries = Object.entries(skin.tokens);
    if (!entries.length || skin.status !== "valid") return;
    if (entries.some(([name, value]) => !SAFE_TOKEN_NAME.test(name) || /[;{}\0]/.test(value))) {
      skinError.value = "invalidValue";
      return;
    }
    if (typeof CSSStyleSheet === "undefined" || !("adoptedStyleSheets" in document)) {
      skinError.value = "unsupportedWebView";
      return;
    }
    const candidate = new CSSStyleSheet();
    const rule = `:root[data-script="mistbell"]{${entries.map(([name, value]) => `${name}:${value}`).join(";")}}`;
    try {
      candidate.replaceSync(rule);
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, candidate];
      sheet = candidate;
      document.documentElement.dataset.script = "mistbell";
    } catch {
      skinError.value = "applyFailed";
    }
  }

  async function setTheme(next: ThemeId): Promise<void> {
    if (disposed) return;
    preferenceRevision += 1;
    themeError.value = "";
    desired = next;
    if (saveTask) return saveTask;
    saving.value = true;
    saveTask = (async () => {
      try {
        while (desired && !disposed) {
          const requested = desired;
          desired = undefined;
          try {
            const confirmed = await setThemePreference(requested);
            if (disposed) return;
            confirmedTheme = confirmed.theme;
            if (!desired) {
              applyConfirmedTheme(confirmed.theme);
            }
          } catch {
            if (!disposed && !desired) await recoverThemeAfterSaveFailure();
          }
        }
      } finally {
        saveTask = undefined;
        if (!disposed) saving.value = false;
      }
    })();
    return saveTask;
  }

  async function loadSkinFor(script: string, epoch: number): Promise<void> {
    if (script !== "mistbell") {
      skinError.value = "";
      return;
    }
    try {
      const result = await loadSkin(script);
      if (disposed || epoch !== generation || disabled.value) return;
      skinError.value = "";
      cache = result;
      warningCount.value = result.warnings.length + Number(result.warningsTruncated);
      applySkin(result);
    } catch {
      if (disposed || epoch !== generation) return;
      clearSheet();
      skinError.value = "readFailed";
    }
  }

  async function drainSkinQueue(): Promise<void> {
    try {
      while (desiredSkin && !disposed) {
        const target = desiredSkin;
        desiredSkin = undefined;
        await loadSkinFor(target.script, target.epoch);
      }
    } finally {
      skinTask = undefined;
    }
  }

  function requestSkinLoad(script: string, epoch: number): void {
    desiredSkin = { script, epoch };
    if (!skinTask) skinTask = drainSkinQueue();
  }

  const stopWatch = watch(
    scriptId,
    (script) => {
      generation += 1;
      disabled.value = false;
      cache = undefined;
      clearSheet();
      skinError.value = "";
      warningCount.value = 0;
      requestSkinLoad(script, generation);
    },
    { immediate: true },
  );

  const initialPreferenceRevision = preferenceRevision;
  void getThemePreference()
    .then((saved) => {
      if (!disposed && saved.version === 1 && preferenceRevision === initialPreferenceRevision) {
        startupPreference = saved.theme;
        applyStartupPreference(saved.theme);
      }
    })
    .catch(() => {
      if (!disposed && preferenceRevision === initialPreferenceRevision && !bootstrapFallback)
        themeError.value = "readFailed";
    });

  void getThemes()
    .then((catalog) => {
      if (disposed) return;
      themes.value = catalog;
      if (startupPreference !== undefined && preferenceRevision === initialPreferenceRevision) {
        applyStartupPreference(startupPreference);
      } else {
        const selected = catalog.find((item) => item.id === theme.value);
        if (selected) document.documentElement.style.colorScheme = selected.colorScheme;
      }
    })
    .catch(() => {
      if (!disposed && !bootstrapFallback) themeError.value = "catalogUnavailable";
    });

  function toggleSkin(): void {
    if (disposed) return;
    disabled.value = !disabled.value;
    generation += 1;
    if (disabled.value) {
      desiredSkin = undefined;
      clearSheet();
    } else if (cache) applySkin(cache);
    else requestSkinLoad(scriptId.value, generation);
  }

  onScopeDispose(() => {
    disposed = true;
    generation += 1;
    desiredSkin = undefined;
    stopWatch();
    clearSheet();
  });

  return {
    theme,
    themes,
    saving,
    disabled,
    themeError,
    skinError,
    warningCount,
    setTheme,
    toggleSkin,
  };
}
