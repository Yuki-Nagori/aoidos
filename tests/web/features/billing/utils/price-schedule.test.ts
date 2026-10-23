import { describe, expect, it, vi } from "vitest";
import {
  isDoublePrice,
  formatPeakSchedule,
  priceTierAt,
  priceWeek,
  nextPriceTransition,
  resolvePriceTimeZone,
} from "../../../../../src-web/features/billing/utils/price-schedule";

describe("formatPeakSchedule", () => {
  it("converts the provider's UTC peak windows to the selected timezone", () => {
    const windows = formatPeakSchedule("Asia/Shanghai", "en-US", new Date("2026-10-12T12:00:00Z"));
    expect(windows).toContain("Mon 09:00–12:00");
    expect(windows).toContain("Mon 14:00–18:00");
    expect(windows).toContain("Fri 14:00–18:00");
  });

  it("represents windows that cross the selected timezone's weekday boundary", () => {
    const windows = formatPeakSchedule(
      "America/New_York",
      "en-US",
      new Date("2026-10-12T12:00:00Z"),
    );
    expect(windows).toContain("Sun 21:00–Mon 00:00");
    expect(windows).toContain("Mon 02:00–06:00");
  });

  it("falls back to UTC when the saved timezone is invalid", () => {
    const windows = formatPeakSchedule("No/Such_Zone", "en-US", new Date("2026-10-12T00:30:00Z"));
    expect(windows).toContain("Mon 01:00–04:00");
    expect(windows).toContain("Mon 06:00–10:00");
  });

  it.each([
    ["weekday", "? 01:00–04:00"],
    ["hour", "Mon 00:00–00:00"],
    ["minute", "Mon 01:00–04:00"],
  ] as const)("uses a fallback when Intl omits the %s part", (missingPart, expectedWindow) => {
    const originalFormatToParts = Intl.DateTimeFormat.prototype.formatToParts;
    const formatToParts = vi
      .spyOn(Intl.DateTimeFormat.prototype, "formatToParts")
      .mockImplementation(function (this: Intl.DateTimeFormat, instant) {
        return originalFormatToParts
          .call(this, instant)
          .filter((part) => part.type !== missingPart);
      });
    try {
      const windows = formatPeakSchedule("UTC", "en-US", new Date("2026-10-12T12:00:00Z"));
      expect(windows.split(" · ")[0]).toBe(expectedWindow);
      expect(windows.split(" · ")).toHaveLength(10);
    } finally {
      formatToParts.mockRestore();
    }
  });
});

const deepseek = "deepseek-api-utc-weekday-holiday-2026-v1";
const go = "opencode-go-deepseek-utc-weekday-v1";
describe("versioned rate display", () => {
  it("matches the UTC peak boundaries and preserves DeepSeek holidays in China time", () => {
    expect(priceTierAt(deepseek, new Date("2026-10-23T00:59:59Z"))).toBe("off");
    expect(priceTierAt(deepseek, new Date("2026-10-23T01:00:00Z"))).toBe("peak");
    expect(priceTierAt(deepseek, new Date("2026-10-23T04:00:00Z"))).toBe("off");
    expect(priceTierAt(deepseek, new Date("2026-10-01T01:30:00Z"))).toBe("off");
    expect(priceTierAt(go, new Date("2026-10-01T01:30:00Z"))).toBe("peak");
    expect(priceTierAt("future-schedule", new Date())).toBeUndefined();
  });
  it("uses the same conservative holiday fallback as the backend outside 2026", () => {
    expect(priceTierAt(deepseek, new Date("2027-01-01T01:00:00Z"))).toBe("peak");
    expect(priceTierAt(deepseek, new Date("2027-01-02T01:00:00Z"))).toBe("off");
  });
  it("finds the next tier across weekends and the multi-day holiday", () => {
    expect(nextPriceTransition(deepseek, new Date("2026-10-23T10:00:00Z"))?.toISOString()).toBe(
      "2026-10-26T01:00:00.000Z",
    );
    expect(nextPriceTransition(deepseek, new Date("2026-10-01T00:00:00Z"))?.toISOString()).toBe(
      "2026-10-08T01:00:00.000Z",
    );
    expect(nextPriceTransition("future-schedule", new Date())).toBeUndefined();
  });
  it("renders exactly seven local days and twenty-four hourly cells", () => {
    const week = priceWeek(deepseek, "Asia/Shanghai", new Date("2026-10-23T04:48:00Z"));
    expect(week.map((day) => day.date)).toEqual([
      "2026-10-19",
      "2026-10-20",
      "2026-10-21",
      "2026-10-22",
      "2026-10-23",
      "2026-10-24",
      "2026-10-25",
    ]);
    expect(week.every((day) => day.hours.length === 24)).toBe(true);
    expect(week[4]?.today).toBe(true);
    expect(
      week[4]?.hours.map((hour, index) => (hour.peak ? index : -1)).filter((hour) => hour >= 0),
    ).toEqual([9, 10, 11, 14, 15, 16, 17]);
    expect(week[5]?.hours.every((hour) => hour.peak === 0)).toBe(true);
  });
  it("projects cross-day windows, quarter-hour offsets, holidays and invalid zones", () => {
    const now = new Date("2026-10-23T04:48:00Z");
    expect(priceWeek(deepseek, "America/New_York", now)[0]?.hours[21]?.peak).toBe(1);
    const kathmandu = priceWeek(deepseek, "Asia/Kathmandu", now);
    expect(kathmandu[0]?.hours[6]?.peak).toBe(0.25);
    expect(kathmandu[0]?.hours[9]?.peak).toBe(0.75);
    const holiday = priceWeek(deepseek, "Asia/Shanghai", new Date("2026-10-01T04:00:00Z"));
    expect(holiday[3]?.hours.every((hour) => hour.peak === 0)).toBe(true);
    expect(priceWeek(deepseek, "No/Such_Zone", now)).toEqual(priceWeek(deepseek, "UTC", now));
    expect(resolvePriceTimeZone("No/Such_Zone")).toBe("UTC");
  });
  it("keeps the local week correct across a DST transition", () => {
    const week = priceWeek(go, "America/New_York", new Date("2026-11-01T08:00:00Z"));
    expect(week[6]?.date).toBe("2026-11-01");
    expect(week[6]?.today).toBe(true);
    expect(week[6]?.hours[20]?.peak).toBe(1);
    expect(week[6]?.hours[23]?.peak).toBe(0);
  });
});

describe("price multiplier badges", () => {
  it("only marks exact doubles without rounding decimal amounts", () => {
    expect(isDoublePrice("0.02", "0.040")).toBe(true);
    expect(isDoublePrice("9007199254740993.01", "18014398509481986.02")).toBe(true);
    expect(isDoublePrice("9007199254740993.01", "18014398509481986.03")).toBe(false);
    expect(isDoublePrice("0", "0")).toBe(false);
    expect(isDoublePrice(null, "2")).toBe(false);
    expect(isDoublePrice("not-a-price", "2")).toBe(false);
  });
});
