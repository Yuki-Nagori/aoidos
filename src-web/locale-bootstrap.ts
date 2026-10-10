import type { LocaleBootstrap } from "./api/locale";

const value: LocaleBootstrap | undefined = window.__AOIDOS_LOCALE_BOOTSTRAP__;
const locale =
  value?.version === 1 && (value.locale === "zh-Hans" || value.locale === "en")
    ? value.locale
    : "en";

document.documentElement.lang = locale;
if (
  value?.fallbackReason === "storageUnavailable" ||
  value?.fallbackReason === "invalidPreference"
) {
  document.documentElement.dataset.localeFallback = value.fallbackReason;
} else {
  delete document.documentElement.dataset.localeFallback;
}
