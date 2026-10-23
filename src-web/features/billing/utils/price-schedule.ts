/** Formats the providers' fixed UTC windows in the user's selected reporting timezone. */
export function formatPeakSchedule(timeZone: string, locale: string, now = new Date()): string {
  let selectedTimeZone = timeZone;
  try {
    new Intl.DateTimeFormat(locale, { timeZone }).format(now);
  } catch {
    selectedTimeZone = "UTC";
  }

  const dateParts = new Intl.DateTimeFormat("en-CA", {
    timeZone: selectedTimeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).formatToParts(now);
  const datePart = (type: Intl.DateTimeFormatPartTypes) =>
    Number(dateParts.find((part) => part.type === type)?.value);
  const localDate = new Date(Date.UTC(datePart("year"), datePart("month") - 1, datePart("day")));
  const mondayOffset = (localDate.getUTCDay() + 6) % 7;
  const mondayUtc = localDate.getTime() - mondayOffset * 24 * 60 * 60 * 1000;
  const formatter = new Intl.DateTimeFormat(locale, {
    timeZone: selectedTimeZone,
    weekday: "short",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  });
  const formatParts = (instant: Date) => {
    const parts = formatter.formatToParts(instant);
    return {
      weekday: parts.find((part) => part.type === "weekday")?.value ?? "?",
      time: `${parts.find((part) => part.type === "hour")?.value ?? "00"}:${parts.find((part) => part.type === "minute")?.value ?? "00"}`,
    };
  };
  const windows: string[] = [];
  const utcWindows: Array<[number, number]> = [
    [1, 4],
    [6, 10],
  ];
  for (let day = 0; day < 5; day += 1) {
    for (const [startHour, endHour] of utcWindows) {
      const start = new Date(mondayUtc + (day * 24 + startHour) * 60 * 60 * 1000);
      const end = new Date(mondayUtc + (day * 24 + endHour) * 60 * 60 * 1000);
      const startParts = formatParts(start);
      const endParts = formatParts(end);
      const endDay = startParts.weekday === endParts.weekday ? "" : `${endParts.weekday} `;
      windows.push(`${startParts.weekday} ${startParts.time}–${endDay}${endParts.time}`);
    }
  }
  return windows.join(" · ");
}

const hourMs = 3_600_000;
const dayMs = 24 * hourMs;
const deepseekSchedule = "deepseek-api-utc-weekday-holiday-2026-v1";
const goSchedule = "opencode-go-deepseek-utc-weekday-v1";

/** Mirrors the versioned Rust schedule for display only; settlement remains in Rust. */
export function priceTierAt(version: string, instant: Date): "peak" | "off" | undefined {
  if (version !== deepseekSchedule && version !== goSchedule) return undefined;
  if (version === deepseekSchedule) {
    const china = new Date(instant.getTime() + 8 * hourMs);
    const month = china.getUTCMonth() + 1;
    const day = china.getUTCDate();
    const holidays: Record<number, [number, number]> = {
      1: [1, 3],
      2: [15, 23],
      4: [4, 6],
      5: [1, 5],
      6: [19, 21],
      9: [25, 27],
      10: [1, 7],
    };
    const range = holidays[month];
    if (china.getUTCFullYear() === 2026 && range && day >= range[0] && day <= range[1])
      return "off";
  }
  const weekday = instant.getUTCDay();
  const minute = instant.getUTCHours() * 60 + instant.getUTCMinutes();
  return weekday >= 1 &&
    weekday <= 5 &&
    ((minute >= 60 && minute < 240) || (minute >= 360 && minute < 600))
    ? "peak"
    : "off";
}

export function resolvePriceTimeZone(timeZone: string): string {
  try {
    new Intl.DateTimeFormat("en", { timeZone }).format();
    return timeZone;
  } catch {
    return "UTC";
  }
}

export interface PriceWeekDay {
  date: string;
  weekday: number;
  today: boolean;
  hours: Array<{ peak: number; peakStart: number; peakEnd: number; past: boolean }>;
}

/** Projects UTC instants to the selected week's wall time, preserving DST and fractional offsets. */
export function priceWeek(version: string, timeZone: string, now: Date): PriceWeekDay[] {
  const formatter = new Intl.DateTimeFormat("en-CA", {
    timeZone: resolvePriceTimeZone(timeZone),
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  });
  const partsAt = (date: Date) => {
    const parts = formatter.formatToParts(date);
    const part = (type: Intl.DateTimeFormatPartTypes) =>
      Number(parts.find((p) => p.type === type)?.value);
    return {
      date: Date.UTC(part("year"), part("month") - 1, part("day")),
      hour: part("hour"),
      minute: part("minute"),
    };
  };
  const current = partsAt(now);
  const monday = current.date - ((new Date(current.date).getUTCDay() + 6) % 7) * dayMs;
  const samples = Array.from({ length: 7 }, () =>
    Array.from({ length: 24 }, () => ({ peak: 0, count: 0, peakStart: 60, peakEnd: 0 })),
  );
  for (let instant = monday - dayMs; instant < monday + 8 * dayMs; instant += hourMs / 4) {
    const date = new Date(instant + hourMs / 8);
    const local = partsAt(date);
    const day = (local.date - monday) / dayMs;
    const cell = samples[day]?.[local.hour];
    if (cell) {
      cell.count += 1;
      if (priceTierAt(version, date) === "peak") {
        cell.peak += 1;
        cell.peakStart = Math.min(cell.peakStart, Math.floor(local.minute / 15) * 15);
        cell.peakEnd = Math.max(cell.peakEnd, (Math.floor(local.minute / 15) + 1) * 15);
      }
    }
  }
  return samples.map((hours, day) => {
    const date = monday + day * dayMs;
    return {
      date: new Date(date).toISOString().slice(0, 10),
      weekday: day,
      today: date === current.date,
      hours: hours.map((cell, hour) => ({
        peak: cell.count ? cell.peak / cell.count : 0,
        peakStart: (cell.peakStart / 60) * 100,
        peakEnd: (cell.peakEnd / 60) * 100,
        past: date < current.date || (date === current.date && hour < current.hour),
      })),
    };
  });
}

export function nextPriceTransition(version: string, now: Date): Date | undefined {
  const tier = priceTierAt(version, now);
  if (tier === undefined) return undefined;
  const start = Math.floor(now.getTime() / hourMs) * hourMs;
  for (let hour = 1; hour <= 24 * 10; hour += 1) {
    const instant = new Date(start + hour * hourMs);
    if (priceTierAt(version, instant) !== tier) return instant;
  }
  return undefined;
}

/** Compares decimal price strings without rounding or converting monetary values to Number. */
export function isDoublePrice(regular: string | null, peak: string | null): boolean {
  if (regular === null || peak === null) return false;
  const parse = (value: string): { units: bigint; scale: number } | undefined => {
    if (!/^\d+(?:\.\d+)?$/.test(value)) return undefined;
    const [whole, fraction = ""] = value.split(".");
    return { units: BigInt(`${whole}${fraction}`), scale: fraction.length };
  };
  const first = parse(regular);
  const second = parse(peak);
  if (!first || !second || first.units === 0n) return false;
  return (
    first.units * 2n * 10n ** BigInt(second.scale) === second.units * 10n ** BigInt(first.scale)
  );
}
