import { describe, expect, it } from "vitest";
import { formatDate, formatExactCurrency, formatInteger } from "../../../src-web/utils/format";

describe("exact locale formatting", () => {
  it("rounds decimal currency symmetrically without converting through Number", () => {
    expect(formatExactCurrency("9007199254740993.125", "USD", "en")).toBe(
      "$9,007,199,254,740,993.13",
    );
    expect(formatExactCurrency("-1.005", "USD", "en")).toBe("-$1.01");
    expect(formatExactCurrency("0.004", "USD", "en")).toBe("$0.00");
    expect(formatExactCurrency("-0.005", "USD", "en")).toBe("-$0.01");
    expect(formatExactCurrency("-0", "USD", "en")).toBe("$0.00");
  });

  it("uses locale currency placement and higher precision for detail views", () => {
    expect(formatExactCurrency("1234.5", "CNY", "zh-Hans")).toContain("¥");
    expect(formatExactCurrency("0.1234567895", "USD", "en", 9)).toBe("$0.123456790");
    expect(formatExactCurrency("-0.0000000005", "USD", "en", 9)).toBe("-$0.000000001");
  });

  it("rejects malformed amounts and formats arbitrary-size integers exactly", () => {
    for (const amount of ["1e3", "01.2", "-", "NaN", "1.2.3"]) {
      expect(() => formatExactCurrency(amount, "USD", "en")).toThrow(RangeError);
    }
    expect(formatInteger(9007199254740993n, "en")).toBe("9,007,199,254,740,993");
  });

  it("requires a valid date and explicit valid IANA time zone", () => {
    expect(formatDate("not a date", "en", "UTC")).toEqual({ text: "—", issue: "invalid-date" });
    expect(formatDate("2026-10-10T00:00:00Z", "en", " ")).toEqual({
      text: "—",
      issue: "invalid-time-zone",
    });
    expect(formatDate("2026-10-10T00:00:00Z", "en", "invalid-zone")).toEqual({
      text: "—",
      issue: "invalid-time-zone",
    });
    expect(formatDate("2026-10-10T00:00:00Z", "en", "Asia/Shanghai").issue).toBeUndefined();
  });

  it("accepts Date instances and reports invalid dates before checking their time zone", () => {
    const date = new Date("2026-10-10T00:00:00Z");
    expect(formatDate(date, "en", "UTC")).toEqual(formatDate(date.toISOString(), "en", "UTC"));
    expect(formatDate(new Date(NaN), "en", "invalid-zone")).toEqual({
      text: "—",
      issue: "invalid-date",
    });
  });

  it("rejects missing time zones received from untyped callers", () => {
    expect(formatDate("2026-10-10T00:00:00Z", "en", undefined as unknown as string)).toEqual({
      text: "—",
      issue: "invalid-time-zone",
    });
  });
});
