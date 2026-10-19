import { mount } from "@vue/test-utils";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "../../../src-web/app/App.vue";
import { localeStartupKey } from "../../../src-web/api/locale";
import { i18n } from "../../../src-web/i18n/index";

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
      if (command === "llm_get_key_status") return { set: true, hint: "1234" };
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
      if (command === "store_set_ui_preferences")
        return { version: 1, panelPinned: false, diceMode: "auto" };
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
      if (command === "memory_get_app_config")
        return {
          configVersion: 1,
          algorithmVersion: 1,
          parameters: {
            algorithm: "exponential",
            baseStrength: 500_000,
            reinforcementGain: 150_000,
            reinforcementCap: 500_000,
            strengthCap: 1_000_000,
            halfLifeRounds: 100,
            repeatWindowRounds: 20,
            repeatLimit: 2,
            revivalCooldownRounds: 20,
            activeCapacity: 1_000,
            rateTableVersion: 1,
            decayRate: 993_092,
          },
        };
      if (command === "memory_get_script_config")
        return {
          scriptId: "mistbell",
          configVersion: 0,
          appConfigVersion: 1,
          algorithmVersion: 1,
          overrides: {},
          inherited: {
            algorithm: "exponential",
            baseStrength: 500_000,
            reinforcementGain: 150_000,
            reinforcementCap: 500_000,
            strengthCap: 1_000_000,
            halfLifeRounds: 100,
            repeatWindowRounds: 20,
            repeatLimit: 2,
            revivalCooldownRounds: 20,
            activeCapacity: 1_000,
            rateTableVersion: 1,
            decayRate: 993_092,
          },
          effective: {
            algorithm: "exponential",
            baseStrength: 500_000,
            reinforcementGain: 150_000,
            reinforcementCap: 500_000,
            strengthCap: 1_000_000,
            halfLifeRounds: 100,
            repeatWindowRounds: 20,
            repeatLimit: 2,
            revivalCooldownRounds: 20,
            activeCapacity: 1_000,
            rateTableVersion: 1,
            decayRate: 993_092,
          },
        };
      throw new Error("unexpected invocation");
    });
    const wrapper = mount(App, { attachTo: document.body, global: { plugins: [i18n] } });
    await vi.waitFor(() => expect(wrapper.text()).toContain("Mistbell Cellar"));
    expect(wrapper.find(".welcome-body > .hero").exists()).toBe(true);
    expect(wrapper.find(".welcome-body > .setup").exists()).toBe(true);
    expect(wrapper.findAll(".pillar")).toHaveLength(4);
    expect(wrapper.find(".welcome-start").attributes("disabled")).toBeUndefined();
    expect(wrapper.find(".welcome-new-cycle").exists()).toBe(true);
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).not.toContain(
      "engine_open_session",
    );
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).not.toContain(
      "memory_get_app_config",
    );
    await wrapper.get('button[aria-label="Settings"]').trigger("click");
    expect(wrapper.get(".app-shell").element.hasAttribute("inert")).toBe(true);
    expect(wrapper.text()).toContain("system dialog");
    expect(wrapper.get(".settings-key-card").text()).toContain("Key is set");
    expect(wrapper.get(".settings-form-grid").findAll("select")).toHaveLength(2);
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
    const language = wrapper.get('input[name="interface-language"][value="zh-Hans"]');
    await language.setValue();
    await vi.waitFor(() => {
      expect(theme.findAll("option").map((option) => option.text())).toEqual([
        "深色",
        "浅色",
        "浅紫",
      ]);
      expect(script.findAll("option").map((option) => option.text())).toEqual(["雾钟地窖"]);
    });
    const skinSwitch = wrapper.get('[role="switch"][aria-label="剧本皮肤"]');
    expect(skinSwitch.attributes("aria-checked")).toBe("true");
    await skinSwitch.trigger("click");
    expect(skinSwitch.attributes("aria-checked")).toBe("false");
    await wrapper.get('.settings-nav button[aria-controls="gameplay"]').trigger("click");
    await wrapper.get('input[name="dice-mode"][value="auto"]').setValue();
    await vi.waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("store_set_ui_preferences", {
        panelPinned: false,
        diceMode: "auto",
      });
    });
    await theme.trigger("keydown", { key: "Escape" });
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    await vi.waitFor(() =>
      expect(wrapper.get('button[aria-label="设置"]').element).toBe(document.activeElement),
    );
    await wrapper.get(".model-settings").trigger("click");
    expect(wrapper.find('.settings-nav button[aria-current="page"]').text()).toBe("模型与 API");
    await wrapper.get('button[aria-label="关闭"]').trigger("click");
    expect(wrapper.find("textarea").exists()).toBe(false);
    expect(
      vi
        .mocked(invoke)
        .mock.calls.map(([command]) => command)
        .sort(),
    ).toEqual([
      "engine_list_scripts",
      "llm_get_key_status",
      "llm_list_profiles",
      "locale_get_preference",
      "locale_set_preference",
      "memory_get_app_config",
      "memory_get_app_config",
      "memory_get_script_config",
      "memory_get_script_config",
      "store_get_ui_preferences",
      "store_set_ui_preferences",
      "theme_get_preference",
      "theme_list",
      "theme_skin_load",
    ]);
    wrapper.unmount();
  });
  it("浏览器缺 IPC 明确展示恢复错误，不伪造本地生成结果", async () => {
    vi.mocked(invoke).mockRejectedValue({ code: "app.not-ready", message: "桌面 IPC 不可用" });
    const wrapper = mount(App, { global: { plugins: [i18n] } });
    await vi.waitFor(() => expect(wrapper.text()).toContain("app is still starting"));
    wrapper.unmount();
  });

  it("mounts the latest saved locale immediately after startup hydration", async () => {
    const result = {
      preference: { version: 1 as const, locale: "zh-Hans" as const },
      resolvedLocale: "zh-Hans" as const,
      nativeStatus: "applied" as const,
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "llm_list_profiles") return { items: [] };
      if (command === "llm_get_key_status") return { set: false, hint: null };
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
    expect(wrapper.find(".welcome-start").attributes("disabled")).toBeDefined();
    expect(document.documentElement.lang).toBe("zh-Hans");
    expect(invoke).not.toHaveBeenCalledWith("locale_get_preference");
    await wrapper.get('button[aria-label="设置"]').trigger("click");
    await wrapper
      .findAll(".settings-nav button")
      .find((button) => button.text() === "模型与 API")!
      .trigger("click");
    expect(wrapper.get(".settings-key-card").classes()).toContain("is-missing");
    expect(wrapper.get(".settings-key-card").text()).toContain("密钥未设置");
    await wrapper
      .findAll(".settings-nav button")
      .find((button) => button.text() === "游戏")!
      .trigger("click");
    expect(
      wrapper
        .findAll("button")
        .find((button) => button.text() === "重开 / 首次打开")!
        .attributes("disabled"),
    ).toBeDefined();
    expect(
      wrapper
        .findAll("button")
        .find((button) => button.text() === "新建周目")!
        .attributes("disabled"),
    ).toBeDefined();
    wrapper.unmount();
  });
});
