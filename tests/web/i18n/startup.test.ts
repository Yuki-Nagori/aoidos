import { beforeEach, describe, expect, it, vi } from "vitest";
import { getLocalePreference, type LocaleResult } from "../../../src-web/api/locale";
import { i18n } from "../../../src-web/i18n";
import { hydrateLocale } from "../../../src-web/i18n/startup";

vi.mock("../../../src-web/api/locale", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../src-web/api/locale")>();
  return { ...actual, getLocalePreference: vi.fn() };
});

const chinese: LocaleResult = {
  preference: { version: 1, locale: "zh-Hans" },
  resolvedLocale: "zh-Hans",
  nativeStatus: "applied",
};

describe("hydrateLocale", () => {
  beforeEach(() => {
    vi.mocked(getLocalePreference).mockReset();
    i18n.global.locale.value = "en";
    document.documentElement.lang = "en";
    delete document.documentElement.dataset.localeFallback;
  });

  it("applies the latest persisted locale before returning mount data", async () => {
    vi.mocked(getLocalePreference).mockResolvedValue(chinese);

    await expect(hydrateLocale()).resolves.toEqual({ result: chinese });
    expect(i18n.global.t("game.intro")).toContain("选择剧本与模型后开始");
    expect(document.documentElement.lang).toBe("zh-Hans");
    expect(document.documentElement.dataset.localeFallback).toBeUndefined();
  });

  it("preserves the bootstrap locale when persisted preference cannot be read", async () => {
    window.__AOIDOS_LOCALE_BOOTSTRAP__ = {
      version: 1,
      locale: "zh-Hans",
      fallbackReason: "storageUnavailable",
    };
    document.documentElement.lang = "zh-Hans";
    document.documentElement.dataset.localeFallback = "storageUnavailable";
    i18n.global.locale.value = "zh-Hans";
    vi.mocked(getLocalePreference).mockRejectedValue(new Error("private storage failure"));

    await expect(hydrateLocale()).resolves.toEqual({});
    expect(i18n.global.locale.value).toBe("zh-Hans");
    expect(document.documentElement.lang).toBe("zh-Hans");
    expect(document.documentElement.dataset.localeFallback).toBe("storageUnavailable");
  });
});
