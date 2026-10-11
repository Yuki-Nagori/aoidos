import { describe, expect, it, vi } from "vitest";
import { formatPeakSchedule } from "../../../src-web/utils/price-schedule";

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
