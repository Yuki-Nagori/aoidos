import type { BillingCurrency } from "../../../api/billing";

export interface RunUsageLine {
  currency: BillingCurrency;
  settledAmount: string | null;
  reservedAmount: string | null;
}

export interface RunCurrencyTotal {
  currency: BillingCurrency;
  amount: string;
  limit: string | null;
}

const NANOS_PER_UNIT = 1_000_000_000n;
const AMOUNT_PATTERN = /^(0|[1-9]\d*)(?:\.(\d{1,9}))?$/;

function nanos(amount: string): bigint {
  const match = AMOUNT_PATTERN.exec(amount);
  if (!match) throw new RangeError("billing amount is not a canonical decimal");
  return BigInt(match[1]!) * NANOS_PER_UNIT + BigInt((match[2] ?? "").padEnd(9, "0") || "0");
}

function decimal(amount: bigint): string {
  const whole = amount / NANOS_PER_UNIT;
  const fraction = (amount % NANOS_PER_UNIT).toString().padStart(9, "0").replace(/0+$/, "");
  return fraction ? `${whole}.${fraction}` : whole.toString();
}

/** 汇总本局原币用量，不做跨币换算；预留金额与已结算金额分别来自账本。 */
export function summarizeRunUsage(
  items: readonly RunUsageLine[],
  limits: Readonly<Record<BillingCurrency, string | null>>,
): RunCurrencyTotal[] {
  return (["CNY", "USD"] as const).flatMap((currency) => {
    const matching = items.filter((item) => item.currency === currency);
    const limit = limits[currency];
    if (matching.length === 0 && limit === null) return [];
    const total = matching.reduce(
      (sum, item) => sum + nanos(item.settledAmount ?? "0") + nanos(item.reservedAmount ?? "0"),
      0n,
    );
    return [{ currency, amount: decimal(total), limit }];
  });
}
