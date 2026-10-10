import { invoke } from "@tauri-apps/api/core";

declare global {
  interface Window {
    __AOIDOS_THEME_BOOTSTRAP__?: ThemeBootstrap;
  }
}

export interface ThemeBootstrap {
  version?: number;
  theme?: string;
  colorScheme?: "light" | "dark";
  fallbackReason?: "storageUnavailable" | "invalidPreference" | "themeUnavailable";
}

export type ThemeId = string;

export interface ThemeInfo {
  id: ThemeId;
  name: string;
  colorScheme: "light" | "dark";
}

export interface ThemePreference {
  version: 1;
  theme: ThemeId;
}

interface SkinWarning {
  code:
    | "unknown-token"
    | "protected-token"
    | "invalid-declaration"
    | "invalid-value"
    | "invalid-reference"
    | "cyclic-reference";
  token?: string;
  line?: number;
}

export interface Skin {
  scriptId: string;
  sourceHash?: string;
  status: "missing" | "valid";
  tokens: Record<string, string>;
  warnings: SkinWarning[];
  warningsTruncated: boolean;
}

export function getThemePreference(): Promise<ThemePreference> {
  return invoke("theme_get_preference");
}

export function getThemes(): Promise<ThemeInfo[]> {
  return invoke("theme_list");
}

export function setThemePreference(theme: ThemeId): Promise<ThemePreference> {
  return invoke("theme_set_preference", { theme });
}

export function loadSkin(scriptId: string): Promise<Skin> {
  return invoke("theme_skin_load", { scriptId });
}
