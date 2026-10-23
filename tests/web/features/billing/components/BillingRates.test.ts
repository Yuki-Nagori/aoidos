import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PriceVersion } from "../../../../../src-web/api/billing";
import BillingRates from "../../../../../src-web/features/billing/components/BillingRates.vue";
import { i18n } from "../../../../../src-web/i18n/index";
const price: PriceVersion = {
  versionId: "registered-price",
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
afterEach(() => {
  vi.useRealTimers();
});
function render(value: PriceVersion | null = price) {
  return mount(BillingRates, {
    props: { price: value, locale: "en", timeZone: "Asia/Shanghai", providerLabel: "DeepSeek API" },
    global: { plugins: [i18n] },
  });
}
describe("BillingRates", () => {
  it("uses the complete frozen tier report without changing historic amounts or combining currencies", async () => {
    const wrapper = render();
    await wrapper.setProps({
      usage: [
        {
          currency: "CNY",
          tier: "peak",
          settledAmount: "1.25",
          reservedAmount: "0.2",
          requestCount: 80,
          estimatedRequests: 1,
        },
        {
          currency: "CNY",
          tier: "off",
          settledAmount: "3.75",
          reservedAmount: "0",
          requestCount: 120,
          estimatedRequests: 0,
        },
        {
          currency: "USD",
          tier: "regular",
          settledAmount: "2",
          reservedAmount: "0",
          requestCount: 2,
          estimatedRequests: 0,
        },
      ],
    });
    expect(wrapper.findAll(".billing-tier-group")).toHaveLength(2);
    expect(wrapper.findAll(".billing-tier-row")).toHaveLength(3);
    expect(wrapper.get(".billing-tier-split .peak").attributes("style")).toContain("25%");
    expect(wrapper.text()).toContain(i18n.global.t("billing.estimated"));
    expect(wrapper.text()).toContain(i18n.global.t("rates.requestCount", { count: 120 }));
    wrapper.unmount();
  });
  it("renders the registered prices and metadata with a live tier and zone controls", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-23T04:48:00Z"));
    const wrapper = render();
    expect(wrapper.text()).toContain("registered-price");
    expect(wrapper.findAll("tbody tr")).toHaveLength(4);
    expect(wrapper.findAll(".billing-rates-week-row i")).toHaveLength(168);
    expect(wrapper.get(".billing-rates-now-copy strong").text()).toBe(
      i18n.global.t("rates.nowOff"),
    );
    await wrapper.get(".billing-rates-segment button:last-child").trigger("click");
    expect(wrapper.get(".billing-rates-segment button:last-child").attributes("aria-pressed")).toBe(
      "true",
    );
    expect(wrapper.get(".billing-rates-now-line b").text()).toBe("04:48");
    expect(
      wrapper.find('a[href="https://api-docs.deepseek.com/quick_start/pricing/"]').exists(),
    ).toBe(true);
    vi.setSystemTime(new Date("2026-10-23T06:00:00Z"));
    await vi.advanceTimersByTimeAsync(60_000);
    expect(wrapper.get(".billing-rates-now-copy strong").text()).toBe(
      i18n.global.t("rates.nowPeak"),
    );
    expect(wrapper.text()).toContain(i18n.global.t("rates.splitUnavailable"));
    wrapper.unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
  it("does not manufacture a schedule, source link, or registered price", async () => {
    const wrapper = render({ ...price, schedule: null, sourceUrl: "javascript:alert(1)" });
    expect(wrapper.find(".billing-rates-week").exists()).toBe(false);
    expect(wrapper.find("a").exists()).toBe(false);
    expect(wrapper.findAll("tbody tr td")).toHaveLength(8);
    await wrapper.setProps({ price: null });
    expect(wrapper.get('[role="status"]').text()).toBe(i18n.global.t("billing.priceMissing"));
    wrapper.unmount();
  });
});
