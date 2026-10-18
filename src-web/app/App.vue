<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import SetupPanel from "./components/SetupPanel.vue";
import MemorySettingsPanel from "../features/memory/components/MemorySettingsPanel.vue";
import MemoryQueryPanel from "../features/memory/components/MemoryQueryPanel.vue";
import GameWorkspace from "../features/game/components/GameWorkspace.vue";
import { useProduct } from "../features/product/composables/useProduct";
import { useTheme } from "../features/theme/composables/useTheme";
import { useLocale } from "../features/locale/composables/useLocale";
import { useBilling } from "../features/billing/composables/useBilling";
import { useUiPreferences } from "../features/game/composables/useUiPreferences";

const { t } = useI18n();
const product = useProduct();
const theme = useTheme(product.scriptId);
const locale = useLocale();
const preferences = useUiPreferences();
const sessionId = computed(() => product.session.value?.sessionId);
const billingProfile = computed(() =>
  product.profiles.value.find((profile) => profile.profileId === product.profileId.value),
);
const billing = useBilling(
  sessionId,
  computed(() => billingProfile.value?.profileId ?? ""),
  computed(() => billingProfile.value?.providerId ?? product.providerId.value),
  computed(() => billingProfile.value?.model ?? product.model.value),
  product.credentialRevision,
);
type AppFault = { code: string; message: string; detail?: unknown };
const gameError = ref<AppFault>();
const historyRevision = ref(0);
watch(sessionId, (id) => {
  if (id) return;
  gameError.value = undefined;
  historyRevision.value = 0;
}, { flush: "sync" });
const error = computed(() => product.error.value ?? gameError.value);
function faultReason(failure: unknown): string | undefined {
  if (typeof failure !== "object" || failure === null || !("detail" in failure)) return undefined;
  const detail = failure.detail;
  if (typeof detail !== "object" || detail === null || !("reason" in detail)) return undefined;
  return typeof detail.reason === "string" ? detail.reason : undefined;
}
const errorMessage = computed(() => {
  const failure = error.value;
  if (!failure) return undefined;
  switch (failure.code) {
    case "app.not-ready": return t("errors.appNotReady");
    case "app.not-found": return t("errors.appNotFound");
    case "app.bad-request":
      return faultReason(failure) === "stalePresenceRevision" || faultReason(failure) === "targetUnavailable"
        ? t("game.presenceChanged")
        : t("errors.badRequest");
    case "store.corrupt": return t("errors.storeCorrupt");
    case "store.io": return t("errors.storeIo");
    case "app.event-failed": return t("errors.appEventFailed");
    case "app.busy": return t("errors.appBusy");
    default: return t("errors.unknownWithCode", { code: failure.code });
  }
});
const workspace = ref<InstanceType<typeof GameWorkspace>>();
void product.load();
void preferences.load();
</script>
<template>
  <main class="mx-auto grid min-h-screen max-w-4xl gap-6 p-6 text-ink">
    <SetupPanel
      :product="product"
      :theme="theme"
      :locale="locale"
      :billing="billing"
      :error-message="errorMessage"
      @reconnect="workspace?.reconnect()"
    >
      <template #memory-settings>
        <MemorySettingsPanel :script-id="product.scriptId.value" />
      </template>
    </SetupPanel>
    <MemoryQueryPanel
      v-if="product.session.value"
      :key="product.session.value.memoryRunId"
      :run-id="product.session.value.memoryRunId"
      :history-revision="historyRevision"
    />
    <GameWorkspace
      v-if="product.session.value"
      ref="workspace"
      :session="product.session.value"
      :product-busy="product.busy.value"
      :preferences="preferences"
      @error="gameError = $event as AppFault"
      @history-revision="historyRevision = $event"
    />
  </main>
</template>
