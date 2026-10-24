import { mount } from "@vue/test-utils";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "../../../src-web/app/App.vue";
import RunCatalogPanel from "../../../src-web/features/product/components/RunCatalogPanel.vue";
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
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "llm_list_profiles") return { items: [] };
      if (command === "engine_open_session") throw new Error("race test request completed");
      if (command === "engine_list_scripts")
        return [
          {
            scriptId: "mistbell",
            title: "雾钟地窖",
            displayNames: { en: "Mistbell Cellar", "zh-Hans": "雾钟地窖" },
            suggestedActions: {
              en: "I look around, trying to understand where I am and notice any possible exits.",
              "zh-Hans": "我环顾四周，试着弄清自己身在何处，并留意可能的出口。",
            },
            attributions:
              "# Script License\n\nCC BY 4.0\n\n# SRD Attribution\n\nWizards of the Coast",
          },
          {
            scriptId: "lantern-archive",
            title: "灯塔档案",
            displayNames: { en: "Lantern Archive", "zh-Hans": "灯塔档案" },
            suggestedActions: { en: "Look around.", "zh-Hans": "环顾四周。" },
            attributions: "",
          },
        ];
      if (command === "theme_get_preference") return { version: 1, theme: "dark" };
      if (command === "theme_set_preference")
        return { version: 1, theme: (args as { theme?: string } | undefined)?.theme ?? "dark" };
      if (command === "store_get_ui_preferences")
        return { version: 1, panelPinned: false, diceMode: "manual", diceSkin: "auto" };
      if (command === "store_set_ui_preferences")
        return { version: 1, panelPinned: false, diceMode: "auto", diceSkin: "auto" };
      if (command === "theme_list")
        return [
          { id: "dark", name: "深色", colorScheme: "dark" },
          { id: "light", name: "浅色", colorScheme: "light" },
          { id: "light-purple", name: "浅紫", colorScheme: "light" },
        ];
      if (command === "theme_skin_load")
        return {
          scriptId: "mistbell",
          status: "valid",
          tokens: {
            "--accent": "#5cc8b4",
            "--on-accent": "#102126",
            "--accent-violet": "#a88bff",
            "--accent-warm": "#f0924a",
          },
          warnings: [{ code: "protected-token", token: "--panel" }],
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
    const welcomeScenario = wrapper.get('.welcome-script-select [role="combobox"]');
    expect(welcomeScenario.text()).toContain("Mistbell Cellar");
    await welcomeScenario.trigger("click");
    await wrapper.get('[role="option"][aria-selected="false"]').trigger("click");
    expect(wrapper.get(".script-copy").text()).toContain("Lantern Archive");
    await welcomeScenario.trigger("click");
    await wrapper.get('[role="option"][aria-selected="false"]').trigger("click");
    expect(wrapper.get(".script-copy").text()).toContain("Mistbell Cellar");
    await wrapper.get('button[aria-label="Settings"]').trigger("click");
    expect(wrapper.get(".app-shell").element.hasAttribute("inert")).toBe(true);
    expect(wrapper.text()).toContain("system dialog");
    expect(wrapper.findAll(".settings-nav-icon")).toHaveLength(14);
    const dialog = wrapper.get('[role="dialog"]');
    const focusable = Array.from(
      dialog.element.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), textarea:not(:disabled), a[href], summary, [tabindex]:not([tabindex="-1"])',
      ),
    ).filter((element) => {
      let parent: HTMLElement | null = element;
      while (parent && dialog.element.contains(parent)) {
        if (getComputedStyle(parent).display === "none") return false;
        parent = parent.parentElement;
      }
      return true;
    });
    focusable[0]!.focus();
    await focusable[0]!.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true }),
    );
    expect(document.activeElement).toBe(focusable.at(-1));
    await wrapper.get('.settings-theme-card[aria-label="Dark"]').trigger("keydown", {
      key: "Escape",
      isComposing: true,
    });
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
    expect(wrapper.get(".settings-key-card").text()).toContain("status has not been confirmed");
    expect(invoke).not.toHaveBeenCalledWith("llm_get_key_status", { providerId: "deepseek" });
    expect(wrapper.get(".settings-theme-field").findAll(".settings-theme-card")).toHaveLength(3);
    const theme = wrapper.get('.settings-theme-card[aria-label="Dark"]');
    expect(theme.attributes("aria-pressed")).toBe("true");
    await wrapper.get('.settings-theme-card[aria-label="Light Purple"]').trigger("click");
    await vi.waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("theme_set_preference", { theme: "light-purple" }),
    );
    const script = wrapper.get(".script-copy");
    expect(script.text()).toContain("Mistbell Cellar");
    const model = wrapper.get('[role="combobox"][aria-label="Model"]');
    expect(model.text()).toContain("DeepSeek V4.1 Flash");
    const language = wrapper.get('input[name="interface-language"][value="zh-Hans"]');
    await language.setValue();
    await vi.waitFor(() => {
      expect(
        wrapper.get('.settings-theme-card[aria-label="浅紫"]').attributes("aria-pressed"),
      ).toBe("true");
      expect(script.text()).toContain("雾钟地窖");
    });
    expect(wrapper.get(".settings-skin-section .settings-skin-heading").text()).toContain(
      "剧本皮肤",
    );
    expect(wrapper.get(".settings-skin .script-ic svg").classes()).toContain("i");
    await vi.waitFor(() =>
      expect(wrapper.get(".settings-skin-copy").text()).toContain("覆盖 4 个强调色 token"),
    );
    expect(wrapper.get(".settings-skin-warning").text()).toContain("剧本皮肤有 1 条兼容性提示。");
    expect(wrapper.get(".settings-skin-warning svg").classes()).toContain("i");
    const skinSwitch = wrapper.get('[role="switch"][aria-label="剧本皮肤"]');
    expect(skinSwitch.attributes("aria-checked")).toBe("true");
    await skinSwitch.trigger("click");
    expect(skinSwitch.attributes("aria-checked")).toBe("false");
    await wrapper.get('.settings-nav button[aria-controls="scenario"]').trigger("click");
    expect(wrapper.get(".scenario-script-copy").text()).toContain("雾钟地窖");
    expect(wrapper.get(".scenario-attributions").text()).toContain("CC BY 4.0");
    expect(
      wrapper
        .findAll(".scenario-attribution-section > .scenario-attribution-heading")
        .map((heading) => heading.text()),
    ).toEqual(["Script License", "SRD Attribution"]);
    expect(wrapper.findAll(".scenario-resource-table tr")).toHaveLength(2);
    expect(wrapper.get(".scenario-resource-table").text()).toContain("舞台图");
    await wrapper.get('.settings-nav button[aria-controls="gameplay"]').trigger("click");
    expect(wrapper.find("#gameplay .select-menu").exists()).toBe(false);
    await wrapper.get('input[name="dice-mode"][value="auto"]').setValue();
    await vi.waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("store_set_ui_preferences", {
        panelPinned: false,
        diceMode: "auto",
        diceSkin: "auto",
      });
    });
    await wrapper.get('[role="dialog"]').trigger("keydown", { key: "Escape" });
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    await vi.waitFor(() =>
      expect(wrapper.get('button[aria-label="设置"]').element).toBe(document.activeElement),
    );
    await wrapper.get(".model-settings").trigger("click");
    expect(wrapper.find('.settings-nav button[aria-current="page"]').text()).toBe("模型与 API");
    expect(wrapper.find(".settings-currency-heading").exists()).toBe(false);
    await wrapper.get("#api .settings-section-close").trigger("click");
    await vi.waitFor(() =>
      expect(wrapper.get(".model-settings").element).toBe(document.activeElement),
    );
    expect(wrapper.find("textarea").exists()).toBe(false);
    expect(
      vi
        .mocked(invoke)
        .mock.calls.map(([command]) => command)
        .sort(),
    ).toEqual([
      "budget_get_deepseek_price_templates",
      "engine_list_scripts",
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
      "theme_set_preference",
      "theme_skin_load",
      "theme_skin_load",
    ]);
    await wrapper.get(".welcome-start").trigger("click");
    expect(wrapper.findComponent(RunCatalogPanel).props("open")).toBe(true);
    expect(invoke).not.toHaveBeenCalledWith("llm_save_profile", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("engine_open_session", expect.anything());
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
            suggestedActions: {
              en: "I look around, trying to understand where I am and notice any possible exits.",
              "zh-Hans": "我环顾四周，试着弄清自己身在何处，并留意可能的出口。",
            },
            attributions: "CC BY 4.0",
          },
        ];
      if (command === "theme_get_preference") return { version: 1, theme: "dark" };
      if (command === "store_get_ui_preferences")
        return { version: 1, panelPinned: false, diceMode: "manual", diceSkin: "auto" };
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
    expect(wrapper.find(".welcome-start").attributes("disabled")).toBeUndefined();
    expect(wrapper.find(".welcome-new-cycle").attributes("disabled")).toBeDefined();
    expect(document.documentElement.lang).toBe("zh-Hans");
    expect(invoke).not.toHaveBeenCalledWith("locale_get_preference");
    await wrapper.get('button[aria-label="设置"]').trigger("click");
    await wrapper
      .findAll(".settings-nav button")
      .find((button) => button.text() === "模型与 API")!
      .trigger("click");
    expect(wrapper.get(".settings-key-card").classes()).not.toContain("is-missing");
    expect(wrapper.get(".settings-key-card").text()).toContain("密钥状态尚未确认");
    await wrapper.get(".settings-key-card button.ghost").trigger("click");
    await vi.waitFor(() =>
      expect(wrapper.get(".settings-key-card").classes()).toContain("is-missing"),
    );
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
