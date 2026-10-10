import { createI18n } from "vue-i18n";
import en from "../../locales/en.json";
import zhHans from "../../locales/zh-Hans.json";
import type { Locale } from "../api/locale";

export type MessageSchema = typeof en;

declare module "vue-i18n" {
  // 英文资源作为键与占位参数的编译期 schema。
  export interface DefineLocaleMessage extends MessageSchema {}
}

const bootstrap = window.__AOIDOS_LOCALE_BOOTSTRAP__;
const initialLocale: Locale =
  bootstrap?.version === 1 && (bootstrap.locale === "zh-Hans" || bootstrap.locale === "en")
    ? bootstrap.locale
    : "en";

export const i18n = createI18n<[MessageSchema], Locale, false>({
  legacy: false,
  locale: initialLocale,
  fallbackLocale: "en",
  messages: { "zh-Hans": zhHans, en },
});

export function applyLocale(locale: Locale): void {
  i18n.global.locale.value = locale;
  document.documentElement.lang = locale;
  delete document.documentElement.dataset.localeFallback;
}
