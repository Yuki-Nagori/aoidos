import type { Ref } from "vue";
import { computed, onScopeDispose, ref, watch } from "vue";
import type {
  BudgetSettings,
  BillingCurrency,
  RunPeriod,
  MonthlyPeriod,
  BillingRequestPage,
  BillingBackfillPreview,
  ProviderBalanceSnapshot,
} from "../api/billing";
import {
  getBudgetSettings,
  getPrice,
  getSelectedPrice,
  getMonthlyPeriod,
  getRunPeriod,
  getSuggestedLimits,
  listBillingRequests,
  previewBillingBackfill,
  commitBillingBackfill,
  getProviderBalance,
  retryBillingSettlement,
  registerPrice,
  selectPrice,
  setBudgetSettings,
} from "../api/billing";

export interface BillingFailure {
  code?: string;
  message: string;
}

function normalizeError(failure: unknown): BillingFailure {
  const code =
    failure && typeof failure === "object" && "code" in failure
      ? (failure as { code?: unknown }).code
      : undefined;
  const message =
    failure instanceof Error
      ? failure.message
      : typeof failure === "string"
        ? failure
        : failure && typeof failure === "object" && "message" in failure
          ? (failure as { message?: unknown }).message
          : undefined;
  return {
    ...(typeof code === "string" ? { code } : {}),
    message: typeof message === "string" ? message : "Billing operation failed",
  };
}

function isStaleRevision(failure: unknown): boolean {
  if (!failure || typeof failure !== "object") return false;
  const error = failure as { code?: unknown; detail?: unknown };
  if (error.code !== "app.bad-request" || !error.detail || typeof error.detail !== "object")
    return false;
  return (error.detail as { reason?: unknown }).reason === "staleRevision";
}

/** 管理单局双币种上限与明确登记的模型价格。 */
export function useBilling(
  runId: Ref<string | undefined>,
  profileId: Ref<string>,
  providerId: Ref<string>,
  modelId: Ref<string>,
  credentialRevision: Ref<number> = ref(0),
) {
  const settings = ref<BudgetSettings | null>(null);
  const period = ref<RunPeriod>();
  const monthly = ref<MonthlyPeriod>();
  const requests = ref<BillingRequestPage>();
  const retryingRequestId = ref<string>();
  const backfillPreview = ref<BillingBackfillPreview | null>(null);
  const backfillingRequestId = ref<string>();
  const providerBalance = ref<ProviderBalanceSnapshot>();
  const balanceBusy = ref(false);
  const priceCurrency = ref<BillingCurrency>("CNY");
  const cnyLimit = ref("");
  const usdLimit = ref("");
  const unitTokens = ref("1000000");
  const inputUncached = ref("");
  const inputCached = ref("");
  const inputCachedWrite = ref("");
  const output = ref("");
  const peakInputUncached = ref("");
  const peakInputCached = ref("");
  const peakInputCachedWrite = ref("");
  const peakOutput = ref("");
  const isDeepseekApiSchedule = computed(
    () =>
      providerId.value === "deepseek" &&
      ["deepseek-flash", "deepseek-v4-pro"].includes(modelId.value),
  );
  const requiresScheduledPrice = computed(
    () =>
      isDeepseekApiSchedule.value ||
      (providerId.value === "opencode-go" &&
        [
          "deepseek-v4.1-flash",
          "deepseek-v4-pro",
          "deepseek-v4-flash",
          "deepseek-v4-flash-vision-exp",
        ].includes(modelId.value)),
  );
  const scheduledPriceComplete = computed(
    () =>
      !requiresScheduledPrice.value ||
      [peakInputUncached, peakInputCached, peakInputCachedWrite, peakOutput].every((value) =>
        value.value.trim(),
      ),
  );
  const sourceUrl = ref("");
  const priceReady = ref(false);
  const busy = ref(false);
  const error = ref<BillingFailure>();
  const routePolicy = computed(() => (providerId.value === "openrouter" ? "openrouter" : "direct"));
  let generation = 0;
  let balanceGeneration = 0;
  let priceSelectionGeneration = 0;
  let suppressNextCurrencySelection = false;
  let selectionQueue: Promise<void> = Promise.resolve();
  let disposed = false;
  onScopeDispose(() => {
    disposed = true;
  });

  watch(credentialRevision, () => {
    balanceGeneration += 1;
    providerBalance.value = undefined;
    balanceBusy.value = false;
    error.value = undefined;
  });

  watch(
    [runId, profileId, providerId, modelId],
    async ([activeRun, activeProfile, provider, model]) => {
      const current = ++generation;
      priceSelectionGeneration += 1;
      const selectionAtLoad = priceSelectionGeneration;
      settings.value = null;
      period.value = undefined;
      monthly.value = undefined;
      requests.value = undefined;
      backfillPreview.value = null;
      backfillingRequestId.value = undefined;
      providerBalance.value = undefined;
      balanceBusy.value = false;
      priceReady.value = false;
      busy.value = false;
      unitTokens.value = "1000000";
      inputUncached.value = "";
      inputCached.value = "";
      inputCachedWrite.value = "";
      output.value = "";
      peakInputUncached.value = "";
      peakInputCached.value = "";
      peakInputCachedWrite.value = "";
      peakOutput.value = "";
      sourceUrl.value = "";
      error.value = undefined;
      if (!activeRun) return;
      try {
        const [savedSettings, savedPrice, savedPeriod, savedMonthly, suggestions, requestPage] =
          await Promise.all([
            getBudgetSettings(activeRun),
            getSelectedPrice(activeProfile, provider, model, routePolicy.value),
            getRunPeriod(activeRun),
            fetchCurrentMonth(),
            getSuggestedLimits(provider, model, routePolicy.value),
            listBillingRequests(activeRun),
          ]);
        if (disposed || current !== generation) return;
        settings.value = savedSettings;
        period.value = savedPeriod;
        monthly.value = savedMonthly;
        requests.value = requestPage;
        cnyLimit.value = savedSettings?.cnyLimit ?? suggestions.cny ?? "";
        usdLimit.value = savedSettings?.usdLimit ?? suggestions.usd ?? "";
        if (savedPrice && selectionAtLoad === priceSelectionGeneration) {
          suppressNextCurrencySelection = savedPrice.currency !== priceCurrency.value;
          priceCurrency.value = savedPrice.currency;
          unitTokens.value = String(savedPrice.unitTokens);
          inputUncached.value = savedPrice.inputUncached;
          inputCached.value = savedPrice.inputCached;
          inputCachedWrite.value = savedPrice.inputCachedWrite ?? "";
          output.value = savedPrice.output;
          peakInputUncached.value = savedPrice.schedule?.peak.inputUncached ?? "";
          peakInputCached.value = savedPrice.schedule?.peak.inputCached ?? "";
          peakInputCachedWrite.value = savedPrice.schedule?.peak.inputCachedWrite ?? "";
          peakOutput.value = savedPrice.schedule?.peak.output ?? "";
          sourceUrl.value = savedPrice.sourceUrl ?? "";
          priceReady.value = true;
        }
      } catch (failure) {
        if (!disposed && current === generation) error.value = normalizeError(failure);
      }
    },
    { immediate: true },
  );

  watch(
    priceCurrency,
    async (currency) => {
      if (suppressNextCurrencySelection) {
        suppressNextCurrencySelection = false;
        return;
      }
      const activeRun = runId.value;
      const activeProfile = profileId.value;
      if (!activeRun || !activeProfile) return;
      const current = generation;
      const selection = ++priceSelectionGeneration;
      const provider = providerId.value;
      const model = modelId.value;
      const route = routePolicy.value;
      const isCurrent = () =>
        !disposed &&
        current === generation &&
        selection === priceSelectionGeneration &&
        activeRun === runId.value &&
        activeProfile === profileId.value &&
        provider === providerId.value &&
        model === modelId.value &&
        route === routePolicy.value &&
        currency === priceCurrency.value;
      try {
        priceReady.value = false;
        const price = await getPrice(provider, model, route, currency);
        if (!isCurrent()) return;
        if (!price) {
          priceReady.value = false;
          return;
        }
        const selectionRequest = selectionQueue.then(async () => {
          if (!isCurrent()) return false;
          await selectPrice(activeProfile, provider, model, route, currency);
          return true;
        });
        selectionQueue = selectionRequest.then(
          () => undefined,
          () => undefined,
        );
        if (!(await selectionRequest) || !isCurrent()) return;
        priceReady.value = true;
        unitTokens.value = String(price.unitTokens);
        inputUncached.value = price.inputUncached;
        inputCached.value = price.inputCached;
        inputCachedWrite.value = price.inputCachedWrite ?? "";
        output.value = price.output;
        peakInputUncached.value = price.schedule?.peak.inputUncached ?? "";
        peakInputCached.value = price.schedule?.peak.inputCached ?? "";
        peakInputCachedWrite.value = price.schedule?.peak.inputCachedWrite ?? "";
        peakOutput.value = price.schedule?.peak.output ?? "";
        sourceUrl.value = price.sourceUrl ?? "";
      } catch (failure) {
        if (isCurrent()) {
          priceReady.value = false;
          error.value = normalizeError(failure);
        }
      }
    },
    { flush: "sync" },
  );

  async function saveSettings(): Promise<void> {
    if (!runId.value || busy.value) return;
    const activeRun = runId.value;
    const current = generation;
    const isCurrent = () => !disposed && current === generation && activeRun === runId.value;
    busy.value = true;
    error.value = undefined;
    try {
      const saved = await setBudgetSettings(
        activeRun,
        cnyLimit.value,
        usdLimit.value,
        settings.value?.revision,
      );
      if (!isCurrent()) return;
      settings.value = saved;
      await loadPeriodFor(activeRun, current);
    } catch (failure) {
      if (isCurrent()) error.value = normalizeError(failure);
    } finally {
      if (isCurrent()) busy.value = false;
    }
  }

  async function savePrice(): Promise<void> {
    if (!runId.value || !profileId.value || busy.value || !scheduledPriceComplete.value) return;
    const activeRun = runId.value;
    const activeProfile = profileId.value;
    const provider = providerId.value;
    const model = modelId.value;
    const route = routePolicy.value;
    const currency = priceCurrency.value;
    const current = generation;
    const selection = priceSelectionGeneration;
    const isCurrent = () =>
      !disposed &&
      current === generation &&
      selection === priceSelectionGeneration &&
      activeRun === runId.value &&
      activeProfile === profileId.value &&
      provider === providerId.value &&
      model === modelId.value &&
      route === routePolicy.value &&
      currency === priceCurrency.value;
    busy.value = true;
    error.value = undefined;
    const checkedAt = new Date().toISOString();
    const hasGoPeakPrice = requiresScheduledPrice.value;
    try {
      await registerPrice({
        versionId: crypto.randomUUID(),
        providerId: provider,
        modelId: model,
        routePolicy: route,
        currency,
        unitTokens: Number(unitTokens.value),
        inputUncached: inputUncached.value,
        inputCached: inputCached.value,
        inputCachedWrite: inputCachedWrite.value || null,
        output: output.value,
        schedule: hasGoPeakPrice
          ? {
              version: isDeepseekApiSchedule.value
                ? "deepseek-api-utc-weekday-holiday-2026-v1"
                : "opencode-go-deepseek-utc-weekday-v1",
              peak: {
                inputUncached: peakInputUncached.value,
                inputCached: peakInputCached.value,
                inputCachedWrite: peakInputCachedWrite.value,
                output: peakOutput.value,
              },
            }
          : null,
        usageMappingVersion: "provider-usage-v1",
        sourceKind: "user-entered",
        ...(sourceUrl.value ? { sourceUrl: sourceUrl.value } : {}),
        checkedAt,
        effectiveFrom: checkedAt,
      });
      if (!isCurrent()) return;
      const selectionRequest = selectionQueue.then(async () => {
        if (!isCurrent()) return false;
        await selectPrice(activeProfile, provider, model, route, currency);
        return true;
      });
      selectionQueue = selectionRequest.then(
        () => undefined,
        () => undefined,
      );
      if ((await selectionRequest) && isCurrent()) priceReady.value = true;
    } catch (failure) {
      if (isCurrent()) error.value = normalizeError(failure);
    } finally {
      if (!disposed && current === generation) busy.value = false;
    }
  }

  async function loadPeriod(): Promise<void> {
    if (!runId.value) return;
    await loadPeriodFor(runId.value, generation);
  }

  async function loadPeriodFor(activeRun: string, current: number): Promise<void> {
    try {
      const result = await getRunPeriod(activeRun);
      if (!disposed && current === generation && activeRun === runId.value) period.value = result;
    } catch (failure) {
      if (!disposed && current === generation && activeRun === runId.value)
        error.value = normalizeError(failure);
    }
  }

  async function loadRequests(reset = false): Promise<void> {
    if (!runId.value) return;
    const activeRun = runId.value;
    const current = generation;
    const cursor = reset ? undefined : requests.value?.nextCursor;
    const isCurrent = () => !disposed && current === generation && activeRun === runId.value;
    try {
      const page = await listBillingRequests(activeRun, cursor);
      if (!isCurrent()) return;
      requests.value =
        reset || !requests.value
          ? page
          : { ...page, items: [...requests.value.items, ...page.items] };
    } catch (failure) {
      if (!isCurrent()) return;
      if (isStaleRevision(failure) && !reset) {
        await loadRequests(true);
      } else {
        error.value = normalizeError(failure);
      }
    }
  }

  async function retrySettlement(requestId: string): Promise<void> {
    const activeRun = runId.value;
    if (!activeRun || retryingRequestId.value) return;
    const current = generation;
    error.value = undefined;
    retryingRequestId.value = requestId;
    try {
      await retryBillingSettlement(activeRun, requestId);
      if (disposed || current !== generation || activeRun !== runId.value) return;
      await Promise.all([loadRequests(true), loadPeriodFor(activeRun, current)]);
    } catch (failure) {
      if (!disposed && current === generation && activeRun === runId.value) {
        error.value = normalizeError(failure);
        await loadRequests(true);
      }
    } finally {
      retryingRequestId.value = undefined;
    }
  }

  async function previewBackfill(requestId: string): Promise<void> {
    const activeRun = runId.value;
    if (!activeRun || busy.value || backfillingRequestId.value) return;
    const current = generation;
    error.value = undefined;
    backfillPreview.value = null;
    backfillingRequestId.value = requestId;
    try {
      const preview = await previewBillingBackfill(activeRun, requestId);
      if (disposed || current !== generation || activeRun !== runId.value) return;
      backfillPreview.value = preview;
    } catch (failure) {
      if (!disposed && current === generation && activeRun === runId.value) {
        error.value = normalizeError(failure);
      }
    } finally {
      if (!disposed && current === generation) backfillingRequestId.value = undefined;
    }
  }

  async function commitBackfill(): Promise<void> {
    const preview = backfillPreview.value;
    const activeRun = runId.value;
    if (!preview || !activeRun || backfillingRequestId.value) return;
    const current = generation;
    error.value = undefined;
    backfillingRequestId.value = preview.requestId;
    try {
      await commitBillingBackfill(
        activeRun,
        preview.requestId,
        crypto.randomUUID(),
        preview.ledgerRevision,
      );
      if (disposed || current !== generation || activeRun !== runId.value) return;
      backfillPreview.value = null;
      await Promise.all([loadRequests(true), loadPeriodFor(activeRun, current)]);
    } catch (failure) {
      if (!disposed && current === generation && activeRun === runId.value) {
        error.value = normalizeError(failure);
        backfillPreview.value = null;
        await loadRequests(true);
      }
    } finally {
      if (!disposed && current === generation) backfillingRequestId.value = undefined;
    }
  }

  async function queryProviderBalance(): Promise<void> {
    if (providerId.value !== "deepseek" || !profileId.value || balanceBusy.value) return;
    const current = generation;
    const currentBalance = balanceGeneration;
    const activeCredentialRevision = credentialRevision.value;
    const activeProfile = profileId.value;
    balanceBusy.value = true;
    error.value = undefined;
    try {
      const snapshot = await getProviderBalance(activeProfile);
      if (
        disposed ||
        current !== generation ||
        currentBalance !== balanceGeneration ||
        activeCredentialRevision !== credentialRevision.value ||
        activeProfile !== profileId.value
      )
        return;
      providerBalance.value = snapshot;
    } catch (failure) {
      if (
        !disposed &&
        current === generation &&
        currentBalance === balanceGeneration &&
        activeCredentialRevision === credentialRevision.value &&
        activeProfile === profileId.value
      ) {
        error.value = normalizeError(failure);
      }
    } finally {
      if (!disposed && current === generation && currentBalance === balanceGeneration)
        balanceBusy.value = false;
    }
  }

  async function loadMonthly(): Promise<void> {
    try {
      const current = await fetchCurrentMonth();
      monthly.value = current;
    } catch (failure) {
      if (!disposed) error.value = normalizeError(failure);
    }
  }

  function fetchCurrentMonth(): Promise<MonthlyPeriod> {
    const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
    const parts = new Intl.DateTimeFormat("en-CA", {
      timeZone,
      year: "numeric",
      month: "2-digit",
    }).formatToParts(new Date());
    const year = parts.find((part) => part.type === "year")?.value;
    const month = parts.find((part) => part.type === "month")?.value;
    if (!year || !month) throw new RangeError("system month could not be determined");
    return getMonthlyPeriod(`${year}-${month}`, timeZone);
  }

  return {
    settings,
    period,
    monthly,
    requests,
    retryingRequestId,
    backfillPreview,
    backfillingRequestId,
    providerBalance,
    balanceBusy,
    cnyLimit,
    usdLimit,
    priceCurrency,
    unitTokens,
    inputUncached,
    inputCached,
    inputCachedWrite,
    output,
    requiresScheduledPrice,
    isDeepseekApiSchedule,
    peakInputUncached,
    peakInputCached,
    peakInputCachedWrite,
    peakOutput,
    scheduledPriceComplete,
    sourceUrl,
    priceReady,
    busy,
    error,
    saveSettings,
    savePrice,
    loadPeriod,
    loadMonthly,
    loadRequests,
    retrySettlement,
    previewBackfill,
    commitBackfill,
    queryProviderBalance,
  };
}
