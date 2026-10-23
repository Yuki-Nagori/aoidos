import { mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import type { PriceVersion } from "../../../../../src-web/api/billing";
import PriceTemplateDialog from "../../../../../src-web/features/billing/components/PriceTemplateDialog.vue";
import { i18n } from "../../../../../src-web/i18n/index";
const price: PriceVersion = {
  versionId: "reference-version",
  providerId: "deepseek",
  modelId: "deepseek-flash",
  routePolicy: "direct",
  currency: "CNY",
  unitTokens: 1000000,
  inputCached: "0.02",
  inputUncached: "1",
  inputCachedWrite: "1",
  output: "4",
  schedule: {
    version: "deepseek-api-utc-weekday-holiday-2026-v1",
    peak: { inputCached: "0.04", inputUncached: "2", inputCachedWrite: "2", output: "8" },
  },
  usageMappingVersion: "v1",
  sourceKind: "official",
  sourceUrl: "https://api-docs.deepseek.com/quick_start/pricing/",
  checkedAt: "2026-10-21T00:00:00Z",
  effectiveFrom: "2026-10-21T00:00:00Z",
};
describe("PriceTemplateDialog", () => {
  it("presents the complete reference card and never registers without confirmation", async () => {
    const wrapper = mount(PriceTemplateDialog, {
      props: {
        prices: [price, { ...price, currency: "USD", versionId: "usd-reference" }],
        currency: "CNY",
        locale: "en",
        busy: false,
      },
      global: { plugins: [i18n] },
      attachTo: document.body,
    });
    await vi.waitFor(() =>
      expect(document.activeElement?.textContent).toContain(i18n.global.t("common.cancel")),
    );
    expect(wrapper.findAll("tbody tr")).toHaveLength(4);
    expect(wrapper.findAll(".billing-rates-ribbon i")).toHaveLength(24);
    expect(wrapper.findAll(".billing-rates-x2")).toHaveLength(4);
    expect(wrapper.text()).toContain("reference-version");
    expect(wrapper.emitted("confirm")).toBeUndefined();
    await wrapper.get(".billing-rates-segment button:last-child").trigger("click");
    expect(wrapper.emitted("currency")?.[0]).toEqual(["USD"]);
    await wrapper.setProps({ currency: "USD", registeredCurrency: "USD" });
    expect(wrapper.text()).toContain("usd-reference");
    await wrapper.get(".billing-template-actions .primary").trigger("click");
    expect(wrapper.emitted("confirm")).toHaveLength(1);
    await wrapper.get(".billing-template-actions .ghost").trigger("click");
    expect(wrapper.emitted("manual")).toHaveLength(1);
    await wrapper.get('[role="dialog"]').trigger("keydown", { key: "Escape" });
    expect(wrapper.emitted("close")).toHaveLength(1);
    wrapper.unmount();
  });
  it("traps keyboard focus and disables confirmation when a currency has no reference", async () => {
    const wrapper = mount(PriceTemplateDialog, {
      props: { prices: [price], currency: "USD", locale: "en", busy: false },
      global: { plugins: [i18n] },
      attachTo: document.body,
    });
    expect(wrapper.get(".billing-template-actions .primary").attributes("disabled")).toBeDefined();
    expect(wrapper.get('[role="status"]').text()).toBe(
      i18n.global.t("settings.priceTemplateUnavailable"),
    );
    const buttons = wrapper.findAll("button:not(:disabled)");
    (buttons[0]!.element as HTMLElement).focus();
    await buttons[0]!.trigger("keydown", { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(buttons.at(-1)!.element);
    await buttons.at(-1)!.trigger("keydown", { key: "Tab" });
    expect(document.activeElement).toBe(buttons[0]!.element);
    await wrapper.setProps({ busy: true });
    await wrapper.get('[role="dialog"]').trigger("keydown", { key: "Escape" });
    expect(wrapper.emitted("close")).toBeUndefined();
    wrapper.unmount();
  });
});
