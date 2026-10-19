import { describe, expect, it } from "vitest";
import { summarizeRunUsage } from "../../../../../src-web/features/billing/utils/run-usage";

describe("summarizeRunUsage", () => {
  it("adds settled and reserved balances by original currency without conversion", () => {
    expect(
      summarizeRunUsage(
        [
          { currency: "CNY", settledAmount: "1.25", reservedAmount: "0.000000001" },
          { currency: "CNY", settledAmount: null, reservedAmount: "0.75" },
          { currency: "USD", settledAmount: "2", reservedAmount: null },
        ],
        { CNY: "20", USD: "5" },
      ),
    ).toEqual([
      { currency: "CNY", amount: "2.000000001", limit: "20" },
      { currency: "USD", amount: "2", limit: "5" },
    ]);
  });

  it("shows configured limits before any charge and omits currencies with no data", () => {
    expect(summarizeRunUsage([], { CNY: "20", USD: null })).toEqual([
      { currency: "CNY", amount: "0", limit: "20" },
    ]);
  });

  it("rejects malformed ledger amounts instead of silently displaying a wrong total", () => {
    expect(() =>
      summarizeRunUsage([{ currency: "USD", settledAmount: "NaN", reservedAmount: null }], {
        CNY: null,
        USD: null,
      }),
    ).toThrow(RangeError);
  });
});
