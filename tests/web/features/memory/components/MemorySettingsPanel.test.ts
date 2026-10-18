import { mount, flushPromises } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import MemorySettingsPanel from "../../../../../src-web/features/memory/components/MemorySettingsPanel.vue";
import { i18n } from "../../../../../src-web/i18n/index";
import {
  getMemoryAppConfig,
  getMemoryScriptConfig,
  saveMemoryAppConfig,
  saveMemoryScriptConfig,
} from "../../../../../src-web/api/memory";

vi.mock("../../../../../src-web/api/memory", () => ({
  getMemoryAppConfig: vi.fn(),
  getMemoryScriptConfig: vi.fn(),
  saveMemoryAppConfig: vi.fn(),
  saveMemoryScriptConfig: vi.fn(),
}));

const parameters = {
  algorithm: "exponential" as const,
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
};

describe("MemorySettingsPanel", () => {
  beforeEach(() => {
    i18n.global.locale.value = "en";
    vi.resetAllMocks();
    vi.mocked(getMemoryAppConfig).mockResolvedValue({
      configVersion: 3,
      algorithmVersion: 1,
      parameters,
      defaultParameters: parameters,
    });
    vi.mocked(getMemoryScriptConfig).mockResolvedValue({
      scriptId: "mistbell",
      configVersion: 2,
      appConfigVersion: 3,
      algorithmVersion: 1,
      overrides: {},
      inherited: parameters,
      effective: parameters,
    });
    vi.mocked(saveMemoryAppConfig).mockResolvedValue({ configVersion: 4 });
    vi.mocked(saveMemoryScriptConfig).mockResolvedValue({ configVersion: 3 });
  });

  it("saves the edited application policy with its observed version", async () => {
    const wrapper = mount(MemorySettingsPanel, {
      props: { scriptId: "mistbell" },
      global: { plugins: [i18n] },
    });
    await flushPromises();

    const input = wrapper.get('input[type="number"]');
    await input.setValue("600000");
    await wrapper.findAll("form")[0]!.trigger("submit");
    await flushPromises();

    expect(saveMemoryAppConfig).toHaveBeenCalledWith(
      3,
      expect.objectContaining({ baseStrength: 600_000 }),
    );
    expect(wrapper.text()).toContain("Policy saved");
    wrapper.unmount();
  });

  it("restores the built-in application values as a draft until explicitly saved", async () => {
    vi.mocked(getMemoryAppConfig).mockResolvedValue({
      configVersion: 3,
      algorithmVersion: 1,
      parameters: { ...parameters, baseStrength: 600_000 },
      defaultParameters: parameters,
    });
    const wrapper = mount(MemorySettingsPanel, {
      props: { scriptId: "mistbell" },
      global: { plugins: [i18n] },
    });
    await flushPromises();

    const baseStrength = wrapper.get('input[type="number"]');
    expect((baseStrength.element as HTMLInputElement).value).toBe("600000");
    await wrapper
      .findAll("button")
      .find((button) => button.text() === "Restore built-in defaults")!
      .trigger("click");
    expect((baseStrength.element as HTMLInputElement).value).toBe("500000");
    expect(saveMemoryAppConfig).not.toHaveBeenCalled();

    await wrapper.findAll("form")[0]!.trigger("submit");
    await flushPromises();
    expect(saveMemoryAppConfig).toHaveBeenCalledWith(3, parameters);
    wrapper.unmount();
  });

  it("restores every scenario override, including hidden algorithm fields, as an unsaved draft", async () => {
    vi.mocked(getMemoryScriptConfig).mockResolvedValue({
      scriptId: "mistbell",
      configVersion: 2,
      appConfigVersion: 3,
      algorithmVersion: 1,
      overrides: { algorithm: "segmented", halfLifeRounds: 200, baseStrength: 600_000 },
      inherited: parameters,
      effective: {
        ...parameters,
        algorithm: "segmented",
        halfLifeRounds: 200,
        baseStrength: 600_000,
      },
    });
    const wrapper = mount(MemorySettingsPanel, {
      props: { scriptId: "mistbell" },
      global: { plugins: [i18n] },
    });
    await flushPromises();

    const forms = wrapper.findAll("form");
    await forms[1]!
      .findAll("button")
      .find((button) => button.text() === "Restore application inheritance")!
      .trigger("click");
    expect(forms[1]!.get("select").element).toHaveProperty("value", "exponential");
    const inheritedBaseStrength = forms[1]!.find('input[type="number"]');
    expect((inheritedBaseStrength.element as HTMLInputElement).value).toBe("500000");
    expect((inheritedBaseStrength.element as HTMLInputElement).disabled).toBe(true);
    expect(forms[1]!.text()).not.toContain("Half-life rounds");
    expect(forms[1]!.text()).toContain("Exponential decay rate");
    expect(saveMemoryScriptConfig).not.toHaveBeenCalled();
    await forms[1]!.trigger("submit");
    await flushPromises();
    expect(saveMemoryScriptConfig).toHaveBeenCalledWith("mistbell", 2, 3, {});
    wrapper.unmount();
  });

  it("keeps the user's draft when a versioned save is rejected", async () => {
    vi.mocked(saveMemoryAppConfig).mockRejectedValue(new Error("stale version"));
    const wrapper = mount(MemorySettingsPanel, {
      props: { scriptId: "mistbell" },
      global: { plugins: [i18n] },
    });
    await flushPromises();

    const input = wrapper.get('input[type="number"]');
    await input.setValue("600000");
    await wrapper.findAll("form")[0]!.trigger("submit");
    await flushPromises();

    expect((input.element as HTMLInputElement).value).toBe("600000");
    expect(wrapper.text()).toContain("Your current edits are preserved");
    expect(wrapper.findAll("form")).toHaveLength(2);
    const retry = wrapper.findAll("button").find((button) => button.text() === "Retry");
    await retry!.trigger("click");
    await flushPromises();
    expect((input.element as HTMLInputElement).value).toBe("600000");
    wrapper.unmount();
  });

  it("does not turn an empty numeric field into zero", async () => {
    const wrapper = mount(MemorySettingsPanel, {
      props: { scriptId: "mistbell" },
      global: { plugins: [i18n] },
    });
    await flushPromises();

    const baseStrength = wrapper.get('input[type="number"]');
    await baseStrength.setValue("");
    expect(baseStrength.element).toHaveProperty("value", "");
    expect(wrapper.findAll("button").some((button) => button.text() === "Save policy")).toBe(true);
    await wrapper.findAll("form")[0]!.trigger("submit");
    expect(saveMemoryAppConfig).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain("Enter a whole number");
    wrapper.unmount();
  });

  it("does not restore an old scenario draft after switching during the app-save refresh", async () => {
    let resolveOldRefresh:
      | ((value: Awaited<ReturnType<typeof getMemoryScriptConfig>>) => void)
      | undefined;
    vi.mocked(getMemoryScriptConfig)
      .mockResolvedValueOnce({
        scriptId: "mistbell",
        configVersion: 2,
        appConfigVersion: 3,
        algorithmVersion: 1,
        overrides: { reinforcementGain: 200_000 },
        inherited: parameters,
        effective: parameters,
      })
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveOldRefresh = resolve;
          }),
      )
      .mockResolvedValueOnce({
        scriptId: "other-script",
        configVersion: 5,
        appConfigVersion: 4,
        algorithmVersion: 1,
        overrides: { reinforcementGain: 250_000 },
        inherited: parameters,
        effective: parameters,
      });
    const wrapper = mount(MemorySettingsPanel, {
      props: { scriptId: "mistbell" },
      global: { plugins: [i18n] },
    });
    await flushPromises();
    await wrapper.findAll("form")[0]!.trigger("submit");
    await vi.waitFor(() => expect(getMemoryScriptConfig).toHaveBeenCalledTimes(2));
    await wrapper.setProps({ scriptId: "other-script" });
    await vi.waitFor(() => expect(getMemoryScriptConfig).toHaveBeenCalledTimes(3));
    await flushPromises();
    resolveOldRefresh?.({
      scriptId: "mistbell",
      configVersion: 3,
      appConfigVersion: 4,
      algorithmVersion: 1,
      overrides: { reinforcementGain: 200_000 },
      inherited: parameters,
      effective: parameters,
    });
    await flushPromises();

    expect(
      (wrapper.findAll("form")[1]!.findAll('input[type="number"]')[1]!.element as HTMLInputElement)
        .value,
    ).toBe("250000");
    wrapper.unmount();
  });

  it("keeps a new scenario draft when a prior scenario save finishes", async () => {
    let resolveOldSave: ((value: { configVersion: number }) => void) | undefined;
    vi.mocked(getMemoryScriptConfig)
      .mockResolvedValueOnce({
        scriptId: "mistbell",
        configVersion: 2,
        appConfigVersion: 3,
        algorithmVersion: 1,
        overrides: { reinforcementGain: 200_000 },
        inherited: parameters,
        effective: parameters,
      })
      .mockResolvedValueOnce({
        scriptId: "other-script",
        configVersion: 5,
        appConfigVersion: 3,
        algorithmVersion: 1,
        overrides: { reinforcementGain: 250_000 },
        inherited: parameters,
        effective: parameters,
      });
    vi.mocked(saveMemoryScriptConfig).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveOldSave = resolve;
        }),
    );
    const wrapper = mount(MemorySettingsPanel, {
      props: { scriptId: "mistbell" },
      global: { plugins: [i18n] },
    });
    await flushPromises();
    const forms = wrapper.findAll("form");
    await forms[1]!.findAll('input[type="number"]')[1]!.setValue("300000");
    await forms[1]!.trigger("submit");
    await vi.waitFor(() => expect(saveMemoryScriptConfig).toHaveBeenCalledOnce());
    await wrapper.setProps({ scriptId: "other-script" });
    await vi.waitFor(() => expect(getMemoryScriptConfig).toHaveBeenCalledTimes(2));
    await flushPromises();
    const newForm = wrapper.findAll("form")[1]!;
    await newForm.findAll('input[type="number"]')[1]!.setValue("260000");
    resolveOldSave?.({ configVersion: 3 });
    await flushPromises();

    expect((newForm.findAll('input[type="number"]')[1]!.element as HTMLInputElement).value).toBe(
      "260000",
    );
    wrapper.unmount();
  });

  it("allows only one in-flight save per scenario", async () => {
    const resolveSave = new Map<string, (value: { configVersion: number }) => void>();
    vi.mocked(getMemoryScriptConfig).mockImplementation(async (scriptId) => ({
      scriptId,
      configVersion: 2,
      appConfigVersion: 3,
      algorithmVersion: 1,
      overrides: {},
      inherited: parameters,
      effective: parameters,
    }));
    vi.mocked(saveMemoryScriptConfig).mockImplementation(
      (scriptId) =>
        new Promise((resolve) => {
          resolveSave.set(scriptId, resolve);
        }),
    );
    const wrapper = mount(MemorySettingsPanel, {
      props: { scriptId: "mistbell" },
      global: { plugins: [i18n] },
    });
    await flushPromises();
    await wrapper.findAll("form")[1]!.trigger("submit");
    await vi.waitFor(() => expect(saveMemoryScriptConfig).toHaveBeenCalledTimes(1));

    await wrapper.setProps({ scriptId: "other-script" });
    await vi.waitFor(() => expect(getMemoryScriptConfig).toHaveBeenCalledTimes(2));
    await flushPromises();
    await wrapper.findAll("form")[1]!.trigger("submit");
    await vi.waitFor(() => expect(saveMemoryScriptConfig).toHaveBeenCalledTimes(2));

    await wrapper.setProps({ scriptId: "mistbell" });
    await flushPromises();
    expect(wrapper.findAll("form")[1]!.get('button[type="submit"]').element).toHaveProperty(
      "disabled",
      true,
    );

    resolveSave.get("mistbell")?.({ configVersion: 3 });
    resolveSave.get("other-script")?.({ configVersion: 3 });
    await flushPromises();
    wrapper.unmount();
  });
});
