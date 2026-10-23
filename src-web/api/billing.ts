import { invoke } from "@tauri-apps/api/core";

export type BillingCurrency = "CNY" | "USD";

interface TokenPriceTier {
  inputUncached: string;
  inputCached: string;
  inputCachedWrite: string | null;
  output: string;
}

interface RecurringPriceSchedule {
  version: string;
  peak: TokenPriceTier;
}

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
  inputCachedWrite: string | null;
  output: string;
  schedule?: RecurringPriceSchedule | null;
  usageMappingVersion: string;
  sourceKind: string;
  sourceUrl?: string;
  checkedAt: string;
  effectiveFrom: string;
  validUntil?: string;
}

/** 用户确认 DeepSeek 账户币种后，将内置价登记并绑定到 profile。 */
export function confirmDeepseekPrice(
  profileId: string,
  modelId: string,
  currency: BillingCurrency,
): Promise<PriceVersion> {
  return invoke("budget_confirm_deepseek_price", { profileId, modelId, currency });
}

/** 读取随应用发布的 DeepSeek 价格参考；不会访问账本或发送网络请求。 */
export function getDeepseekPriceTemplates(): Promise<PriceVersion[]> {
  return invoke("budget_get_deepseek_price_templates");
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
  tierItems?: TierUsageSummary[];
  hasEstimatedRequests?: boolean;
}

export type PriceTier = "regular" | "peak" | "off";
export interface TierUsageSummary {
  currency: BillingCurrency;
  tier: PriceTier;
  settledAmount: string;
  reservedAmount: string;
  requestCount: number;
  estimatedRequests: number;
}

export interface MonthlyPeriod {
  month: string;
  timeZone: string;
  items: ModelUsageSummary[];
}

interface BillingRequestItem {
  requestId: string;
  profileId: string;
  providerId: string;
  modelId: string;
  currency: BillingCurrency;
  priceVersionId: string;
  dispatchAt: string;
  priceTier?: PriceTier;
  estimated?: boolean;
  state: "reserved" | "settled" | "unconfirmed";
  settlementPending: boolean;
  diagnosticCode: string | null;
  reservedAmount: string;
  actualAmount: string | null;
  inputTotal: number | null;
  inputCached: number | null;
  inputCachedWrite: number | null;
  outputTotal: number | null;
  reasoningIncluded: number | null;
  totalTokens: number | null;
}

export interface BillingRequestPage {
  revision: string;
  items: BillingRequestItem[];
  nextCursor?: string;
}

export interface BillingBackfillPreview {
  requestId: string;
  runId: string;
  providerId: string;
  modelId: string;
  currency: BillingCurrency;
  priceVersionId: string;
  estimatedAmount: string;
  ledgerRevision: number;
}

export interface ProviderBalanceSnapshot {
  providerId: "deepseek";
  queriedAt: string;
  stale: boolean;
  status: "available" | "unavailable" | "unknown";
  accounts: Array<{
    currency: string;
    total: string;
    granted: string;
    toppedUp: string;
  }>;
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

/** 重试本地暂存的数值用量，不会重新发送供应商请求。 */
export function retryBillingSettlement(runId: string, requestId: string): Promise<void> {
  return invoke("budget_retry_settlement", { runId, requestId });
}

export function previewBillingBackfill(
  runId: string,
  requestId: string,
): Promise<BillingBackfillPreview | null> {
  return invoke("budget_preview_backfill", { runId, requestId });
}

export function commitBillingBackfill(
  runId: string,
  requestId: string,
  operationId: string,
  expectedRevision: number,
): Promise<void> {
  return invoke("budget_commit_backfill", { runId, requestId, operationId, expectedRevision });
}

/** 仅在玩家显式请求时查询当前支持的供应商余额。 */
export function getProviderBalance(profileId: string): Promise<ProviderBalanceSnapshot> {
  return invoke("budget_get_provider_balance", { profileId });
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
