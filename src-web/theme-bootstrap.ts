import type { ThemeBootstrap } from "./api/theme";

/** 在 Vue 与应用 CSS 启动前设置根主题，并报告 Rust bootstrap 的安全回退。 */
const value: ThemeBootstrap | undefined = window.__AOIDOS_THEME_BOOTSTRAP__;
const theme =
  value?.version === 1 &&
  typeof value.theme === "string" &&
  /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/.test(value.theme)
    ? value.theme
    : "dark";
document.documentElement.dataset.theme = theme;
document.documentElement.style.colorScheme =
  value?.colorScheme === "light" || value?.colorScheme === "dark" ? value.colorScheme : "dark";
