import { mount } from "@vue/test-utils";
import { ref } from "vue";
import { describe, expect, it, vi } from "vitest";
import SetupPanel from "../../../src-web/app/components/SetupPanel.vue";
import RunCatalogPanel from "../../../src-web/features/product/components/RunCatalogPanel.vue";
import type { OpenedRun } from "../../../src-web/api/runs";
import { i18n } from "../../../src-web/i18n/index";

describe("SetupPanel", () => {
  it("loads the run catalog only in its settings section and adopts an explicitly restored run", async () => {
    i18n.global.locale.value = "en";
    const adoptOpenedRun = vi.fn();
    const product = {
      scripts: ref([]),
      profiles: ref([]),
      priceTemplates: ref([]),
      billingCurrency: ref("CNY"),
      registeredBillingCurrency: ref(undefined),
      providerId: ref("custom"),
      providerOptions: [],
      model: ref("model"),
      models: ref([]),
      scriptId: ref(""),
      session: ref(undefined),
      busy: ref(false),
      error: ref(undefined),
      keyStatus: ref(undefined),
      keyStatusLoading: ref(false),
      adoptOpenedRun,
    } as unknown as InstanceType<typeof SetupPanel>["$props"]["product"];
    const theme = {
      themes: ref([]),
      theme: ref("dark"),
      disabled: ref(false),
      saving: ref(false),
      themeError: ref(undefined),
      skinError: ref(undefined),
      skinTokenCount: ref(0),
      warningCount: ref(0),
    } as unknown as InstanceType<typeof SetupPanel>["$props"]["theme"];
    const locale = {
      resolved: ref("en"),
      choice: ref("en"),
      notice: ref(undefined),
      fallback: ref(false),
      ready: ref(true),
      saving: ref(false),
    } as unknown as InstanceType<typeof SetupPanel>["$props"]["locale"];
    const preferences = {
      diceMode: ref("manual"),
      diceSkin: ref("auto"),
      ready: ref(true),
      saving: ref(false),
    } as unknown as InstanceType<typeof SetupPanel>["$props"]["preferences"];
    const wrapper = mount(SetupPanel, {
      props: {
        open: false,
        initialSection: "appearance",
        product,
        theme,
        locale,
        preferences,
        billing: {} as InstanceType<typeof SetupPanel>["$props"]["billing"],
      },
      global: {
        plugins: [i18n],
        stubs: {
          BillingPanel: true,
          PriceTemplateDialog: true,
          DiceSkinPicker: true,
          KeyboardShortcuts: true,
          SelectMenu: true,
          RunCatalogPanel: true,
        },
      },
    });
    await wrapper.setProps({ open: true });
    expect(wrapper.findComponent(RunCatalogPanel).exists()).toBe(false);
    await wrapper.get('.settings-nav button[aria-controls="runs"]').trigger("click");
    const catalog = wrapper.findComponent(RunCatalogPanel);
    expect(catalog.props()).toMatchObject({ open: true, embedded: true, locale: "en" });
    expect(catalog.props("scripts")).toEqual([]);
    expect(catalog.props("profiles")).toEqual([]);
    const opened = {
      session: { sessionId: "restored", scriptId: "mistbell" },
      profile: { profileId: "saved" },
    } as OpenedRun;
    catalog.vm.$emit("restore", opened);
    expect(adoptOpenedRun).toHaveBeenCalledWith(opened);
    expect(wrapper.emitted("close")).toHaveLength(1);
    wrapper.unmount();
  });
});
