import { mount } from "@vue/test-utils";
import { ref } from "vue";
import { describe, expect, it, vi } from "vitest";
import type { useBilling } from "../../../../../src-web/features/billing/composables/useBilling";
import BillingPanel from "../../../../../src-web/features/billing/components/BillingPanel.vue";
import { i18n } from "../../../../../src-web/i18n/index";
import { formatExactCurrency } from "../../../../../src-web/shared/utils/format";

describe("BillingPanel", () => {
  it("reacts to session and provider changes after mount", async () => {
    const period = ref({
      items: [
        {
          providerId: "openrouter",
          currency: "USD",
          modelId: "model",
          settledAmount: "12.50",
          reservedAmount: "0",
          knownTokens: 25,
          incompleteRequests: 0,
        },
      ],
      warnings: [],
    });
    const settings = ref({ cnyLimit: "20", usdLimit: "3" });
    const billing = new Proxy(
      {},
      {
        get: (_target, key) => {
          if (key === "__v_isRef" || key === "then") return undefined;
          if (key === "period") return period;
          if (key === "settings") return settings;
          if (key === "requests") return ref({ items: [] });
          if (key === "monthly") return ref({ month: "", timeZone: "UTC", items: [] });
          if (typeof key === "string" && key.includes("input")) return ref("");
          return ref(false);
        },
      },
    ) as ReturnType<typeof useBilling>;
    const wrapper = mount(BillingPanel, {
      props: { billing, locale: "en", active: false, providerId: "openrouter" },
      global: { plugins: [i18n] },
    });

    expect(wrapper.find("h4").exists()).toBe(false);
    await wrapper.setProps({ active: true });
    expect(wrapper.get("h4").text()).toBe(i18n.global.t("billing.title"));
    expect(wrapper.text()).not.toContain(i18n.global.t("billing.queryBalance"));

    await wrapper.setProps({ providerId: "deepseek" });
    expect(wrapper.text()).toContain(i18n.global.t("billing.queryBalance"));
    expect(wrapper.text()).toContain(formatExactCurrency("12.50", "USD", "en"));
    expect(wrapper.text()).toContain(formatExactCurrency("3", "USD", "en"));
    await wrapper.setProps({ locale: "zh-Hans" });
    expect(wrapper.text()).toContain(formatExactCurrency("12.50", "USD", "zh-Hans"));
  });

  it("shows only the selected design section in the settings drawer", async () => {
    const billing = new Proxy(
      {},
      {
        get: (_target, key) => {
          if (key === "__v_isRef" || key === "then") return undefined;
          if (typeof key === "string" && key.includes("input")) return ref("");
          return ref(false);
        },
      },
    ) as ReturnType<typeof useBilling>;
    const wrapper = mount(BillingPanel, {
      props: {
        billing,
        locale: "en",
        active: true,
        providerId: "deepseek",
        providerLabel: "DeepSeek",
        modelId: "deepseek-v4-flash",
        view: "price",
      },
      global: { plugins: [i18n] },
    });

    expect(wrapper.text()).toContain(i18n.global.t("billing.savePrice"));
    expect(wrapper.text()).toContain("DeepSeek");
    expect(wrapper.text()).toContain("deepseek-v4-flash");
    expect(wrapper.find(".billing-price-context [role=status]").exists()).toBe(true);
    expect(wrapper.text()).not.toContain(i18n.global.t("billing.cnyLimit"));
    expect(wrapper.text()).not.toContain(i18n.global.t("billing.queryBalance"));

    await wrapper.setProps({ view: "balance" });
    expect(wrapper.text()).toContain(i18n.global.t("billing.queryBalance"));
    expect(wrapper.text()).not.toContain(i18n.global.t("billing.savePrice"));
  });

  it("presents the cached provider balance and keeps its status explicit", async () => {
    const queryProviderBalance = vi.fn();
    const billing = new Proxy(
      {},
      {
        get: (_target, key) => {
          if (key === "__v_isRef" || key === "then") return undefined;
          if (key === "providerBalance")
            return ref({
              providerId: "deepseek",
              queriedAt: "2026-10-19T00:00:00Z",
              stale: true,
              status: "available",
              accounts: [{ currency: "CNY", total: "12.34", granted: "2", toppedUp: "10.34" }],
            });
          if (key === "monthly")
            return ref({ month: "2026-10", timeZone: "Asia/Shanghai", items: [] });
          if (key === "balanceBusy") return ref(false);
          if (key === "queryProviderBalance") return queryProviderBalance;
          return ref(false);
        },
      },
    ) as ReturnType<typeof useBilling>;
    const wrapper = mount(BillingPanel, {
      props: {
        billing,
        locale: "zh-Hans",
        active: true,
        providerId: "deepseek",
        providerLabel: "DeepSeek API",
        view: "balance",
      },
      global: { plugins: [i18n] },
    });

    expect(wrapper.get(".billing-balance-header").text()).toContain("DeepSeek API");
    expect(wrapper.text()).toContain(i18n.global.t("billing.balanceStale"));
    expect(wrapper.text()).toContain("¥12.34");
    await wrapper.get(".billing-balance-section button").trigger("click");
    expect(queryProviderBalance).toHaveBeenCalledOnce();
  });
});
