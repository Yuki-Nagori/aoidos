import "./app.css";
import { createApp } from "vue";
import App from "./App.vue";
import { localeStartupKey } from "./api/locale";
import { i18n } from "./i18n";
import { hydrateLocale } from "./i18n/startup";

async function mountApp(): Promise<void> {
  const startup = await hydrateLocale();
  createApp(App).provide(localeStartupKey, startup).use(i18n).mount("#app");
}

void mountApp();
