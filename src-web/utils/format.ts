import type { Locale } from "../api/locale";

export type SupportedCurrency = "CNY" | "USD";
export type CurrencyPrecision = 2 | 9;
export type DateFormatIssue = "invalid-date" | "invalid-time-zone";

/** 金额全程按十进制字符串运算；只借 Intl 生成分组、符号与货币位置。 */
export function formatExactCurrency(
  amount: string,
  currency: SupportedCurrency,
  locale: Locale,
  precision: CurrencyPrecision = 2,
): string {
  const match = /^(-?)(0|[1-9]\d*)(?:\.(\d+))?$/.exec(amount);
  if (!match) throw new RangeError("amount must be a canonical decimal string");

  const [, sign, whole, fraction = ""] = match;
  const scale = 10n ** BigInt(precision);
  const digits = fraction.padEnd(precision, "0").slice(0, precision);
  let minor = BigInt(whole!) * scale + BigInt(digits);
  if (fraction.length > precision && fraction[precision]! >= "5") minor += 1n;
  const negative = sign === "-" && minor !== 0n;
  const major = minor / scale;
  const fractional = (minor % scale).toString().padStart(precision, "0");
  const formatter = new Intl.NumberFormat(locale, {
    style: "currency",
    currency,
    minimumFractionDigits: precision,
    maximumFractionDigits: precision,
  });
  const signedMajor = negative ? (major === 0n ? -1n : -major) : major;
  const parts = formatter.formatToParts(signedMajor);
  let integerReplaced = false;
  return parts
    .map((part) => {
      if (part.type === "integer" && negative && major === 0n && !integerReplaced) {
        integerReplaced = true;
        return "0";
      }
      if (part.type === "fraction") return fractional;
      return part.value;
    })
    .join("");
}

/** 大整数显示使用 BigInt，排序与持久化仍使用调用方的原始值。 */
export function formatInteger(value: bigint, locale: Locale): string {
  return new Intl.NumberFormat(locale).format(value);
}

/** 日期必须给出明确 IANA 时区；错误以占位值与机器可读原因返回。 */
export function formatDate(
  value: string | Date,
  locale: Locale,
  timeZone: string,
): { text: string; issue?: DateFormatIssue } {
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) return { text: "—", issue: "invalid-date" };
  if (typeof timeZone !== "string" || !timeZone.trim()) {
    return { text: "—", issue: "invalid-time-zone" };
  }
  try {
    return {
      text: new Intl.DateTimeFormat(locale, {
        dateStyle: "medium",
        timeStyle: "short",
        timeZone,
      }).format(date),
    };
  } catch {
    return { text: "—", issue: "invalid-time-zone" };
  }
}
