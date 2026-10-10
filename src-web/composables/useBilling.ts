import { computed, onScopeDispose, ref, watch, type Ref } from "vue";
import {
  getBudgetSettings,
  getPrice,
  getSelectedPrice,
  getMonthlyPeriod,
  getRunPeriod,
  getSuggestedLimits,
  listBillingRequests,
  registerPrice,
  selectPrice,
  setBudgetSettings,
  type BudgetSettings,
  type BillingCurrency,
  type RunPeriod,
  type MonthlyPeriod,
  type BillingRequestPage,
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
) {
  const settings = ref<BudgetSettings | null>(null);
  const period = ref<RunPeriod>();
  const monthly = ref<MonthlyPeriod>();
  const requests = ref<BillingRequestPage>();
  const priceCurrency = ref<BillingCurrency>("CNY");
  const cnyLimit = ref("");
  const usdLimit = ref("");
  const unitTokens = ref("1000000");
  const inputUncached = ref("");
  const inputCached = ref("");
  const output = ref("");
  const sourceUrl = ref("");
  const priceReady = ref(false);
  const busy = ref(false);
  const error = ref<BillingFailure>();
  const routePolicy = computed(() => (providerId.value === "openrouter" ? "openrouter" : "direct"));
  let generation = 0;
  let priceSelectionGeneration = 0;
  let suppressNextCurrencySelection = false;
  let selectionQueue: Promise<void> = Promise.resolve();
  let disposed = false;
  onScopeDispose(() => {
    disposed = true;
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
      priceReady.value = false;
      busy.value = false;
      unitTokens.value = "1000000";
      inputUncached.value = "";
      inputCached.value = "";
      output.value = "";
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
          output.value = savedPrice.output;
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
        output.value = price.output;
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
    if (!runId.value || !profileId.value || busy.value) return;
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
        output: output.value,
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

  async function loadMonthly(): Promise<void> {
    try {
      monthly.value = await fetchCurrentMonth();
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
    cnyLimit,
    usdLimit,
    priceCurrency,
    unitTokens,
    inputUncached,
    inputCached,
    output,
    sourceUrl,
    priceReady,
    busy,
    error,
    saveSettings,
    savePrice,
    loadPeriod,
    loadMonthly,
    loadRequests,
  };
}
