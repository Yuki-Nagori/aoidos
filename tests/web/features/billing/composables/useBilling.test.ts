import { effectScope, nextTick, ref } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  BillingRequestPage,
  BillingBackfillPreview,
  BudgetSettings,
  MonthlyPeriod,
  PriceVersion,
  RunPeriod,
  ProviderBalanceSnapshot,
} from "../../../../../src-web/api/billing";
import {
  getBudgetSettings,
  getMonthlyPeriod,
  getPrice,
  getRunPeriod,
  getSelectedPrice,
  getProviderBalance,
  listBillingRequests,
  previewBillingBackfill,
  commitBillingBackfill,
  retryBillingSettlement,
  registerPrice,
  selectPrice,
  setBudgetSettings,
} from "../../../../../src-web/api/billing";
import { useBilling } from "../../../../../src-web/features/billing/composables/useBilling";

vi.mock("../../../../../src-web/api/billing", () => ({
  getBudgetSettings: vi.fn(),
  getMonthlyPeriod: vi.fn(),
  getPrice: vi.fn(),
  getRunPeriod: vi.fn(),
  getSelectedPrice: vi.fn(),
  getProviderBalance: vi.fn(),
  listBillingRequests: vi.fn(),
  previewBillingBackfill: vi.fn(),
  commitBillingBackfill: vi.fn(),
  retryBillingSettlement: vi.fn(),
  registerPrice: vi.fn(),
  selectPrice: vi.fn(),
  setBudgetSettings: vi.fn(),
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((accept, fail) => {
    resolve = accept;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function settings(overrides: Partial<BudgetSettings> = {}): BudgetSettings {
  return { runId: "run-1", cnyLimit: "10", usdLimit: "2", revision: 7, ...overrides };
}

function price(overrides: Partial<PriceVersion> = {}): PriceVersion {
  return {
    versionId: "price-1",
    providerId: "provider-1",
    modelId: "model-1",
    routePolicy: "direct",
    currency: "CNY",
    unitTokens: 1_000_000,
    inputUncached: "1",
    inputCached: "0.5",
    inputCachedWrite: "0.25",
    output: "2",
    usageMappingVersion: "provider-usage-v1",
    sourceKind: "user-entered",
    checkedAt: "2026-10-11T00:00:00.000Z",
    effectiveFrom: "2026-10-11T00:00:00.000Z",
    ...overrides,
  };
}

function runPeriod(runId = "run-1"): RunPeriod {
  return { runId, items: [], warnings: [] };
}

function monthPeriod(): MonthlyPeriod {
  return { month: "2026-10", timeZone: "Asia/Shanghai", items: [] };
}

function requestPage(revision = "1", nextCursor?: string): BillingRequestPage {
  return { revision, items: [], ...(nextCursor ? { nextCursor } : {}) };
}

function balanceSnapshot(): ProviderBalanceSnapshot {
  return {
    providerId: "deepseek",
    queriedAt: "2026-10-11T00:00:00Z",
    stale: false,
    status: "available",
    accounts: [{ currency: "CNY", total: "10", granted: "0", toppedUp: "10" }],
  };
}

function createState(initialRun: string | null = "run-1", initialProvider = "provider-1") {
  const runId = ref<string | undefined>(initialRun ?? undefined);
  const profileId = ref("profile-1");
  const providerId = ref(initialProvider);
  const modelId = ref("model-1");
  const credentialRevision = ref(0);
  const scope = effectScope();
  const state = scope.run(() =>
    useBilling(runId, profileId, providerId, modelId, credentialRevision),
  )!;
  return { state, runId, profileId, providerId, modelId, credentialRevision, scope };
}

async function waitForLoaded(state: ReturnType<typeof createState>["state"]) {
  await vi.waitFor(() => expect(state.period.value).toBeDefined());
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getBudgetSettings).mockResolvedValue(null);
  vi.mocked(getMonthlyPeriod).mockResolvedValue(monthPeriod());
  vi.mocked(getPrice).mockResolvedValue(null);
  vi.mocked(getRunPeriod).mockImplementation(async (id) => runPeriod(id));
  vi.mocked(getSelectedPrice).mockResolvedValue(null);
  vi.mocked(getProviderBalance).mockResolvedValue(balanceSnapshot());
  vi.mocked(listBillingRequests).mockResolvedValue(requestPage());
  vi.mocked(previewBillingBackfill).mockResolvedValue(null);
  vi.mocked(commitBillingBackfill).mockResolvedValue();
  vi.mocked(retryBillingSettlement).mockResolvedValue();
  vi.mocked(registerPrice).mockResolvedValue(price());
  vi.mocked(selectPrice).mockResolvedValue(price());
  vi.mocked(setBudgetSettings).mockImplementation(async (runId, cnyLimit, usdLimit) =>
    settings({ runId, cnyLimit, usdLimit }),
  );
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("useBilling initial loading", () => {
  it("queries provider balance only for an explicitly selected DeepSeek profile", async () => {
    const { state, scope, providerId } = createState("run-1", "opencode-zen");
    await waitForLoaded(state);
    await state.queryProviderBalance();
    expect(getProviderBalance).not.toHaveBeenCalled();
    providerId.value = "deepseek";
    await nextTick();
    await waitForLoaded(state);
    await state.queryProviderBalance();
    expect(getProviderBalance).toHaveBeenCalledOnce();
    expect(state.providerBalance.value).toEqual(balanceSnapshot());
    scope.stop();
  });

  it("ignores a balance response that arrives after the billing view is disposed", async () => {
    const pending = deferred<ProviderBalanceSnapshot>();
    vi.mocked(getProviderBalance).mockReturnValueOnce(pending.promise);
    const { state, scope } = createState("run-1", "deepseek");
    await waitForLoaded(state);
    const querying = state.queryProviderBalance();
    scope.stop();
    pending.resolve(balanceSnapshot());
    await querying;
    expect(state.providerBalance.value).toBeUndefined();
  });

  it("loads saved settings, selected exact-route price, periods and requests", async () => {
    const selected = price({
      currency: "USD",
      routePolicy: "openrouter",
      unitTokens: 1000,
      sourceUrl: "https://prices.example/model",
    });
    vi.mocked(getBudgetSettings).mockResolvedValue(settings({ cnyLimit: "12", usdLimit: "3" }));
    vi.mocked(getSelectedPrice).mockResolvedValue(selected);
    vi.mocked(getPrice).mockResolvedValue(selected);
    vi.mocked(listBillingRequests).mockResolvedValue(requestPage("8", "cursor-2"));
    const { state, scope } = createState("run-1", "openrouter");

    await waitForLoaded(state);

    expect(state.settings.value?.revision).toBe(7);
    expect(state.cnyLimit.value).toBe("12");
    expect(state.usdLimit.value).toBe("3");
    expect(state.priceCurrency.value).toBe("USD");
    expect(state.unitTokens.value).toBe("1000");
    expect(state.inputUncached.value).toBe("1");
    expect(state.inputCached.value).toBe("0.5");
    expect(state.inputCachedWrite.value).toBe("0.25");
    expect(state.output.value).toBe("2");
    expect(state.sourceUrl.value).toBe("https://prices.example/model");
    expect(state.priceReady.value).toBe(true);
    expect(state.requests.value?.nextCursor).toBe("cursor-2");
    expect(getSelectedPrice).toHaveBeenCalledWith(
      "profile-1",
      "openrouter",
      "model-1",
      "openrouter",
    );
    expect(getMonthlyPeriod).toHaveBeenCalledWith(
      expect.stringMatching(/^\d{4}-\d{2}$/),
      expect.any(String),
    );
    scope.stop();
  });

  it("persists the default CNY 10 and USD 5 caps for an empty run", async () => {
    const { state, scope } = createState();
    await waitForLoaded(state);
    await vi.waitFor(() => expect(state.ready.value).toBe(true));
    expect(state.cnyLimit.value).toBe("10");
    expect(state.usdLimit.value).toBe("5");
    expect(state.settings.value?.cnyLimit).toBe("10");
    expect(state.settings.value?.usdLimit).toBe("5");
    expect(setBudgetSettings).toHaveBeenCalledWith("run-1", "10", "5", undefined);
    expect(state.priceReady.value).toBe(false);
    expect(state.sourceUrl.value).toBe("");
    expect(getSelectedPrice).toHaveBeenCalledWith("profile-1", "provider-1", "model-1", "direct");
    scope.stop();
  });

  it("does not replace a saved historical price with the newest price on load", async () => {
    vi.mocked(getSelectedPrice).mockResolvedValue(
      price({ currency: "USD", versionId: "price-v1" }),
    );
    vi.mocked(getPrice).mockResolvedValue(price({ currency: "USD", versionId: "price-v2" }));
    const { state, scope } = createState();

    await waitForLoaded(state);

    expect(state.priceCurrency.value).toBe("USD");
    expect(state.unitTokens.value).toBe("1000000");
    expect(selectPrice).not.toHaveBeenCalled();
    scope.stop();
  });

  it("uses fixed default caps for a run without saved settings", async () => {
    vi.mocked(getSelectedPrice).mockResolvedValue(price({ sourceUrl: undefined }));
    const { state, scope } = createState();
    await waitForLoaded(state);
    await vi.waitFor(() => expect(state.ready.value).toBe(true));
    expect(state.cnyLimit.value).toBe("10");
    expect(state.usdLimit.value).toBe("5");
    expect(state.settings.value?.cnyLimit).toBe("10");
    expect(state.settings.value?.usdLimit).toBe("5");
    expect(state.sourceUrl.value).toBe("");
    expect(state.priceReady.value).toBe(true);
    scope.stop();
  });

  it("loads a legacy saved price without a cache-write rate as an empty field", async () => {
    vi.mocked(getSelectedPrice).mockResolvedValue(price({ inputCachedWrite: null }));
    const { state, scope } = createState();

    await waitForLoaded(state);

    expect(state.inputCachedWrite.value).toBe("");
    scope.stop();
  });

  it("does not load when there is no active run", async () => {
    const { scope } = createState(null);
    await nextTick();
    expect(getBudgetSettings).not.toHaveBeenCalled();
    expect(getRunPeriod).not.toHaveBeenCalled();
    expect(listBillingRequests).not.toHaveBeenCalled();
    scope.stop();
  });

  it("does not let a previous run's delayed load overwrite the current run", async () => {
    const pending = deferred<BudgetSettings | null>();
    vi.mocked(getBudgetSettings).mockReturnValueOnce(pending.promise);
    const { state, runId, scope } = createState();
    runId.value = "run-2";
    await nextTick();
    await waitForLoaded(state);
    await vi.waitFor(() => expect(state.ready.value).toBe(true));
    expect(state.period.value?.runId).toBe("run-2");

    pending.resolve(settings({ runId: "run-1", cnyLimit: "999" }));
    await Promise.resolve();
    await Promise.resolve();
    expect(state.settings.value?.runId).toBe("run-2");
    expect(state.cnyLimit.value).toBe("10");
    expect(state.usdLimit.value).toBe("5");
    scope.stop();
  });

  it("keeps a user's currency selection when the saved price load finishes late", async () => {
    const savedPrice = deferred<PriceVersion | null>();
    vi.mocked(getSelectedPrice).mockReturnValue(savedPrice.promise);
    vi.mocked(getPrice).mockResolvedValue(price({ currency: "USD", unitTokens: 500 }));
    const { state, scope } = createState();

    state.priceCurrency.value = "USD";
    await vi.waitFor(() =>
      expect(selectPrice).toHaveBeenCalledWith(
        "profile-1",
        "provider-1",
        "model-1",
        "direct",
        "USD",
      ),
    );
    savedPrice.resolve(price({ currency: "CNY", unitTokens: 900 }));

    await waitForLoaded(state);
    expect(state.priceCurrency.value).toBe("USD");
    expect(state.unitTokens.value).toBe("500");
    expect(state.priceReady.value).toBe(true);
    expect(selectPrice).toHaveBeenCalledTimes(1);
    scope.stop();
  });

  it("ignores a delayed error from an obsolete run", async () => {
    const pending = deferred<BudgetSettings | null>();
    vi.mocked(getBudgetSettings).mockReturnValueOnce(pending.promise);
    const { state, runId, scope } = createState();
    runId.value = "run-2";
    await nextTick();
    await waitForLoaded(state);
    pending.reject(new Error("obsolete failure"));
    await Promise.resolve();
    await Promise.resolve();
    expect(state.error.value?.message).toBeUndefined();
    scope.stop();
  });

  it("ignores an initial load that resolves after scope disposal", async () => {
    const pending = deferred<BudgetSettings | null>();
    vi.mocked(getBudgetSettings).mockReturnValueOnce(pending.promise);
    const { state, scope } = createState();
    scope.stop();
    pending.resolve(settings());
    await Promise.resolve();
    await Promise.resolve();
    expect(state.settings.value).toBeNull();
  });

  it.each([
    [new Error("network detail"), "network detail"],
    ["plain failure", "plain failure"],
    [{ message: "structured failure" }, "structured failure"],
    [{ detail: "private" }, "Billing operation failed"],
    [{ message: 42 }, "Billing operation failed"],
  ])("normalizes initial load errors", async (failure, expected) => {
    vi.mocked(getBudgetSettings).mockRejectedValueOnce(failure);
    const { state, scope } = createState();
    await vi.waitFor(() => expect(state.error.value?.message).toBe(expected));
    scope.stop();
  });
});

describe("useBilling price selection and registration", () => {
  it("loads and selects a registered price for the chosen currency", async () => {
    const registered = price({
      currency: "USD",
      unitTokens: 500,
      inputCachedWrite: null,
      sourceUrl: undefined,
    });
    vi.mocked(getPrice).mockResolvedValue(registered);
    const { state, scope } = createState();
    await waitForLoaded(state);

    state.priceCurrency.value = "USD";
    await vi.waitFor(() => expect(state.priceReady.value).toBe(true));

    expect(getPrice).toHaveBeenCalledWith("provider-1", "model-1", "direct", "USD");
    expect(selectPrice).toHaveBeenCalledWith("profile-1", "provider-1", "model-1", "direct", "USD");
    expect(state.unitTokens.value).toBe("500");
    expect(state.inputCachedWrite.value).toBe("");
    expect(state.sourceUrl.value).toBe("");
    scope.stop();
  });

  it("marks a currency without a registered price as not ready", async () => {
    const { state, scope } = createState();
    await waitForLoaded(state);
    state.priceReady.value = true;
    state.priceCurrency.value = "USD";
    await vi.waitFor(() => expect(getPrice).toHaveBeenCalled());
    await nextTick();
    expect(state.priceReady.value).toBe(false);
    expect(selectPrice).not.toHaveBeenCalled();
    scope.stop();
  });

  it("does not mark a price ready when persisting its selection fails", async () => {
    vi.mocked(getPrice).mockResolvedValue(price({ currency: "USD" }));
    vi.mocked(selectPrice).mockRejectedValueOnce(new Error("selection failed"));
    const { state, scope } = createState();
    await waitForLoaded(state);

    state.priceCurrency.value = "USD";
    await vi.waitFor(() => expect(state.error.value?.message).toBe("selection failed"));

    expect(state.priceReady.value).toBe(false);
    scope.stop();
  });

  it("serializes rapid currency selections and leaves the newest choice active", async () => {
    const selectingUsd = deferred<PriceVersion>();
    vi.mocked(getPrice).mockImplementation(async (_provider, _model, _route, currency) =>
      price({ currency }),
    );
    vi.mocked(selectPrice).mockImplementation(
      async (_profile, _provider, _model, _route, currency) => {
        if (currency === "USD") return selectingUsd.promise;
        return price({ currency });
      },
    );
    const { state, scope } = createState();
    await waitForLoaded(state);

    state.priceCurrency.value = "USD";
    await vi.waitFor(() =>
      expect(selectPrice).toHaveBeenCalledWith(
        "profile-1",
        "provider-1",
        "model-1",
        "direct",
        "USD",
      ),
    );
    state.priceCurrency.value = "CNY";
    await nextTick();
    expect(selectPrice).toHaveBeenCalledTimes(1);

    selectingUsd.resolve(price({ currency: "USD" }));
    await vi.waitFor(() => expect(selectPrice).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(state.unitTokens.value).toBe("1000000"));
    expect(selectPrice).toHaveBeenLastCalledWith(
      "profile-1",
      "provider-1",
      "model-1",
      "direct",
      "CNY",
    );
    scope.stop();
  });

  it("skips an obsolete queued currency selection before selecting the current model", async () => {
    const selectingUsd = deferred<PriceVersion>();
    vi.mocked(getPrice).mockImplementation(async (_provider, model, _route, currency) =>
      price({ modelId: model, currency }),
    );
    vi.mocked(selectPrice).mockReturnValueOnce(selectingUsd.promise);
    const { state, modelId, scope } = createState();
    await waitForLoaded(state);

    state.priceCurrency.value = "USD";
    await vi.waitFor(() => expect(selectPrice).toHaveBeenCalledTimes(1));
    state.priceCurrency.value = "CNY";
    await nextTick();
    expect(getPrice).toHaveBeenCalledTimes(2);
    expect(selectPrice).toHaveBeenCalledTimes(1);

    modelId.value = "model-2";
    await nextTick();
    await waitForLoaded(state);
    state.priceCurrency.value = "USD";
    await nextTick();
    selectingUsd.resolve(price({ currency: "USD" }));

    await vi.waitFor(() => expect(selectPrice).toHaveBeenCalledTimes(2));
    expect(selectPrice).toHaveBeenLastCalledWith(
      "profile-1",
      "provider-1",
      "model-2",
      "direct",
      "USD",
    );
    expect(selectPrice).not.toHaveBeenCalledWith(
      "profile-1",
      "provider-1",
      "model-1",
      "direct",
      "CNY",
    );
    scope.stop();
  });

  it("ignores an old price read after identity changes and an old read after disposal", async () => {
    const first = deferred<PriceVersion | null>();
    vi.mocked(getPrice).mockReturnValueOnce(first.promise);
    const { state, modelId, scope } = createState();
    await waitForLoaded(state);
    state.priceCurrency.value = "USD";
    await vi.waitFor(() => expect(getPrice).toHaveBeenCalled());
    modelId.value = "model-2";
    await nextTick();
    await vi.waitFor(() => expect(state.period.value?.runId).toBe("run-1"));
    first.resolve(price({ currency: "USD", modelId: "model-1" }));
    await Promise.resolve();
    expect(selectPrice).not.toHaveBeenCalled();
    scope.stop();

    const late = deferred<PriceVersion | null>();
    vi.mocked(getPrice).mockReturnValueOnce(late.promise);
    const disposed = createState();
    await waitForLoaded(disposed.state);
    disposed.state.priceCurrency.value = "USD";
    await vi.waitFor(() => expect(getPrice).toHaveBeenCalledTimes(2));
    disposed.scope.stop();
    late.resolve(price({ currency: "USD" }));
    await Promise.resolve();
    expect(selectPrice).not.toHaveBeenCalled();
  });

  it("skips price lookup when run or profile is unavailable", async () => {
    const noRun = createState(null);
    noRun.state.priceCurrency.value = "USD";
    await nextTick();
    expect(getPrice).not.toHaveBeenCalled();
    noRun.scope.stop();

    const noProfile = createState();
    await waitForLoaded(noProfile.state);
    noProfile.profileId.value = "";
    noProfile.state.priceCurrency.value = "USD";
    await nextTick();
    expect(getPrice).not.toHaveBeenCalled();
    noProfile.scope.stop();
  });

  it("ignores a selected price and lookup error after the model identity changes", async () => {
    const selected = deferred<PriceVersion>();
    vi.mocked(getPrice).mockResolvedValue(price({ currency: "USD" }));
    vi.mocked(selectPrice).mockReturnValueOnce(selected.promise);
    const { state, modelId, scope } = createState();
    await waitForLoaded(state);
    state.priceCurrency.value = "USD";
    await vi.waitFor(() => expect(selectPrice).toHaveBeenCalled());
    modelId.value = "model-2";
    await nextTick();
    selected.resolve(price({ currency: "USD", unitTokens: 333 }));
    await Promise.resolve();
    expect(state.unitTokens.value).toBe("1000000");

    const staleError = deferred<PriceVersion | null>();
    vi.mocked(getPrice).mockReturnValueOnce(staleError.promise);
    state.priceCurrency.value = "CNY";
    await vi.waitFor(() => expect(getPrice).toHaveBeenCalledTimes(2));
    modelId.value = "model-3";
    await nextTick();
    staleError.reject("obsolete price failure");
    await Promise.resolve();
    expect(state.error.value?.message).toBeUndefined();
    scope.stop();
  });

  it("reports price lookup and selection errors", async () => {
    vi.mocked(getPrice).mockRejectedValueOnce({ message: "price lookup failed" });
    const { state, scope } = createState();
    await waitForLoaded(state);
    state.priceCurrency.value = "USD";
    await vi.waitFor(() => expect(state.error.value?.message).toBe("price lookup failed"));

    vi.mocked(getPrice).mockResolvedValueOnce(price({ currency: "CNY" }));
    vi.mocked(selectPrice).mockRejectedValueOnce("price selection failed");
    state.priceCurrency.value = "CNY";
    await vi.waitFor(() => expect(state.error.value?.message).toBe("price selection failed"));
    scope.stop();
  });

  it("registers and selects a user-entered price, omitting a blank source URL", async () => {
    const { state, scope } = createState();
    await waitForLoaded(state);
    state.unitTokens.value = "250000";
    state.inputUncached.value = "0.3";
    state.inputCached.value = "0.1";
    state.inputCachedWrite.value = "0";
    state.output.value = "0.8";
    state.sourceUrl.value = "";

    await state.savePrice();

    expect(registerPrice).toHaveBeenCalledWith(
      expect.objectContaining({
        providerId: "provider-1",
        modelId: "model-1",
        routePolicy: "direct",
        currency: "CNY",
        unitTokens: 250000,
        inputUncached: "0.3",
        inputCached: "0.1",
        inputCachedWrite: "0",
        output: "0.8",
        sourceKind: "user-entered",
        usageMappingVersion: "provider-usage-v1",
      }),
    );
    expect(registerPrice).not.toHaveBeenCalledWith(expect.objectContaining({ sourceUrl: "" }));
    expect(selectPrice).toHaveBeenCalledWith("profile-1", "provider-1", "model-1", "direct", "CNY");
    expect(state.priceReady.value).toBe(true);
    expect(state.busy.value).toBe(false);
    state.sourceUrl.value = "https://prices.example/manual";
    await state.savePrice();
    expect(registerPrice).toHaveBeenLastCalledWith(
      expect.objectContaining({ sourceUrl: "https://prices.example/manual" }),
    );
    scope.stop();
  });

  it("requires and registers provider-specific DeepSeek peak prices", async () => {
    const { state, modelId, scope } = createState("run-1", "opencode-go");
    modelId.value = "deepseek-v4.1-flash";
    await waitForLoaded(state);
    expect(state.requiresScheduledPrice.value).toBe(true);
    expect(state.scheduledPriceComplete.value).toBe(false);
    await state.savePrice();
    expect(registerPrice).not.toHaveBeenCalled();

    state.peakInputUncached.value = "0.3";
    state.peakInputCached.value = "0.15";
    state.peakInputCachedWrite.value = "0";
    state.peakOutput.value = "1.2";
    expect(state.scheduledPriceComplete.value).toBe(true);
    await state.savePrice();
    expect(registerPrice).toHaveBeenCalledWith(
      expect.objectContaining({
        schedule: {
          version: "opencode-go-deepseek-utc-weekday-v1",
          peak: {
            inputUncached: "0.3",
            inputCached: "0.15",
            inputCachedWrite: "0",
            output: "1.2",
          },
        },
      }),
    );
    scope.stop();
  });

  it("uses the DeepSeek API holiday schedule for direct DeepSeek models", async () => {
    const { state, modelId, scope } = createState("run-1", "deepseek");
    modelId.value = "deepseek-flash";
    await waitForLoaded(state);
    expect(state.isDeepseekApiSchedule.value).toBe(true);
    expect(state.requiresScheduledPrice.value).toBe(true);
    state.peakInputUncached.value = "2";
    state.peakInputCached.value = "0.04";
    state.peakInputCachedWrite.value = "0";
    state.peakOutput.value = "8";
    await state.savePrice();
    expect(registerPrice).toHaveBeenCalledWith(
      expect.objectContaining({
        schedule: expect.objectContaining({ version: "deepseek-api-utc-weekday-holiday-2026-v2" }),
      }),
    );
    scope.stop();
  });

  it("does not update price readiness or busy state after disposal", async () => {
    const registering = deferred<PriceVersion>();
    vi.mocked(registerPrice).mockReturnValueOnce(registering.promise);
    const { state, scope } = createState();
    await waitForLoaded(state);
    const saving = state.savePrice();
    scope.stop();
    registering.resolve(price());
    await saving;
    expect(state.priceReady.value).toBe(false);
    expect(state.busy.value).toBe(true);

    const failingRegistration = deferred<PriceVersion>();
    vi.mocked(registerPrice).mockReturnValueOnce(failingRegistration.promise);
    const failed = createState();
    await waitForLoaded(failed.state);
    const rejected = failed.state.savePrice();
    failed.scope.stop();
    failingRegistration.reject(new Error("late registration failure"));
    await rejected;
    expect(failed.state.error.value?.message).toBeUndefined();
    expect(failed.state.busy.value).toBe(true);
  });

  it("does not select a registered price after its profile identity changes", async () => {
    const registering = deferred<PriceVersion>();
    vi.mocked(registerPrice).mockReturnValueOnce(registering.promise);
    const { state, profileId, scope } = createState();
    await waitForLoaded(state);

    const saving = state.savePrice();
    profileId.value = "profile-2";
    await nextTick();
    registering.resolve(price());
    await saving;

    expect(selectPrice).not.toHaveBeenCalled();
    expect(state.priceReady.value).toBe(false);
    scope.stop();
  });

  it("skips a queued registered-price selection after its model identity changes", async () => {
    const selectingUsd = deferred<PriceVersion>();
    vi.mocked(getPrice).mockResolvedValue(price({ currency: "USD" }));
    vi.mocked(selectPrice).mockReturnValueOnce(selectingUsd.promise);
    const { state, modelId, scope } = createState();
    await waitForLoaded(state);
    state.priceCurrency.value = "USD";
    await vi.waitFor(() => expect(selectPrice).toHaveBeenCalledTimes(1));

    const saving = state.savePrice();
    await nextTick();
    expect(registerPrice).toHaveBeenCalledTimes(1);
    expect(state.busy.value).toBe(true);
    modelId.value = "model-2";
    await nextTick();
    await waitForLoaded(state);
    selectingUsd.resolve(price({ currency: "USD" }));
    await saving;

    expect(selectPrice).toHaveBeenCalledTimes(1);
    expect(state.priceReady.value).toBe(false);
    expect(state.busy.value).toBe(false);
    expect(state.error.value?.message).toBeUndefined();
    scope.stop();
  });

  it("reports registration selection failure and allows a subsequent save to succeed", async () => {
    vi.mocked(selectPrice).mockRejectedValueOnce(new Error("registered price selection failed"));
    const { state, scope } = createState();
    await waitForLoaded(state);

    await state.savePrice();
    expect(state.error.value?.message).toBe("registered price selection failed");
    expect(state.priceReady.value).toBe(false);
    expect(state.busy.value).toBe(false);

    await state.savePrice();
    expect(registerPrice).toHaveBeenCalledTimes(2);
    expect(selectPrice).toHaveBeenCalledTimes(2);
    expect(state.error.value?.message).toBeUndefined();
    expect(state.priceReady.value).toBe(true);
    expect(state.busy.value).toBe(false);
    scope.stop();
  });

  it("requires an active run and profile and reports registration failures", async () => {
    const missingRun = createState(null);
    await missingRun.state.savePrice();
    expect(registerPrice).not.toHaveBeenCalled();
    missingRun.scope.stop();

    const noProfile = createState();
    noProfile.profileId.value = "";
    await noProfile.state.savePrice();
    expect(registerPrice).not.toHaveBeenCalled();
    noProfile.scope.stop();

    vi.mocked(registerPrice).mockRejectedValueOnce(new Error("registration failed"));
    const { state, scope } = createState();
    await waitForLoaded(state);
    await state.savePrice();
    expect(state.error.value?.message).toBe("registration failed");
    expect(state.busy.value).toBe(false);
    expect(selectPrice).not.toHaveBeenCalled();
    scope.stop();
  });
});

describe("useBilling settings and period refresh", () => {
  it("saves both currency caps with the expected revision and refreshes the run period", async () => {
    vi.mocked(getBudgetSettings).mockResolvedValue(settings());
    const { state, scope } = createState();
    await waitForLoaded(state);
    state.cnyLimit.value = "15";
    state.usdLimit.value = "4";
    await state.saveSettings();

    expect(setBudgetSettings).toHaveBeenCalledWith("run-1", "15", "4", 7);
    expect(state.settings.value?.cnyLimit).toBe("15");
    expect(getRunPeriod).toHaveBeenCalledTimes(2);
    expect(state.busy.value).toBe(false);
    scope.stop();
  });

  it("does not save without a run or while busy, and surfaces save failures", async () => {
    const noRun = createState(null);
    await noRun.state.saveSettings();
    expect(setBudgetSettings).not.toHaveBeenCalled();
    noRun.scope.stop();

    vi.mocked(getBudgetSettings).mockResolvedValue(settings());
    const { state, scope } = createState();
    await waitForLoaded(state);
    state.busy.value = true;
    await state.saveSettings();
    expect(setBudgetSettings).not.toHaveBeenCalled();
    state.busy.value = false;
    vi.mocked(setBudgetSettings).mockRejectedValueOnce({
      code: "conflict",
      message: "revision changed",
    });
    await state.saveSettings();
    expect(state.error.value?.message).toBe("revision changed");
    expect(state.busy.value).toBe(false);
    scope.stop();
  });

  it("recovers readiness after a failed automatic default write", async () => {
    vi.mocked(setBudgetSettings)
      .mockRejectedValueOnce(new Error("temporary storage failure"))
      .mockResolvedValueOnce(settings());
    const { state, scope } = createState();

    await vi.waitFor(() => expect(state.error.value?.message).toBe("temporary storage failure"));
    expect(state.ready.value).toBe(false);

    await state.saveSettings();

    expect(state.ready.value).toBe(true);
    expect(state.settings.value?.cnyLimit).toBe("10");
    scope.stop();
  });

  it("ignores a settings save that completes after switching runs", async () => {
    const pending = deferred<BudgetSettings>();
    vi.mocked(getBudgetSettings).mockImplementation(async (runId) =>
      runId === "run-1" ? settings({ runId }) : null,
    );
    vi.mocked(listBillingRequests).mockImplementation(async (runId) =>
      runId === "run-2" ? requestPage("1", "older-page") : requestPage(),
    );
    vi.mocked(setBudgetSettings).mockReturnValueOnce(pending.promise);
    const { state, runId, scope } = createState();
    await waitForLoaded(state);

    const saving = state.saveSettings();
    runId.value = "run-2";
    await waitForLoaded(state);
    pending.resolve(settings({ runId: "run-1", cnyLimit: "999" }));
    await saving;

    expect(state.settings.value).toBeNull();
    expect(state.period.value?.runId).toBe("run-2");
    expect(getRunPeriod).toHaveBeenCalledTimes(2);
    expect(state.busy.value).toBe(false);
    scope.stop();
  });

  it("ignores an old period refresh after switching runs", async () => {
    const pending = deferred<RunPeriod>();
    const { state, runId, scope } = createState();
    await waitForLoaded(state);
    vi.mocked(getRunPeriod).mockReturnValueOnce(pending.promise);
    const refreshing = state.loadPeriod();

    runId.value = "run-2";
    await waitForLoaded(state);
    pending.resolve(runPeriod("run-1-late"));
    await refreshing;

    expect(state.period.value?.runId).toBe("run-2");
    scope.stop();
  });

  it("keeps the newest run-period refresh when earlier reads resolve later", async () => {
    const earlier = deferred<RunPeriod>();
    const later = deferred<RunPeriod>();
    const { state, scope } = createState();
    await waitForLoaded(state);
    vi.mocked(getRunPeriod).mockReturnValueOnce(earlier.promise).mockReturnValueOnce(later.promise);

    const firstRefresh = state.loadPeriod();
    const secondRefresh = state.loadPeriod();
    later.resolve(runPeriod("run-1-new"));
    await secondRefresh;
    earlier.resolve(runPeriod("run-1-old"));
    await firstRefresh;

    expect(state.period.value?.runId).toBe("run-1-new");
    scope.stop();
  });

  it("does not let the initial period read overwrite a newer refresh", async () => {
    const initial = deferred<RunPeriod>();
    vi.mocked(getRunPeriod).mockReturnValueOnce(initial.promise);
    vi.mocked(getBudgetSettings).mockResolvedValueOnce(settings());
    const { state, scope } = createState();
    await vi.waitFor(() => expect(getRunPeriod).toHaveBeenCalledTimes(1));

    vi.mocked(getRunPeriod).mockResolvedValueOnce(runPeriod("run-1-new"));
    await state.loadPeriod();
    expect(state.period.value?.runId).toBe("run-1-new");

    initial.resolve(runPeriod("run-1-initial"));
    await vi.waitFor(() => expect(state.settings.value).not.toBeNull());
    expect(state.period.value?.runId).toBe("run-1-new");
    scope.stop();
  });

  it("ignores a failed settings save after disposal", async () => {
    const pending = deferred<BudgetSettings>();
    const { state, scope } = createState();
    await waitForLoaded(state);
    vi.mocked(setBudgetSettings).mockReturnValueOnce(pending.promise);
    const saving = state.saveSettings();
    scope.stop();
    pending.reject(new Error("late settings failure"));
    await saving;
    expect(state.error.value?.message).toBeUndefined();
    expect(state.busy.value).toBe(true);
  });

  it("refreshes periods and monthly data, retaining values while reporting failures", async () => {
    const { state, scope } = createState();
    await waitForLoaded(state);
    await state.loadPeriod();
    await state.loadMonthly();
    expect(state.period.value?.runId).toBe("run-1");
    expect(state.monthly.value?.month).toMatch(/^\d{4}-\d{2}$/);

    vi.mocked(getRunPeriod).mockRejectedValueOnce(new Error("run refresh failed"));
    await state.loadPeriod();
    expect(state.error.value?.message).toBe("run refresh failed");
    vi.mocked(getMonthlyPeriod).mockRejectedValueOnce("month refresh failed");
    await state.loadMonthly();
    expect(state.error.value?.message).toBe("month refresh failed");
    scope.stop();
  });

  it("ignores period and settings completions after disposal", async () => {
    const periodWait = deferred<RunPeriod>();
    const disposed = createState();
    await waitForLoaded(disposed.state);
    vi.mocked(getRunPeriod).mockReturnValueOnce(periodWait.promise);
    const refreshing = disposed.state.loadPeriod();
    disposed.scope.stop();
    periodWait.resolve(runPeriod("late"));
    await refreshing;
    expect(disposed.state.period.value?.runId).toBe("run-1");

    const saveWait = deferred<BudgetSettings>();
    const saving = createState();
    await waitForLoaded(saving.state);
    vi.mocked(setBudgetSettings).mockReturnValueOnce(saveWait.promise);
    const save = saving.state.saveSettings();
    saving.scope.stop();
    saveWait.resolve(settings({ cnyLimit: "99" }));
    await save;
    expect(saving.state.settings.value?.cnyLimit).toBe("10");
    expect(saving.state.busy.value).toBe(true);
  });

  it("skips period refresh without a run and ignores a refresh failure after disposal", async () => {
    const noRun = createState(null);
    await noRun.state.loadPeriod();
    expect(getRunPeriod).not.toHaveBeenCalled();
    noRun.scope.stop();

    const pending = deferred<RunPeriod>();
    const { state, scope } = createState();
    await waitForLoaded(state);
    vi.mocked(getRunPeriod).mockReturnValueOnce(pending.promise);
    const refresh = state.loadPeriod();
    scope.stop();
    pending.reject(new Error("late refresh failure"));
    await refresh;
    expect(state.error.value?.message).toBeUndefined();
  });

  it("ignores a monthly refresh error after disposal", async () => {
    const pending = deferred<MonthlyPeriod>();
    const { state, scope } = createState();
    await waitForLoaded(state);
    vi.mocked(getMonthlyPeriod).mockReturnValueOnce(pending.promise);
    const refresh = state.loadMonthly();
    scope.stop();
    pending.reject(new Error("late month failure"));
    await refresh;
    expect(state.error.value?.message).toBeUndefined();
  });

  it("uses UTC if the runtime reports no time zone", async () => {
    vi.spyOn(Intl.DateTimeFormat.prototype, "resolvedOptions").mockImplementation(
      () => ({ timeZone: "" }) as Intl.ResolvedDateTimeFormatOptions,
    );
    const { state, scope } = createState();
    await waitForLoaded(state);
    expect(getMonthlyPeriod).toHaveBeenCalledWith(expect.stringMatching(/^\d{4}-\d{2}$/), "UTC");
    scope.stop();
  });

  it("reports when the date formatter cannot provide a year or month", async () => {
    vi.spyOn(Intl.DateTimeFormat.prototype, "formatToParts").mockReturnValue([
      { type: "day", value: "11" },
    ]);
    const { state, scope } = createState();
    await vi.waitFor(() =>
      expect(state.error.value?.message).toBe("system month could not be determined"),
    );
    expect(getMonthlyPeriod).not.toHaveBeenCalled();
    scope.stop();
  });
});

describe("useBilling request pagination", () => {
  it("skips settlement recovery without an active run", async () => {
    const { state, scope } = createState(null);
    await state.retrySettlement("request-1");
    expect(retryBillingSettlement).not.toHaveBeenCalled();
    expect(state.retryingRequestId.value).toBeUndefined();
    scope.stop();
  });

  it("allows only one settlement recovery at a time", async () => {
    const pending = deferred<void>();
    vi.mocked(retryBillingSettlement).mockReturnValueOnce(pending.promise);
    const { state, scope } = createState();
    await waitForLoaded(state);
    const recovering = state.retrySettlement("request-1");
    expect(state.retryingRequestId.value).toBe("request-1");
    await state.retrySettlement("request-2");
    expect(retryBillingSettlement).toHaveBeenCalledTimes(1);
    expect(state.retryingRequestId.value).toBe("request-1");
    pending.resolve();
    await recovering;
    expect(state.retryingRequestId.value).toBeUndefined();
    await state.retrySettlement("request-2");
    expect(retryBillingSettlement).toHaveBeenLastCalledWith("run-1", "request-2");
    scope.stop();
  });

  it.each(["disposed", "configuration changed", "run changed"] as const)(
    "ignores late settlement success when %s",
    async (boundary) => {
      const pending = deferred<void>();
      vi.mocked(retryBillingSettlement).mockReturnValueOnce(pending.promise);
      const { state, scope, modelId, runId } = createState();
      await waitForLoaded(state);
      const recovering = state.retrySettlement("request-1");
      if (boundary === "disposed") scope.stop();
      if (boundary === "configuration changed") {
        modelId.value = "model-2";
        await nextTick();
        await waitForLoaded(state);
      }
      vi.mocked(listBillingRequests).mockClear();
      vi.mocked(getRunPeriod).mockClear();
      pending.resolve();
      // Resolve before switching so recovery also sees a changed run before its watcher runs.
      if (boundary === "run changed") runId.value = "run-2";
      await recovering;
      expect(vi.mocked(listBillingRequests).mock.calls.filter(([id]) => id === "run-1")).toEqual(
        [],
      );
      expect(vi.mocked(getRunPeriod).mock.calls.filter(([id]) => id === "run-1")).toEqual([]);
      expect(state.retryingRequestId.value).toBeUndefined();
      scope.stop();
    },
  );

  it.each(["disposed", "configuration changed", "run changed"] as const)(
    "ignores late settlement failure when %s",
    async (boundary) => {
      const pending = deferred<void>();
      vi.mocked(retryBillingSettlement).mockReturnValueOnce(pending.promise);
      const { state, scope, modelId, runId } = createState();
      await waitForLoaded(state);
      const recovering = state.retrySettlement("request-1");
      if (boundary === "disposed") scope.stop();
      if (boundary === "configuration changed") {
        modelId.value = "model-2";
        await nextTick();
        await waitForLoaded(state);
      }
      vi.mocked(listBillingRequests).mockClear();
      pending.reject(new Error("old settlement failure"));
      if (boundary === "run changed") runId.value = "run-2";
      await recovering;
      expect(state.error.value).toBeUndefined();
      expect(vi.mocked(listBillingRequests).mock.calls.filter(([id]) => id === "run-1")).toEqual(
        [],
      );
      expect(state.retryingRequestId.value).toBeUndefined();
      scope.stop();
    },
  );

  it("recovers a saved settlement without fetching another provider result", async () => {
    const { state, scope } = createState();
    await waitForLoaded(state);
    await state.retrySettlement("request-1");
    expect(retryBillingSettlement).toHaveBeenCalledWith("run-1", "request-1");
    expect(listBillingRequests).toHaveBeenLastCalledWith("run-1", undefined);
    expect(getRunPeriod).toHaveBeenLastCalledWith("run-1");
    expect(state.retryingRequestId.value).toBeUndefined();
    scope.stop();
  });

  it("keeps a failed local settlement visible and refreshes its diagnostic", async () => {
    vi.mocked(retryBillingSettlement).mockRejectedValueOnce(new Error("local recovery failed"));
    const { state, scope } = createState();
    await waitForLoaded(state);
    await state.retrySettlement("request-1");
    expect(state.error.value?.message).toBe("local recovery failed");
    expect(listBillingRequests).toHaveBeenLastCalledWith("run-1", undefined);
    expect(state.retryingRequestId.value).toBeUndefined();
    scope.stop();
  });

  it("clears a prior settlement error after a successful retry", async () => {
    vi.mocked(retryBillingSettlement)
      .mockRejectedValueOnce(new Error("local recovery failed"))
      .mockResolvedValueOnce();
    const { state, scope } = createState();
    await waitForLoaded(state);
    await state.retrySettlement("request-1");
    expect(state.error.value?.message).toBe("local recovery failed");

    await state.retrySettlement("request-1");
    expect(state.error.value).toBeUndefined();
    scope.stop();
  });

  it("loads the first page, appends the next page and resets on request", async () => {
    vi.mocked(listBillingRequests)
      .mockResolvedValueOnce(requestPage("1", "cursor-a"))
      .mockResolvedValueOnce({
        revision: "2",
        items: [{ requestId: "next" }] as never,
        nextCursor: "cursor-b",
      })
      .mockResolvedValueOnce({ revision: "3", items: [{ requestId: "fresh" }] as never });
    const { state, scope } = createState();
    await waitForLoaded(state);
    await state.loadRequests();
    expect(listBillingRequests).toHaveBeenLastCalledWith("run-1", "cursor-a");
    expect(state.requests.value?.items.map((item) => item.requestId)).toEqual(["next"]);
    await state.loadRequests(true);
    expect(listBillingRequests).toHaveBeenLastCalledWith("run-1", undefined);
    expect(state.requests.value?.items.map((item) => item.requestId)).toEqual(["fresh"]);
    scope.stop();
  });

  it("uses a fetched page when pagination begins before the initial page arrives", async () => {
    const initial = deferred<BillingRequestPage>();
    vi.mocked(listBillingRequests)
      .mockReturnValueOnce(initial.promise)
      .mockResolvedValueOnce(requestPage("early"));
    const { state, scope } = createState();
    await state.loadRequests();
    expect(state.requests.value?.revision).toBe("early");
    initial.resolve(requestPage("initial"));
    await waitForLoaded(state);
    expect(state.requests.value?.revision).toBe("initial");
    scope.stop();
  });

  it("ignores a request page that completes after switching runs", async () => {
    const pending = deferred<BillingRequestPage>();
    const { state, runId, scope } = createState();
    await waitForLoaded(state);
    vi.mocked(listBillingRequests).mockReturnValueOnce(pending.promise);
    const loadingPage = state.loadRequests(true);

    runId.value = "run-2";
    await waitForLoaded(state);
    pending.resolve({
      revision: "old-run",
      items: [{ requestId: "old-request" }] as never,
    });
    await loadingPage;

    expect(state.requests.value?.revision).toBe("1");
    expect(state.requests.value?.items).toEqual([]);
    scope.stop();
  });

  it("recovers stale cursors by resetting once, reports other failures, and skips without a run", async () => {
    const { state, runId, scope } = createState();
    await waitForLoaded(state);
    vi.mocked(listBillingRequests)
      .mockRejectedValueOnce({ code: "app.bad-request", detail: { reason: "staleRevision" } })
      .mockResolvedValueOnce(requestPage("5", "new-cursor"));
    await state.loadRequests();
    expect(listBillingRequests).toHaveBeenLastCalledWith("run-1", undefined);
    expect(state.requests.value?.revision).toBe("5");

    vi.mocked(listBillingRequests).mockRejectedValueOnce(new Error("page failed"));
    await state.loadRequests(true);
    expect(state.error.value?.message).toBe("page failed");

    vi.mocked(listBillingRequests).mockRejectedValueOnce({
      code: "app.bad-request",
      detail: { reason: "staleRevision" },
    });
    await state.loadRequests(true);
    expect(state.error.value?.message).toBe("Billing operation failed");

    for (const failure of [
      null,
      "not an error object",
      { code: "other" },
      { code: "app.bad-request" },
      { code: "app.bad-request", detail: "not an object" },
      { code: "app.bad-request", detail: { reason: "other" } },
    ]) {
      vi.mocked(listBillingRequests).mockRejectedValueOnce(failure);
      await state.loadRequests(true);
    }
    expect(state.error.value?.message).toBe("Billing operation failed");

    runId.value = undefined;
    await nextTick();
    vi.mocked(listBillingRequests).mockClear();
    await state.loadRequests();
    expect(listBillingRequests).not.toHaveBeenCalled();
    scope.stop();
  });

  it("does not apply a page that resolves after disposal", async () => {
    const pending = deferred<BillingRequestPage>();
    const { state, scope } = createState();
    await waitForLoaded(state);
    vi.mocked(listBillingRequests).mockReturnValueOnce(pending.promise);
    const loading = state.loadRequests(true);
    scope.stop();
    pending.resolve(requestPage("late"));
    await loading;
    expect(state.requests.value?.revision).toBe("1");
  });

  it("does not publish a page error after disposal", async () => {
    const pending = deferred<BillingRequestPage>();
    const { state, scope } = createState();
    await waitForLoaded(state);
    vi.mocked(listBillingRequests).mockReturnValueOnce(pending.promise);
    const loading = state.loadRequests(true);
    scope.stop();
    pending.reject(new Error("late page failure"));
    await loading;
    expect(state.error.value?.message).toBeUndefined();
  });
});

describe("useBilling historical backfill", () => {
  const preview: BillingBackfillPreview = {
    requestId: "legacy-1",
    runId: "run-1",
    providerId: "deepseek",
    modelId: "deepseek-flash",
    currency: "CNY",
    priceVersionId: "original-price",
    estimatedAmount: "0.012345",
    ledgerRevision: 12,
  };

  it("commits the preview's frozen price revision and refreshes ledger views", async () => {
    vi.mocked(previewBillingBackfill).mockResolvedValueOnce(preview);
    const { state, scope } = createState();
    await waitForLoaded(state);
    await state.commitBackfill();
    expect(commitBillingBackfill).not.toHaveBeenCalled();
    await state.previewBackfill("legacy-1");
    expect(previewBillingBackfill).toHaveBeenCalledWith("run-1", "legacy-1");
    expect(state.backfillPreview.value).toEqual(preview);
    expect(state.backfillingRequestId.value).toBeUndefined();
    await state.commitBackfill();
    expect(commitBillingBackfill).toHaveBeenCalledWith("run-1", "legacy-1", expect.any(String), 12);
    expect(state.backfillPreview.value).toBeNull();
    expect(state.backfillingRequestId.value).toBeUndefined();
    expect(listBillingRequests).toHaveBeenCalledTimes(2);
    expect(getRunPeriod).toHaveBeenCalledTimes(2);
    scope.stop();
  });

  it("skips unavailable runs, busy forms and concurrent backfill operations", async () => {
    const noRun = createState(null);
    await noRun.state.previewBackfill("legacy-1");
    noRun.state.backfillPreview.value = preview;
    await noRun.state.commitBackfill();
    expect(previewBillingBackfill).not.toHaveBeenCalled();
    expect(commitBillingBackfill).not.toHaveBeenCalled();
    noRun.scope.stop();
    const pending = deferred<BillingBackfillPreview | null>();
    vi.mocked(previewBillingBackfill).mockReturnValueOnce(pending.promise);
    const { state, scope } = createState();
    await waitForLoaded(state);
    state.busy.value = true;
    await state.previewBackfill("legacy-1");
    expect(previewBillingBackfill).not.toHaveBeenCalled();
    state.busy.value = false;
    const loading = state.previewBackfill("legacy-1");
    await state.previewBackfill("legacy-2");
    expect(previewBillingBackfill).toHaveBeenCalledOnce();
    pending.resolve(preview);
    await loading;
    const committing = deferred<void>();
    vi.mocked(commitBillingBackfill).mockReturnValueOnce(committing.promise);
    const saving = state.commitBackfill();
    expect(state.backfillingRequestId.value).toBe("legacy-1");
    await state.commitBackfill();
    expect(commitBillingBackfill).toHaveBeenCalledOnce();
    committing.resolve();
    await saving;
    scope.stop();
  });

  it("reports preview errors and refreshes diagnostics after a rejected stale commit", async () => {
    vi.mocked(previewBillingBackfill)
      .mockRejectedValueOnce(new Error("preview failed"))
      .mockResolvedValueOnce(preview);
    const { state, scope } = createState();
    await waitForLoaded(state);
    await state.previewBackfill("legacy-1");
    expect(state.error.value?.message).toBe("preview failed");
    expect(state.backfillingRequestId.value).toBeUndefined();
    await state.previewBackfill("legacy-1");
    expect(state.error.value).toBeUndefined();
    vi.mocked(commitBillingBackfill).mockRejectedValueOnce({
      code: "app.bad-request",
      message: "ledger revision changed",
    });
    await state.commitBackfill();
    expect(state.error.value).toEqual({
      code: "app.bad-request",
      message: "ledger revision changed",
    });
    expect(state.backfillPreview.value).toBeNull();
    expect(state.backfillingRequestId.value).toBeUndefined();
    expect(listBillingRequests).toHaveBeenCalledTimes(2);
    expect(getRunPeriod).toHaveBeenCalledOnce();
    scope.stop();
  });

  it.each(["switch", "dispose"] as const)(
    "discards successful and failed preview or commit replies after %s",
    async (change) => {
      for (const operation of ["preview", "commit"] as const) {
        for (const fails of [false, true]) {
          vi.mocked(previewBillingBackfill).mockResolvedValue(preview);
          const pendingPreview = deferred<BillingBackfillPreview | null>();
          const pendingCommit = deferred<void>();
          const { state, runId, scope } = createState();
          await waitForLoaded(state);
          if (operation === "commit") {
            await state.previewBackfill("legacy-1");
            vi.mocked(commitBillingBackfill).mockReturnValueOnce(pendingCommit.promise);
          } else vi.mocked(previewBillingBackfill).mockReturnValueOnce(pendingPreview.promise);
          const working =
            operation === "commit" ? state.commitBackfill() : state.previewBackfill("legacy-1");
          if (change === "dispose") scope.stop();
          else {
            runId.value = "run-2";
            await nextTick();
            await waitForLoaded(state);
          }
          const requestCalls = vi.mocked(listBillingRequests).mock.calls.length;
          if (operation === "preview") {
            if (fails) pendingPreview.reject(new Error("obsolete preview"));
            else pendingPreview.resolve(preview);
          } else {
            if (fails) pendingCommit.reject(new Error("obsolete commit"));
            else pendingCommit.resolve();
          }
          await working;
          expect(state.error.value).toBeUndefined();
          expect(listBillingRequests).toHaveBeenCalledTimes(requestCalls);
          if (operation === "preview" || change === "switch")
            expect(state.backfillPreview.value).toBeNull();
          if (change === "switch") expect(state.backfillingRequestId.value).toBeUndefined();
          scope.stop();
        }
      }
    },
  );
});

describe("useBilling provider balance recovery", () => {
  it("requires a profile, prevents concurrent queries and permits retry after an error", async () => {
    const { state, profileId, scope } = createState("run-1", "deepseek");
    await waitForLoaded(state);
    profileId.value = "";
    await nextTick();
    await state.queryProviderBalance();
    expect(getProviderBalance).not.toHaveBeenCalled();
    profileId.value = "profile-1";
    await nextTick();
    await waitForLoaded(state);
    const pending = deferred<ProviderBalanceSnapshot>();
    vi.mocked(getProviderBalance).mockReturnValueOnce(pending.promise);
    const working = state.queryProviderBalance();
    expect(state.balanceBusy.value).toBe(true);
    await state.queryProviderBalance();
    expect(getProviderBalance).toHaveBeenCalledOnce();
    pending.reject(new Error("balance unavailable"));
    await working;
    expect(state.error.value?.message).toBe("balance unavailable");
    expect(state.balanceBusy.value).toBe(false);
    expect(state.providerBalance.value).toBeUndefined();
    await state.queryProviderBalance();
    expect(state.providerBalance.value).toEqual(balanceSnapshot());
    expect(state.error.value).toBeUndefined();
    expect(state.balanceBusy.value).toBe(false);
    scope.stop();
  });

  it("clears the previous credential balance and ignores its late reply", async () => {
    const { state, credentialRevision, scope } = createState("run-1", "deepseek");
    await waitForLoaded(state);
    state.providerBalance.value = balanceSnapshot();
    const pending = deferred<ProviderBalanceSnapshot>();
    vi.mocked(getProviderBalance).mockReturnValueOnce(pending.promise);
    const working = state.queryProviderBalance();
    credentialRevision.value += 1;
    await nextTick();
    expect(state.providerBalance.value).toBeUndefined();
    expect(state.balanceBusy.value).toBe(false);
    pending.resolve(balanceSnapshot());
    await working;
    expect(state.providerBalance.value).toBeUndefined();
    scope.stop();
  });

  it.each(["switch", "dispose"] as const)(
    "discards successful and failed balance replies after %s",
    async (change) => {
      for (const fails of [false, true]) {
        const pending = deferred<ProviderBalanceSnapshot>();
        vi.mocked(getProviderBalance).mockReturnValueOnce(pending.promise);
        const { state, profileId, scope } = createState("run-1", "deepseek");
        await waitForLoaded(state);
        const working = state.queryProviderBalance();
        if (change === "dispose") scope.stop();
        else {
          profileId.value = "profile-2";
          await nextTick();
          await waitForLoaded(state);
        }
        if (fails) pending.reject(new Error("obsolete balance"));
        else pending.resolve(balanceSnapshot());
        await working;
        expect(state.providerBalance.value).toBeUndefined();
        expect(state.error.value).toBeUndefined();
        if (change === "switch") expect(state.balanceBusy.value).toBe(false);
        scope.stop();
      }
    },
  );
});

describe("registered price display refresh", () => {
  it("refreshes metadata and editor fields after confirmation without selecting another price", async () => {
    const { state, scope } = createState();
    await waitForLoaded(state);
    const registered = price({ currency: "USD", versionId: "confirmed-reference" });
    vi.mocked(getSelectedPrice).mockResolvedValueOnce(registered);
    await state.refreshSelectedPrice();
    await nextTick();
    expect(state.selectedPrice.value).toEqual(registered);
    expect(state.priceCurrency.value).toBe("USD");
    expect(state.inputUncached.value).toBe(registered.inputUncached);
    expect(state.priceReady.value).toBe(true);
    expect(selectPrice).not.toHaveBeenCalled();
    scope.stop();
  });
  it("ignores a stale display refresh after switching model and reports current read errors", async () => {
    const { state, modelId, scope } = createState();
    await waitForLoaded(state);
    const pending = deferred<PriceVersion | null>();
    vi.mocked(getSelectedPrice).mockReturnValueOnce(pending.promise);
    const refreshing = state.refreshSelectedPrice();
    modelId.value = "model-2";
    await nextTick();
    pending.resolve(price());
    await refreshing;
    expect(state.selectedPrice.value).toBeNull();
    vi.mocked(getSelectedPrice).mockRejectedValueOnce(new Error("price lookup unavailable"));
    await state.refreshSelectedPrice();
    expect(state.error.value?.message).toBe("price lookup unavailable");
    scope.stop();
  });
});
