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
