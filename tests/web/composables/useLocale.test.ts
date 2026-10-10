import { createApp, effectScope } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  getLocalePreference,
  localeStartupKey,
  setLocalePreference,
  type LocaleStartup,
  type LocaleResult,
} from "../../../src-web/api/locale";
import { useLocale } from "../../../src-web/composables/useLocale";
import { i18n } from "../../../src-web/i18n";

vi.mock("../../../src-web/api/locale", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../src-web/api/locale")>();
  return {
    ...actual,
    getLocalePreference: vi.fn(),
    setLocalePreference: vi.fn(),
  };
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

const english: LocaleResult = {
  preference: { version: 1, locale: "en" },
  resolvedLocale: "en",
  nativeStatus: "applied",
};
const chinese: LocaleResult = {
  preference: { version: 1, locale: "zh-Hans" },
  resolvedLocale: "zh-Hans",
  nativeStatus: "applied",
};
const pending: LocaleResult = { ...chinese, nativeStatus: "pending" };

function setup(startup?: LocaleStartup) {
  const scope = effectScope();
  const app = createApp({});
  if (startup) app.provide(localeStartupKey, startup);
  let locale!: ReturnType<typeof useLocale>;
  app.runWithContext(() => {
    scope.run(() => {
      locale = useLocale();
    });
  });
  return { scope, locale };
}

async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

describe("useLocale", () => {
  beforeEach(() => {
    vi.mocked(getLocalePreference).mockReset().mockResolvedValue(english);
    vi.mocked(setLocalePreference)
      .mockReset()
      .mockImplementation(async (preference) => ({
        ...english,
        preference,
        resolvedLocale: preference.locale === "zh-Hans" ? "zh-Hans" : "en",
      }));
    i18n.global.locale.value = "en";
    document.documentElement.lang = "en";
    delete document.documentElement.dataset.localeFallback;
    delete window.__AOIDOS_LOCALE_BOOTSTRAP__;
  });

  it("applies the saved locale and clears a temporary bootstrap fallback", async () => {
    window.__AOIDOS_LOCALE_BOOTSTRAP__ = {
      version: 1,
      locale: "en",
      fallbackReason: "storageUnavailable",
    };
    vi.mocked(getLocalePreference).mockResolvedValueOnce(chinese);
    const { locale, scope } = setup();
    expect(locale.fallback.value).toBe(true);
    expect(i18n.global.t("game.intro")).toContain("Choose a scenario");
    await flush();
    expect(locale.choice.value).toBe("zh-Hans");
    expect(locale.resolved.value).toBe("zh-Hans");
    expect(locale.fallback.value).toBe(false);
    expect(i18n.global.locale.value).toBe("zh-Hans");
    expect(i18n.global.t("game.intro")).toContain("选择剧本与模型后开始");
    expect(document.documentElement.lang).toBe("zh-Hans");
    scope.stop();
  });

  it("keeps bootstrap locale and reports a read failure without persisting a fallback", async () => {
    window.__AOIDOS_LOCALE_BOOTSTRAP__ = { version: 1, locale: "zh-Hans" };
    vi.mocked(getLocalePreference).mockRejectedValueOnce(new Error("private storage error"));
    const { locale, scope } = setup();
    await flush();
    expect(locale.ready.value).toBe(true);
    expect(locale.resolved.value).toBe("zh-Hans");
    expect(locale.notice.value).toBe("loadFailed");
    expect(i18n.global.locale.value).toBe("zh-Hans");
    scope.stop();
  });

  it("reports a missing result from injected startup without retrying the IPC read", () => {
    window.__AOIDOS_LOCALE_BOOTSTRAP__ = { version: 1, locale: "zh-Hans" };
    const { locale, scope } = setup({});

    expect(locale.ready.value).toBe(true);
    expect(locale.notice.value).toBe("loadFailed");
    expect(locale.choice.value).toBe("system");
    expect(locale.resolved.value).toBe("zh-Hans");
    expect(getLocalePreference).not.toHaveBeenCalled();
    expect(i18n.global.locale.value).toBe("zh-Hans");
    scope.stop();
  });

  it("applies a confirmed injected startup result without a second IPC read", () => {
    const { locale, scope } = setup({ result: chinese });

    expect(locale.ready.value).toBe(true);
    expect(locale.choice.value).toBe("zh-Hans");
    expect(locale.resolved.value).toBe("zh-Hans");
    expect(getLocalePreference).not.toHaveBeenCalled();
    expect(i18n.global.locale.value).toBe("zh-Hans");
    scope.stop();
  });

  it("serializes rapid choices and applies only the last saved intent", async () => {
    const { locale, scope } = setup();
    await flush();
    const first = locale.setChoice("zh-Hans");
    const second = locale.setChoice("en");
    await Promise.all([first, second]);
    expect(setLocalePreference).toHaveBeenCalledTimes(1);
    expect(setLocalePreference).toHaveBeenCalledWith({ version: 1, locale: "en" });
    expect(locale.choice.value).toBe("en");
    expect(locale.saving.value).toBe(false);
    scope.stop();
  });

  it("reports a pending native update while applying the confirmed web locale", async () => {
    const { locale, scope } = setup();
    await flush();
    vi.mocked(setLocalePreference).mockResolvedValueOnce(pending);
    await locale.setChoice("zh-Hans");
    await flush();
    expect(locale.notice.value).toBe("pending");
    expect(i18n.global.locale.value).toBe("zh-Hans");
    scope.stop();
  });

  it("re-reads after a failed save to resolve an uncertain commit", async () => {
    const { locale, scope } = setup();
    await flush();
    vi.mocked(setLocalePreference).mockRejectedValueOnce(new Error("response lost"));
    vi.mocked(getLocalePreference).mockResolvedValueOnce(chinese);
    await locale.setChoice("zh-Hans");
    await flush();
    expect(locale.choice.value).toBe("zh-Hans");
    expect(locale.notice.value).toBe("saveFailed");
    expect(i18n.global.locale.value).toBe("zh-Hans");
    scope.stop();
  });

  it("keeps the last confirmed language when save and confirmation read both fail", async () => {
    const { locale, scope } = setup();
    await flush();
    vi.mocked(setLocalePreference).mockRejectedValueOnce(new Error("write failed"));
    vi.mocked(getLocalePreference).mockRejectedValueOnce(new Error("read failed"));
    await locale.setChoice("zh-Hans");
    await flush();
    expect(locale.choice.value).toBe("en");
    expect(locale.notice.value).toBe("saveFailed");
    expect(i18n.global.locale.value).toBe("en");
    scope.stop();
  });

  it("ignores a delayed startup response after a newer choice and after disposal", async () => {
    const initial = deferred<LocaleResult>();
    vi.mocked(getLocalePreference).mockReturnValueOnce(initial.promise);
    const { locale, scope } = setup();
    const update = locale.setChoice("zh-Hans");
    await update;
    await flush();
    initial.resolve(english);
    await flush();
    expect(locale.choice.value).toBe("zh-Hans");

    const late = deferred<LocaleResult>();
    vi.mocked(getLocalePreference).mockReturnValueOnce(late.promise);
    const second = setup();
    second.scope.stop();
    late.resolve(chinese);
    await flush();
    expect(second.locale.choice.value).toBe("system");
    scope.stop();
  });

  it("does not update refs after a pending save is disposed", async () => {
    const { locale, scope } = setup();
    await flush();
    const save = deferred<LocaleResult>();
    vi.mocked(setLocalePreference).mockReturnValueOnce(save.promise);
    const update = locale.setChoice("zh-Hans");
    await flush();
    scope.stop();
    save.resolve(chinese);
    await update;
    await flush();
    expect(locale.choice.value).toBe("en");
  });

  it.each(["superseded", "disposed"] as const)(
    "ignores a startup read failure when %s",
    async (state) => {
      const initial = deferred<LocaleResult>();
      vi.mocked(getLocalePreference).mockReturnValueOnce(initial.promise);
      const { locale, scope } = setup();
      if (state === "disposed") scope.stop();
      else await locale.setChoice("zh-Hans");
      initial.reject(new Error("late read failure"));
      await flush();
      expect(locale.notice.value).toBeNull();
      expect(locale.ready.value).toBe(state === "superseded");
      expect(locale.choice.value).toBe(state === "superseded" ? "zh-Hans" : "system");
      scope.stop();
    },
  );

  it("does not dispatch a queued save after disposal", async () => {
    const { locale, scope } = setup();
    await flush();
    const update = locale.setChoice("zh-Hans");
    scope.stop();
    await update;
    expect(setLocalePreference).not.toHaveBeenCalled();
    expect(locale.choice.value).toBe("en");
  });

  it("does not apply a superseded save response before dispatching the latest choice", async () => {
    const { locale, scope } = setup();
    await flush();
    const save = deferred<LocaleResult>();
    vi.mocked(setLocalePreference).mockReturnValueOnce(save.promise);
    const first = locale.setChoice("zh-Hans");
    await flush();
    const latest = locale.setChoice("en");
    save.resolve(pending);
    await first;
    expect(locale.choice.value).toBe("en");
    expect(locale.notice.value).toBeNull();
    expect(locale.saving.value).toBe(true);
    await latest;
    expect(locale.saving.value).toBe(false);
    expect(setLocalePreference).toHaveBeenCalledTimes(2);
    scope.stop();
  });

  it.each(["superseded", "disposed"] as const)(
    "does not recover a save failure when %s",
    async (state) => {
      const { locale, scope } = setup();
      await flush();
      const save = deferred<LocaleResult>();
      vi.mocked(setLocalePreference).mockReturnValueOnce(save.promise);
      const first = locale.setChoice("zh-Hans");
      await flush();
      const latest = state === "superseded" ? locale.setChoice("en") : undefined;
      if (state === "disposed") scope.stop();
      save.reject(new Error("late save failure"));
      await first;
      await latest;
      expect(getLocalePreference).toHaveBeenCalledTimes(1);
      expect(locale.notice.value).toBeNull();
      expect(locale.choice.value).toBe("en");
      scope.stop();
    },
  );

  it.each([
    ["superseded", "resolved"],
    ["disposed", "resolved"],
    ["superseded", "rejected"],
    ["disposed", "rejected"],
  ] as const)("ignores a %s confirmation read that is %s", async (state, outcome) => {
    const { locale, scope } = setup();
    await flush();
    const confirmation = deferred<LocaleResult>();
    vi.mocked(setLocalePreference).mockRejectedValueOnce(new Error("response lost"));
    vi.mocked(getLocalePreference).mockReturnValueOnce(confirmation.promise);
    const first = locale.setChoice("zh-Hans");
    await flush();
    expect(getLocalePreference).toHaveBeenCalledTimes(2);
    const latest = state === "superseded" ? locale.setChoice("en") : undefined;
    if (state === "disposed") scope.stop();
    if (outcome === "resolved") confirmation.resolve(pending);
    else confirmation.reject(new Error("late confirmation failure"));
    await first;
    await latest;
    expect(locale.choice.value).toBe("en");
    expect(locale.notice.value).toBeNull();
    expect(i18n.global.locale.value).toBe("en");
    scope.stop();
  });
});
