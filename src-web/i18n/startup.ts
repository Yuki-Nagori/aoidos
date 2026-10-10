import { getLocalePreference, type LocaleStartup } from "../api/locale";
import { applyLocale } from "./index";

/** 首次挂载前对齐持久偏好，失败时保留 HTML bootstrap 的语言。 */
export async function hydrateLocale(): Promise<LocaleStartup> {
  try {
    const result = await getLocalePreference();
    applyLocale(result.resolvedLocale);
    return { result };
  } catch {
    return {};
  }
}
