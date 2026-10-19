<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import SetupPanel from "./components/SetupPanel.vue";
import MemorySettingsPanel from "../features/memory/components/MemorySettingsPanel.vue";
import MemoryQueryPanel from "../features/memory/components/MemoryQueryPanel.vue";
import GameWorkspace from "../features/game/components/GameWorkspace.vue";
import WelcomeScreen from "./components/WelcomeScreen.vue";
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
const settingsOpen = ref(false);
const settingsSection = ref("appearance");
const settingsTrigger = ref<HTMLButtonElement>();
function openSettings(section = "appearance"): void {
  settingsSection.value = section;
  settingsOpen.value = true;
}
function closeSettings(): void {
  settingsOpen.value = false;
  void nextTick(() => settingsTrigger.value?.focus());
}
void product.load();
void preferences.load();
</script>
<template>
  <div class="app-root">
    <main class="app-shell" :inert="settingsOpen">
      <header v-if="!product.session.value" class="topbar">
        <svg class="logo" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <circle cx="12" cy="12" r="9" stroke="currentColor" stroke-opacity=".3" />
          <path
            d="M6.5 15.5c2-6 9-9 11-7"
            stroke="var(--accent)"
            stroke-width="1.6"
            stroke-linecap="round"
          />
          <circle cx="17.5" cy="8.5" r="1.6" fill="var(--accent-violet)" />
        </svg>
        <span class="brand-word">{{ t("common.appName") }}</span>
        <span class="spacer" />
        <div class="app-header-actions">
          <button
            ref="settingsTrigger"
            type="button"
            class="btn icon ghost gear"
            :aria-label="t('settings.title')"
            :aria-expanded="settingsOpen"
            @click="openSettings()"
          >
            <svg class="i" viewBox="0 0 24 24" aria-hidden="true">
              <circle cx="12" cy="12" r="3" />
              <path d="M12 3v2.5M12 18.5V21M3 12h2.5M18.5 12H21M5.6 5.6l1.8 1.8M16.6 16.6l1.8 1.8M5.6 18.4l1.8-1.8M16.6 7.4l1.8-1.8" />
            </svg>
          </button>
        </div>
      </header>
      <WelcomeScreen
        v-if="!product.session.value"
        :product="product"
        :locale="locale.resolved.value"
        :error-message="errorMessage"
        @open-settings="openSettings"
      />
      <GameWorkspace
        v-if="product.session.value"
        ref="workspace"
        :session="product.session.value"
        :product-busy="product.busy.value"
        :preferences="preferences"
        @error="gameError = $event as AppFault"
        @history-revision="historyRevision = $event"
        @open-settings="openSettings()"
      />
    </main>
    <SetupPanel
      :open="settingsOpen"
      :initial-section="settingsSection"
      :product="product"
      :theme="theme"
      :locale="locale"
      :billing="billing"
      :error-message="errorMessage"
      @reconnect="workspace?.reconnect()"
      @close="closeSettings"
    >
      <template #memory-settings>
        <MemorySettingsPanel :script-id="product.scriptId.value" />
      </template>
      <template #memory-query="{ section }">
        <MemoryQueryPanel
          v-if="product.session.value"
          :key="product.session.value.memoryRunId"
          :run-id="product.session.value.memoryRunId"
          :history-revision="historyRevision"
          :view="section === 'memory-list' ? 'list' : 'pending'"
        />
      </template>
    </SetupPanel>
  </div>
</template>
