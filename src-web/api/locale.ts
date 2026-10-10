import { invoke } from "@tauri-apps/api/core";
import type { InjectionKey } from "vue";

export type Locale = "zh-Hans" | "en";
export type LocaleChoice = "system" | Locale;

export interface LocalePreference {
  version: 1;
  locale: LocaleChoice;
}

export interface LocaleResult {
  preference: LocalePreference;
  resolvedLocale: Locale;
  nativeStatus: "applied" | "pending";
}

export interface LocaleStartup {
  result?: LocaleResult;
}

export const localeStartupKey: InjectionKey<LocaleStartup> = Symbol("locale-startup");

export interface LocaleBootstrap {
  version: 1;
  locale: Locale;
  fallbackReason?: "storageUnavailable" | "invalidPreference";
}

declare global {
  interface Window {
    __AOIDOS_LOCALE_BOOTSTRAP__?: LocaleBootstrap;
  }
}

export function getLocalePreference(): Promise<LocaleResult> {
  return invoke("locale_get_preference");
}

export function setLocalePreference(preference: LocalePreference): Promise<LocaleResult> {
  return invoke("locale_set_preference", { preference });
}
