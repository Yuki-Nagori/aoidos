import { inject, onScopeDispose, ref } from "vue";
import {
  getLocalePreference,
  localeStartupKey,
  setLocalePreference,
  type Locale,
  type LocaleChoice,
  type LocaleResult,
} from "../api/locale";
import { applyLocale } from "../i18n";

/** 语言切换只确认 Rust 已保存的结果；并发选择串行化且只应用最新意图。 */
export function useLocale() {
  const choice = ref<LocaleChoice>("system");
  const initialLocale = window.__AOIDOS_LOCALE_BOOTSTRAP__?.locale;
  const resolved = ref<Locale>(
    initialLocale === "zh-Hans" || initialLocale === "en" ? initialLocale : "en",
  );
  const ready = ref(false);
  const saving = ref(false);
  const notice = ref<"pending" | "saveFailed" | "loadFailed" | "fallback" | null>(null);
  const fallback = ref(Boolean(window.__AOIDOS_LOCALE_BOOTSTRAP__?.fallbackReason));
  applyLocale(resolved.value);
  const startup = inject(localeStartupKey, null);
  let revision = 0;
  let queue = Promise.resolve();
  let active = true;

  function apply(result: LocaleResult): void {
    choice.value = result.preference.locale;
    resolved.value = result.resolvedLocale;
    applyLocale(result.resolvedLocale);
    fallback.value = false;
    notice.value = result.nativeStatus === "pending" ? "pending" : null;
    ready.value = true;
  }

  if (startup) {
    if (startup.result) apply(startup.result);
    else {
      ready.value = true;
      notice.value = "loadFailed";
    }
  } else {
    const currentRevision = revision;
    void getLocalePreference().then(
      (result) => {
        if (active && revision === currentRevision) apply(result);
      },
      () => {
        if (active && revision === currentRevision) {
          ready.value = true;
          notice.value = "loadFailed";
        }
      },
    );
  }

  function setChoice(locale: LocaleChoice): Promise<void> {
    const requestRevision = ++revision;
    saving.value = true;
    queue = queue.then(async () => {
      if (!active || requestRevision !== revision) return;
      try {
        const result = await setLocalePreference({ version: 1, locale });
        if (active && requestRevision === revision) apply(result);
      } catch {
        if (!active || requestRevision !== revision) return;
        try {
          // IPC 失败可能发生在事务已提交之后；回读确认，不重复写入猜测状态。
          const result = await getLocalePreference();
          if (active && requestRevision === revision) {
            apply(result);
            notice.value = "saveFailed";
          }
        } catch {
          if (active && requestRevision === revision) notice.value = "saveFailed";
        }
      } finally {
        if (active && requestRevision === revision) saving.value = false;
      }
    });
    return queue;
  }

  onScopeDispose(() => {
    active = false;
  });

  return { choice, resolved, ready, saving, notice, fallback, setChoice };
}
