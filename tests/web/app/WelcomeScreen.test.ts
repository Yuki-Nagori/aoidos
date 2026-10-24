import { mount } from "@vue/test-utils";
import { ref } from "vue";
import { afterEach, expect, it, vi } from "vitest";
import type { useProduct } from "../../../src-web/features/product/composables/useProduct";
import WelcomeScreen from "../../../src-web/app/components/WelcomeScreen.vue";
import { i18n } from "../../../src-web/i18n/index";

vi.mock("../../../src-web/shared/components/SelectMenu.vue", () => ({
  default: { template: "<div><slot /></div>" },
}));
vi.mock("../../../src-web/features/product/components/RunCatalogPanel.vue", () => ({
  default: {
    props: ["open"],
    template: "<div data-testid='run-catalog' :data-open='open' />",
  },
}));

afterEach(() => {
  i18n.global.locale.value = "en";
});

it("requires explicit dual-currency limits and prepares without opening or dispatching a new run", async () => {
  i18n.global.locale.value = "en";
  const prepareNewCycle = vi.fn().mockResolvedValue("prepared-session");
  const open = vi.fn();
  const product = {
    scripts: ref([
      {
        scriptId: "mistbell",
        title: "Mistbell Cellar",
        displayNames: { en: "Mistbell Cellar", "zh-Hans": "雾钟地窖" },
        suggestedActions: { en: "Look around.", "zh-Hans": "环顾四周。" },
        attributions: "notice",
      },
    ]),
    scriptId: ref("mistbell"),
    models: ref([{ id: "deepseek-flash", label: "DeepSeek V4.1 Flash" }]),
    model: ref("deepseek-flash"),
    providerId: ref("deepseek"),
    providerOptions: [{ id: "deepseek", label: "DeepSeek" }],
    keyStatus: ref({ set: true, hint: null }),
    keyStatusLoading: ref(false),
    busy: ref(false),
    error: ref(undefined),
    profiles: ref([]),
    prepareNewCycle,
    open,
    adoptOpenedRun: vi.fn(),
  } as unknown as ReturnType<typeof useProduct>;

  const wrapper = mount(WelcomeScreen, {
    props: { product, locale: "en" as const },
    global: { plugins: [i18n] },
  });

  await wrapper.get(".welcome-new-cycle").trigger("click");
  const prepare = wrapper.get(".welcome-budget-dialog .primary");
  expect(prepare.attributes("disabled")).toBeDefined();
  await wrapper.findAll(".welcome-budget-field input")[0]!.setValue("12.5");
  await wrapper.findAll(".welcome-budget-field input")[1]!.setValue("3");
  await wrapper.get(".welcome-budget-ack input").setValue(true);
  expect(prepare.attributes("disabled")).toBeUndefined();

  await prepare.trigger("click");

  expect(prepareNewCycle).toHaveBeenCalledExactlyOnceWith("12.5", "3", "en");
  expect(open).not.toHaveBeenCalled();
  expect(wrapper.find(".welcome-budget-dialog").exists()).toBe(false);
  expect(wrapper.get('[data-testid="run-catalog"]').attributes("data-open")).toBe("true");
  wrapper.unmount();
});

it("opens the existing-run catalog from the welcome continuation action instead of creating a run", async () => {
  const open = vi.fn();
  const product = {
    scripts: ref([
      {
        scriptId: "mistbell",
        title: "Mistbell",
        displayNames: { en: "Mistbell", "zh-Hans": "雾钟" },
      },
    ]),
    scriptId: ref("mistbell"),
    models: ref([]),
    model: ref(""),
    providerId: ref(""),
    providerOptions: [],
    keyStatus: ref({ set: true, hint: null }),
    keyStatusLoading: ref(false),
    busy: ref(false),
    profiles: ref([]),
    error: ref(undefined),
    prepareNewCycle: vi.fn(),
    open,
    adoptOpenedRun: vi.fn(),
  } as unknown as ReturnType<typeof useProduct>;
  const wrapper = mount(WelcomeScreen, {
    props: { product, locale: "en" as const },
    global: { plugins: [i18n] },
  });

  await wrapper.get(".welcome-start").trigger("click");

  expect(open).not.toHaveBeenCalled();
  expect(wrapper.get('[data-testid="run-catalog"]').attributes("data-open")).toBe("true");
  wrapper.unmount();
});

it("closes the budget dialog with Escape and restores focus to its trigger", async () => {
  const product = {
    scripts: ref([
      {
        scriptId: "mistbell",
        title: "Mistbell",
        displayNames: { en: "Mistbell", "zh-Hans": "雾钟" },
      },
    ]),
    scriptId: ref("mistbell"),
    models: ref([]),
    model: ref(""),
    providerId: ref(""),
    providerOptions: [],
    keyStatus: ref({ set: true, hint: null }),
    keyStatusLoading: ref(false),
    busy: ref(false),
    profiles: ref([]),
    error: ref(undefined),
    prepareNewCycle: vi.fn(),
    open: vi.fn(),
    adoptOpenedRun: vi.fn(),
  } as unknown as ReturnType<typeof useProduct>;
  const wrapper = mount(WelcomeScreen, {
    attachTo: document.body,
    props: { product, locale: "en" as const },
    global: { plugins: [i18n] },
  });

  const trigger = wrapper.get(".welcome-new-cycle");
  (trigger.element as HTMLButtonElement).focus();
  await trigger.trigger("click");
  expect(document.activeElement).toBe(wrapper.find(".welcome-budget-field input").element);
  await wrapper.get(".welcome-budget-dialog").trigger("keydown", { key: "Escape" });

  expect(wrapper.find(".welcome-budget-dialog").exists()).toBe(false);
  expect(document.activeElement).toBe(trigger.element);
  wrapper.unmount();
});
