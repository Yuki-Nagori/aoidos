import { nextTick, computed, onScopeDispose, ref, shallowRef, watch } from "vue";
import type { LlmProfile, KeyStatus } from "../../../api/llm";
import { getKeyStatus, listProfiles, saveProfile, setKey } from "../../../api/llm";
import type { OpenedSession, ScriptInfo } from "../../../api/engine";
import type { OpenedRun } from "../../../api/runs";
import { listScripts, openSession } from "../../../api/engine";
import type { BillingCurrency } from "../../../api/billing";
import {
  confirmDeepseekPrice,
  getDeepseekPriceTemplates,
  getSelectedPrice,
} from "../../../api/billing";
import { safeError } from "../../../shared/utils/error";

interface Transport {
  profiles: typeof listProfiles;
  keyStatus: typeof getKeyStatus;
  scripts: typeof listScripts;
  save: typeof saveProfile;
  key: typeof setKey;
  confirmPrice: typeof confirmDeepseekPrice;
  priceTemplates: typeof getDeepseekPriceTemplates;
  selectedPrice: typeof getSelectedPrice;
  open: typeof openSession;
}
const defaults: Transport = {
  profiles: listProfiles,
  keyStatus: getKeyStatus,
  scripts: listScripts,
  save: saveProfile,
  key: setKey,
  confirmPrice: confirmDeepseekPrice,
  priceTemplates: getDeepseekPriceTemplates,
  selectedPrice: getSelectedPrice,
  open: openSession,
};
const openCodeZenIds = [
  "deepseek-v4.1-flash",
  "gpt-6-astra",
  "gpt-6-sol",
  "gpt-6.1-sol",
  "gpt-6-luna",
  "gpt-5.6-sol",
  "gpt-5.6-terra",
  "gpt-5.6-luna",
  "gpt-5.5",
  "gpt-5.5-pro",
  "gpt-5.4",
  "gpt-5.4-pro",
  "gpt-5.4-mini",
  "gpt-5.4-nano",
  "gpt-5.3-codex",
  "gpt-5.3-codex-spark",
  "gpt-5.2",
  "gpt-5.2-codex",
  "gpt-5.1",
  "gpt-5.1-codex",
  "gpt-5.1-codex-max",
  "gpt-5.1-codex-mini",
  "gpt-5",
  "gpt-5-codex",
  "gpt-5-nano",
  "claude-fable-5-1",
  "claude-fable-5",
  "claude-opus-5-5",
  "claude-opus-5",
  "claude-opus-4-8",
  "claude-opus-4-7",
  "claude-opus-4-6",
  "claude-opus-4-5",
  "claude-sonnet-5-5",
  "claude-sonnet-5",
  "claude-sonnet-4-6",
  "claude-sonnet-4-5",
  "claude-haiku-5-5",
  "claude-haiku-4-5",
  "gemini-3.8-flash",
  "gemini-3.7-flash",
  "gemini-3.6-flash",
  "gemini-3.5-flash",
  "gemini-3.5-flash-lite",
  "gemini-3.1-pro",
  "gemini-3-flash",
  "grok-4.7",
  "grok-4.6",
  "grok-4.5",
  "grok-build-0.1",
  "muse-spark-1.3",
  "muse-spark-1.2",
  "qwen3.8-max",
  "qwen3.8-flash",
  "qwen3.7-max",
  "qwen3.7-plus",
  "qwen3.6-plus",
  "qwen3.5-plus",
  "deepseek-v4-pro",
  "deepseek-v4-flash",
  "deepseek-v4-flash-vision-exp",
  "minimax-m3",
  "minimax-m2.7",
  "minimax-m2.5",
  "glm-5.3-flash",
  "glm-5.3",
  "glm-5.2",
  "glm-5.1",
  "glm-5",
  "kimi-k2.5",
  "kimi-k2.6",
  "kimi-k2.7-code",
  "kimi-k3",
  "mistral-large-4",
  "big-pickle",
  "space-bunny-free",
  "longcat-2.5-preview-free",
  "step-5-preview-free",
  "exo-free",
  "mimo-v2.6-flash-free",
  "mimo-v2.5-free",
  "ling-3.1-flash-free",
  "ling-3.0-flash-fin-free",
  "nemotron-3-ultra-free",
  "nemotron-3.5-lightning-free",
  "muse-spark-1.3-contributor-free",
] as const;
const openCodeZenModels = openCodeZenIds.map((id) => ({
  id,
  label: id
    .replace(
      /(^|[-.])([a-z])/g,
      (_, separator: string, letter: string) => `${separator}${letter.toUpperCase()}`,
    )
    .replaceAll("-", " ")
    .replace(/\bGpt\b/g, "GPT")
    .replace(/\bDeepseek\b/g, "DeepSeek")
    .replace(/\bMimo\b/g, "MiMo")
    .replace(/\bMinimax\b/g, "MiniMax")
    .replace(/\bGlm\b/g, "GLM"),
}));
/** 最小产品入口只登记配置及会话；异步操作不因卸载更新状态，不隐式生成或续费。 */
export function useProduct(transport: Transport = defaults) {
  const profiles = shallowRef<LlmProfile[]>([]);
  const activeProfile = shallowRef<LlmProfile>();
  const scripts = shallowRef<ScriptInfo[]>([]);
  const priceTemplates = shallowRef<Awaited<ReturnType<typeof getDeepseekPriceTemplates>>>([]);
  const profileId = ref("");
  const scriptId = ref("mistbell");
  const providerId = ref("deepseek");
  const providerOptions = [
    { id: "deepseek", label: "DeepSeek" },
    { id: "opencode-zen", label: "OpenCode Zen" },
    { id: "opencode-go", label: "OpenCode Go" },
  ] as const;
  const modelCatalog = {
    deepseek: [
      { id: "deepseek-flash", label: "DeepSeek V4.1 Flash" },
      { id: "deepseek-v4-pro", label: "DeepSeek V4 Pro" },
    ],
    "opencode-zen": openCodeZenModels,
    "opencode-go": [
      { id: "deepseek-v4.1-flash", label: "DeepSeek V4.1 Flash" },
      { id: "deepseek-v4-pro", label: "DeepSeek V4 Pro" },
      { id: "deepseek-v4-flash", label: "DeepSeek V4 Flash" },
      { id: "deepseek-v4-flash-vision-exp", label: "DeepSeek V4 Flash Vision Exp" },
      { id: "grok-4.7", label: "Grok 4.7" },
      { id: "grok-4.6", label: "Grok 4.6" },
      { id: "glm-5.3-flash", label: "GLM-5.3-Flash" },
      { id: "glm-5.3", label: "GLM-5.3" },
      { id: "glm-5.2", label: "GLM-5.2" },
      { id: "gpt-6-luna", label: "GPT 6 Luna" },
      { id: "gpt-5.6-luna", label: "GPT 5.6 Luna" },
      { id: "claude-haiku-5-5", label: "Claude Haiku 5.5" },
      { id: "kimi-k3", label: "Kimi K3" },
      { id: "kimi-k2.7-code", label: "Kimi K2.7 Code" },
      { id: "kimi-k2.6", label: "Kimi K2.6" },
      { id: "longcat-2.0", label: "LongCat-2.0" },
      { id: "longcat-2.5-preview-free", label: "LongCat 2.5 Preview Free" },
      { id: "step-5-preview-free", label: "Step 5 Preview Free" },
      { id: "mimo-v2.6-flash", label: "MiMo-V2.6-Flash" },
      { id: "mimo-v2.6-pro", label: "MiMo-V2.6-Pro" },
      { id: "mimo-v2.5", label: "MiMo-V2.5" },
      { id: "mimo-v2.5-pro", label: "MiMo-V2.5-Pro" },
      { id: "minimax-m3", label: "MiniMax M3" },
      { id: "minimax-m2.7", label: "MiniMax M2.7" },
      { id: "muse-spark-1.3-contributor", label: "Muse Spark 1.3 Contributor" },
      { id: "muse-spark-1.2-contributor", label: "Muse Spark 1.2 Contributor" },
      { id: "qwen3.8-max", label: "Qwen3.8 Max" },
      { id: "qwen3.8-flash", label: "Qwen3.8 Flash" },
      { id: "qwen3.7-plus", label: "Qwen3.7 Plus" },
      { id: "hy4-preview", label: "Hy4 preview" },
      { id: "hy3", label: "Hy3" },
      { id: "space-bunny", label: "Space Bunny" },
    ],
  } as const;
  const models = computed(() => modelCatalog[providerId.value as keyof typeof modelCatalog]);
  const model = ref("deepseek-flash");
  let keyStatusRequest = 0;
  let selectedPriceRequest = 0;
  let registeredPriceRequest = 0;
  let selectedPriceLookup: Promise<void> | undefined;
  let selectedPriceLookupFailed = false;
  watch(
    providerId,
    (provider) => {
      keyStatusRequest += 1;
      selectedPriceRequest += 1;
      registeredPriceRequest += 1;
      billingCurrency.value = "CNY";
      keyStatus.value = undefined;
      keyStatusLoading.value = false;
      model.value = modelCatalog[provider as keyof typeof modelCatalog][0].id;
      profileId.value =
        profiles.value.find(
          (profile) => profile.providerId === provider && profile.model === model.value,
        )?.profileId ?? "";
    },
    { flush: "sync" },
  );
  watch(
    model,
    (selectedModel) => {
      selectedPriceRequest += 1;
      registeredPriceRequest += 1;
      billingCurrency.value = "CNY";
      profileId.value =
        profiles.value.find(
          (profile) => profile.providerId === providerId.value && profile.model === selectedModel,
        )?.profileId ?? "";
    },
    { flush: "sync" },
  );
  const session = shallowRef<OpenedSession>();
  const keyStatus = shallowRef<KeyStatus>();
  const billingCurrency = ref<BillingCurrency>("CNY");
  const registeredBillingCurrency = ref<BillingCurrency>();
  const keyStatusLoading = ref(false);
  const credentialRevision = ref(0);
  const busy = ref(false);
  const error = shallowRef<{ code: string; message: string }>();
  let disposed = false;
  function refreshRegisteredCurrency(
    provider: string,
    selectedModel: string,
    selectedProfile: string,
  ): void {
    const request = ++selectedPriceRequest;
    const registrationRequest = ++registeredPriceRequest;
    billingCurrency.value = "CNY";
    registeredBillingCurrency.value = undefined;
    selectedPriceLookup = undefined;
    selectedPriceLookupFailed = false;
    if (provider !== "deepseek" || !selectedProfile) return;
    selectedPriceLookup = transport
      .selectedPrice(selectedProfile, provider, selectedModel, "direct")
      .then((price) => {
        if (
          !disposed &&
          registrationRequest === registeredPriceRequest &&
          providerId.value === provider &&
          model.value === selectedModel &&
          profileId.value === selectedProfile
        ) {
          const registeredCurrency =
            price?.currency === "CNY" || price?.currency === "USD" ? price.currency : undefined;
          registeredBillingCurrency.value = registeredCurrency;
          if (request === selectedPriceRequest && registeredCurrency) {
            billingCurrency.value = registeredCurrency;
          }
        }
      })
      .catch((failure) => {
        if (!disposed && registrationRequest === registeredPriceRequest) {
          selectedPriceLookupFailed = true;
          error.value = safeError(failure, "价格币种读取失败");
        }
      });
  }
  watch(
    [providerId, model, profileId],
    ([provider, selectedModel, selectedProfile]) =>
      refreshRegisteredCurrency(provider, selectedModel, selectedProfile),
    { flush: "post" },
  );
  onScopeDispose(() => {
    disposed = true;
  });
  async function run(operation: () => Promise<void>): Promise<void> {
    if (busy.value || disposed) return;
    busy.value = true;
    error.value = undefined;
    try {
      await operation();
    } catch (failure) {
      if (!disposed) error.value = safeError(failure, "回合快照恢复失败");
    } finally {
      if (!disposed) busy.value = false;
    }
  }
  function load(): Promise<void> {
    return run(async () => {
      const [saved, bundled, prices] = await Promise.all([
        transport.profiles(),
        transport.scripts(),
        transport.priceTemplates().catch(() => []),
      ]);
      if (disposed) return;
      profiles.value = saved.items;
      scripts.value = bundled;
      priceTemplates.value = prices;
      profileId.value =
        saved.items.find(
          (profile) => profile.providerId === providerId.value && profile.model === model.value,
        )?.profileId ?? "";
      activeProfile.value = profiles.value.find((profile) => profile.profileId === profileId.value);
    });
  }
  async function refreshKeyStatus(provider = providerId.value): Promise<void> {
    const request = ++keyStatusRequest;
    keyStatusLoading.value = true;
    keyStatus.value = undefined;
    try {
      const status = await transport.keyStatus(provider);
      if (!disposed && request === keyStatusRequest && provider === providerId.value) {
        keyStatus.value = status;
      }
    } catch {
      // 密钥状态不可读时保持未知，不能将读取失败误报为“未设置”。
    } finally {
      if (!disposed && request === keyStatusRequest) keyStatusLoading.value = false;
    }
  }
  async function selectProfile(): Promise<string> {
    const defaultProfileId =
      providerId.value === "deepseek" ? "default" : `default-${providerId.value}`;
    const existing = profiles.value.find(
      (profile) => profile.providerId === providerId.value && profile.model === model.value,
    );
    if (existing) return existing.profileId;
    const previous = profiles.value.find(
      (profile) =>
        profile.providerId === providerId.value && profile.profileId === defaultProfileId,
    );
    const saved = await transport.save({
      profileId: defaultProfileId,
      providerId: providerId.value,
      model: model.value,
      mode: previous?.mode ?? (providerId.value === "deepseek" ? "completion" : "chat"),
      thinking: false,
      sampling: previous?.sampling ?? { temperature: 1, maxTokens: 2048 },
      proxy: previous?.proxy ?? { mode: "system" },
    });
    if (!disposed)
      profiles.value = [
        ...profiles.value.filter((profile) => profile.profileId !== saved.profileId),
        saved,
      ];
    return saved.profileId;
  }
  function configureKey(): Promise<void> {
    return run(async () => {
      const provider = providerId.value;
      const request = ++keyStatusRequest;
      try {
        const status = await transport.key(provider, "set");
        if (!disposed && request === keyStatusRequest && provider === providerId.value) {
          keyStatus.value = status;
          credentialRevision.value += 1;
        }
      } finally {
        if (!disposed && request === keyStatusRequest) keyStatusLoading.value = false;
      }
    });
  }
  /** 等待已登记币种读取完毕，确认没有旧选择后才应用界面语言默认值。 */
  async function prepareBillingCurrency(locale: string): Promise<boolean> {
    await nextTick();
    if (selectedPriceLookupFailed) {
      error.value = undefined;
      refreshRegisteredCurrency(providerId.value, model.value, profileId.value);
    }
    let lookup: Promise<void> | undefined;
    do {
      lookup = selectedPriceLookup;
      await lookup;
    } while (lookup !== selectedPriceLookup);
    if (disposed || selectedPriceLookupFailed) return false;
    if (!registeredBillingCurrency.value) setBillingCurrency(locale === "zh-Hans" ? "CNY" : "USD");
    return true;
  }
  function setBillingCurrency(currency: BillingCurrency): void {
    selectedPriceRequest += 1;
    billingCurrency.value = currency;
  }
  function confirmBillingPrice(): Promise<void> {
    return run(async () => {
      if (providerId.value !== "deepseek") return;
      const currency = billingCurrency.value;
      if (!priceTemplates.value.some((price) => price.currency === currency)) return;
      const selected = await selectProfile();
      await transport.confirmPrice(selected, model.value, currency);
      if (!disposed) {
        registeredPriceRequest += 1;
        profileId.value = selected;
        registeredBillingCurrency.value = currency;
      }
    });
  }
  function open(startNew: boolean): Promise<void> {
    return run(async () => {
      const selected = await selectProfile();
      if (disposed) return;
      const opened = await transport.open(selected, scriptId.value, startNew);
      if (!disposed) {
        profileId.value = selected;
        activeProfile.value = profiles.value.find((profile) => profile.profileId === selected);
        session.value = opened;
      }
    });
  }
  function adoptOpenedRun(opened: OpenedRun): void {
    const profile = opened.profile;
    scriptId.value = opened.session.scriptId;
    providerId.value = profile.providerId;
    model.value = profile.model;
    profileId.value = profile.profileId;
    activeProfile.value = profile;
    if (!profiles.value.some((saved) => saved.profileId === profile.profileId)) {
      profiles.value = [...profiles.value, profile];
    }
    session.value = opened.session;
    void refreshKeyStatus();
  }
  return {
    profiles,
    activeProfile,
    scripts,
    priceTemplates,
    profileId,
    providerId,
    providerOptions,
    scriptId,
    model,
    session,
    keyStatus,
    billingCurrency,
    registeredBillingCurrency,
    setBillingCurrency,
    prepareBillingCurrency,
    keyStatusLoading,
    credentialRevision,
    busy,
    error,
    load,
    models,
    refreshKeyStatus,
    configureKey,
    confirmBillingPrice,
    open,
    adoptOpenedRun,
  };
}
