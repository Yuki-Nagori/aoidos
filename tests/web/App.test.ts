import { mount } from "@vue/test-utils";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "../../src-web/App.vue";
import { localeStartupKey } from "../../src-web/api/locale";
import { i18n } from "../../src-web/i18n";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
describe("App", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    i18n.global.locale.value = "en";
    document.documentElement.lang = "en";
    delete document.documentElement.dataset.localeFallback;
  });
  it("展示正式入口，初始化不打开会话或发起生成", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "llm_list_profiles") return { items: [] };
      if (command === "engine_list_scripts")
        return [
          {
            scriptId: "mistbell",
            title: "雾钟地窖",
            displayNames: { en: "Mistbell Cellar", "zh-Hans": "雾钟地窖" },
            attributions: "CC BY 4.0",
          },
        ];
      if (command === "theme_get_preference") return { version: 1, theme: "dark" };
      if (command === "store_get_ui_preferences")
        return { version: 1, panelPinned: false, diceMode: "manual" };
      if (command === "theme_list")
        return [
          { id: "dark", name: "深色", colorScheme: "dark" },
          { id: "light", name: "浅色", colorScheme: "light" },
          { id: "light-purple", name: "浅紫", colorScheme: "light" },
        ];
      if (command === "theme_skin_load")
        return {
          scriptId: "mistbell",
          status: "missing",
          tokens: {},
          warnings: [],
          warningsTruncated: false,
        };
      if (command === "locale_get_preference")
        return {
          preference: { version: 1, locale: "en" },
          resolvedLocale: "en",
          nativeStatus: "applied",
        };
      if (command === "locale_set_preference")
        return {
          preference: { version: 1, locale: "zh-Hans" },
          resolvedLocale: "zh-Hans",
          nativeStatus: "applied",
        };
      throw new Error("unexpected invocation");
    });
    const wrapper = mount(App, { global: { plugins: [i18n] } });
    await vi.waitFor(() => expect(wrapper.text()).toContain("Mistbell Cellar"));
    expect(wrapper.text()).toContain("system dialog");
    const theme = wrapper.get('select[aria-label="Theme"]');
    expect(theme.findAll("option").map((option) => option.text())).toEqual([
      "Dark",
      "Light",
      "Light Purple",
    ]);
    const script = wrapper
      .findAll("label")
      .find((label) => label.text().includes("Scenario"))!
      .get("select");
    expect(script.findAll("option").map((option) => option.text())).toEqual(["Mistbell Cellar"]);
    const model = wrapper.get('select[aria-label="Model"]');
    expect(model.findAll("option").map((option) => option.text())).toEqual([
      "DeepSeek V4.1 Flash",
      "DeepSeek V4 Pro",
    ]);
    expect((model.element as HTMLSelectElement).value).toBe("deepseek-flash");
    await wrapper.get('select[aria-label="Interface language"]').setValue("zh-Hans");
    await vi.waitFor(() => {
      expect(theme.findAll("option").map((option) => option.text())).toEqual([
        "深色",
        "浅色",
        "浅紫",
      ]);
      expect(script.findAll("option").map((option) => option.text())).toEqual(["雾钟地窖"]);
    });
    expect(wrapper.find("input").exists()).toBe(false);
    expect(
      vi
        .mocked(invoke)
        .mock.calls.map(([command]) => command)
        .sort(),
    ).toEqual([
      "engine_list_scripts",
      "llm_list_profiles",
      "locale_get_preference",
      "locale_set_preference",
      "store_get_ui_preferences",
      "theme_get_preference",
      "theme_list",
      "theme_skin_load",
    ]);
    wrapper.unmount();
  });
  it("浏览器缺 IPC 明确展示恢复错误，不伪造本地生成结果", async () => {
    vi.mocked(invoke).mockRejectedValue({ code: "app.not-ready", message: "桌面 IPC 不可用" });
    const wrapper = mount(App, { global: { plugins: [i18n] } });
    await vi.waitFor(() =>
      expect(wrapper.get('[role="alert"]').text()).toContain("app is still starting"),
    );
    wrapper.unmount();
  });

  it("mounts the latest saved locale immediately after startup hydration", () => {
    const result = {
      preference: { version: 1 as const, locale: "zh-Hans" as const },
      resolvedLocale: "zh-Hans" as const,
      nativeStatus: "applied" as const,
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "llm_list_profiles") return { items: [] };
      if (command === "engine_list_scripts") return [];
      if (command === "theme_get_preference") return { version: 1, theme: "dark" };
      if (command === "store_get_ui_preferences")
        return { version: 1, panelPinned: false, diceMode: "manual" };
      if (command === "theme_list") return [];
      if (command === "theme_skin_load")
        return {
          scriptId: "",
          status: "missing",
          tokens: {},
          warnings: [],
          warningsTruncated: false,
        };
      throw new Error(`unexpected invocation: ${String(command)}`);
    });
    const wrapper = mount(App, {
      global: {
        plugins: [i18n],
        provide: { [localeStartupKey as symbol]: { result } },
      },
    });
    expect(wrapper.text()).toContain("选择剧本与模型后开始");
    expect(document.documentElement.lang).toBe("zh-Hans");
    expect(invoke).not.toHaveBeenCalledWith("locale_get_preference");
    wrapper.unmount();
  });
});
