import { invoke } from "@tauri-apps/api/core";

export type BillingCurrency = "CNY" | "USD";

export interface BudgetSettings {
  runId: string;
  cnyLimit: string;
  usdLimit: string;
  revision: number;
}

export interface PriceVersion {
  versionId: string;
  providerId: string;
  modelId: string;
  routePolicy: string;
  currency: BillingCurrency;
  unitTokens: number;
  inputUncached: string;
  inputCached: string;
  output: string;
  usageMappingVersion: string;
  sourceKind: string;
  sourceUrl?: string;
  checkedAt: string;
  effectiveFrom: string;
  validUntil?: string;
}

interface ModelUsageSummary {
  providerId: string;
  currency: BillingCurrency;
  modelId: string;
  settledAmount: string | null;
  reservedAmount: string | null;
  knownTokens: number | null;
  incompleteRequests: number;
  requestCount: number;
}

interface BudgetWarning {
  currency: BillingCurrency;
  thresholdPercent: number;
  reachedAt: string;
  read: boolean;
}

export interface RunPeriod {
  runId: string;
  items: ModelUsageSummary[];
  warnings: BudgetWarning[];
}

export interface MonthlyPeriod {
  month: string;
  timeZone: string;
  items: ModelUsageSummary[];
}

export interface SuggestedLimits {
  cny: string | null;
  usd: string | null;
  referenceInputTokens: number;
  referenceOutputTokens: number;
}

interface BillingRequestItem {
  requestId: string;
  profileId: string;
  providerId: string;
  modelId: string;
  currency: BillingCurrency;
  priceVersionId: string;
  dispatchAt: string;
  state: "reserved" | "settled" | "unconfirmed";
  reservedAmount: string;
  actualAmount: string | null;
  inputTotal: number | null;
  inputCached: number | null;
  outputTotal: number | null;
  reasoningIncluded: number | null;
  totalTokens: number | null;
}

export interface BillingRequestPage {
  revision: string;
  items: BillingRequestItem[];
  nextCursor?: string;
}

/** 读取每局独立设置的 CNY / USD 上限；未设置时返回 null。 */
export function getBudgetSettings(runId: string): Promise<BudgetSettings | null> {
  return invoke("budget_get_settings", { runId });
}

/** 按服务商、原币和模型分组读取单局用量。 */
export function getRunPeriod(runId: string): Promise<RunPeriod> {
  return invoke("budget_get_period", { runId });
}

/** 使用已持久化的系统时区读取自然月报表。 */
export function getMonthlyPeriod(month: string, detectedTimeZone: string): Promise<MonthlyPeriod> {
  return invoke("budget_get_monthly", { month, detectedTimeZone });
}

export function getSuggestedLimits(
  providerId: string,
  modelId: string,
  routePolicy: string,
): Promise<SuggestedLimits> {
  return invoke("budget_get_suggested_limits", { providerId, modelId, routePolicy });
}

export async function getSelectedPrice(
  profileId: string,
  providerId: string,
  modelId: string,
  routePolicy: string,
): Promise<PriceVersion | null> {
  const result = await invoke<{ price: PriceVersion | null }>("budget_get_selected_price", {
    profileId,
    providerId,
    modelId,
    routePolicy,
  });
  return result.price;
}

export function selectPrice(
  profileId: string,
  providerId: string,
  modelId: string,
  routePolicy: string,
  currency: BillingCurrency,
): Promise<PriceVersion> {
  return invoke("budget_select_price", { profileId, providerId, modelId, routePolicy, currency });
}

export function listBillingRequests(
  runId: string,
  cursor?: string,
  limit = 50,
): Promise<BillingRequestPage> {
  return invoke("budget_list_requests", { runId, cursor, limit });
}

/** 通过乐观修订校验原子保存两种原币上限。 */
export function setBudgetSettings(
  runId: string,
  cnyLimit: string,
  usdLimit: string,
  expectedRevision?: number,
): Promise<BudgetSettings> {
  return invoke("budget_set_settings", { runId, cnyLimit, usdLimit, expectedRevision });
}

/** 仅读取绑定到指定服务商路由与模型的价格。 */
export async function getPrice(
  providerId: string,
  modelId: string,
  routePolicy: string,
  currency: BillingCurrency,
): Promise<PriceVersion | null> {
  const result = await invoke<{ price: PriceVersion | null }>("budget_get_price", {
    providerId,
    modelId,
    routePolicy,
    currency,
  });
  return result.price;
}

/** 登记经用户确认的不可变服务商价格版本。 */
export function registerPrice(price: PriceVersion): Promise<PriceVersion> {
  return invoke("budget_register_price", { price });
}
