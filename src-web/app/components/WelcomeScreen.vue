<script setup lang="ts">
import { computed, nextTick, onScopeDispose, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { Locale } from "../../api/locale";
import type { useProduct } from "../../features/product/composables/useProduct";
import RunCatalogPanel from "../../features/product/components/RunCatalogPanel.vue";
import SelectMenu from "../../shared/components/SelectMenu.vue";

const props = defineProps<{
  product: ReturnType<typeof useProduct>;
  locale: Locale;
  errorMessage?: string;
}>();
const emit = defineEmits<{ openSettings: [section: string, trigger: HTMLElement] }>();
const { t } = useI18n();
const runsOpen = ref(false);
const newCycleOpen = ref(false);
const cnyLimit = ref("");
const usdLimit = ref("");
const budgetAcknowledged = ref(false);
const preparationFailed = ref(false);
const budgetDialog = ref<HTMLElement>();
let previousBudgetFocus: HTMLElement | undefined;
const principles = computed(() => [
  {
    id: "agency",
    title: t("welcome.pillars.agency.title"),
    code: t("welcome.pillars.agency.code"),
    description: t("welcome.pillars.agency.description"),
  },
  {
    id: "causality",
    title: t("welcome.pillars.causality.title"),
    code: t("welcome.pillars.causality.code"),
    description: t("welcome.pillars.causality.description"),
  },
  {
    id: "duet",
    title: t("welcome.pillars.duet.title"),
    code: t("welcome.pillars.duet.code"),
    description: t("welcome.pillars.duet.description"),
  },
  {
    id: "ending",
    title: t("welcome.pillars.ending.title"),
    code: t("welcome.pillars.ending.code"),
    description: t("welcome.pillars.ending.description"),
  },
]);
const scenario = computed(() =>
  props.product.scripts.value.find((script) => script.scriptId === props.product.scriptId.value),
);
const scenarioName = computed(
  () => scenario.value?.displayNames[props.locale] ?? scenario.value?.title ?? t("common.loading"),
);
const modelName = computed(() =>
  props.product.models.value.find((model) => model.id === props.product.model.value)?.label ??
  props.product.model.value,
);
const providerName = computed(
  () => props.product.providerOptions.find((provider) => provider.id === props.product.providerId.value)?.label,
);
const canStart = computed(
  () =>
    !props.product.busy.value &&
    !props.product.keyStatusLoading.value &&
    props.product.keyStatus.value?.set !== false &&
    scenario.value !== undefined,
);
const validBudget = (value: string) =>
  value.length <= 32 && /^(?:0|[1-9]\d*)(?:\.\d{1,9})?$/.test(value) && (() => {
    const [whole = "0", fraction = ""] = value.split(".");
    return BigInt(whole) * 1_000_000_000n + BigInt(fraction.padEnd(9, "0") || "0") <= 9_223_372_036_854_775_807n;
  })();
watch(newCycleOpen, async (open) => {
  if (open) {
    previousBudgetFocus = document.activeElement as HTMLElement;
    await nextTick();
    budgetDialog.value?.querySelector<HTMLElement>("input:not(:disabled)")?.focus();
  } else {
    previousBudgetFocus?.focus();
    previousBudgetFocus = undefined;
  }
});
onScopeDispose(() => previousBudgetFocus?.focus());
function onBudgetDialogKeydown(event: KeyboardEvent): void {
  event.stopPropagation();
  if (event.key === "Escape") {
    if (props.product.busy.value || event.isComposing) return;
    event.preventDefault();
    newCycleOpen.value = false;
    return;
  }
  if (event.key !== "Tab" || !budgetDialog.value) return;
  const elements = [...budgetDialog.value.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled)")];
  const first = elements[0];
  const last = elements.at(-1);
  if (!first || !last) { event.preventDefault(); budgetDialog.value.focus(); return; }
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
}
const canPrepareNewCycle = computed(
  () => canStart.value && budgetAcknowledged.value && validBudget(cnyLimit.value) && validBudget(usdLimit.value),
);
function beginNewCycle(): void {
  cnyLimit.value = "";
  usdLimit.value = "";
  budgetAcknowledged.value = false;
  preparationFailed.value = false;
  newCycleOpen.value = true;
}
async function prepareNewCycle(): Promise<void> {
  if (!canPrepareNewCycle.value) return;
  budgetDialog.value?.focus();
  const sessionId = await props.product.prepareNewCycle(cnyLimit.value, usdLimit.value, props.locale);
  if (!sessionId) {
    preparationFailed.value = true;
    return;
  }
  newCycleOpen.value = false;
  runsOpen.value = true;
}
</script>

<template>
  <div class="welcome-body">
    <section class="hero" aria-labelledby="welcome-title">
      <div class="welcome-eyebrow"><span aria-hidden="true" />{{ t("welcome.eyebrow") }}</div>
      <h1 id="welcome-title">{{ t("welcome.headline") }}</h1>
      <p class="lede">{{ t("welcome.lede") }}</p>
      <div class="pillars" :aria-label="t('welcome.principles')">
        <article v-for="principle in principles" :key="principle.id" class="pillar">
          <div class="pillar-heading">
            <span class="dot" :class="`dot-${principle.id}`" aria-hidden="true" />
            <strong>{{ principle.title }}</strong>
            <span class="pillar-code">{{ principle.code }}</span>
          </div>
          <span class="pillar-description">{{ principle.description }}</span>
        </article>
      </div>
    </section>

    <section class="setup glass" aria-labelledby="welcome-setup-title">
      <div class="setup-heading">
        <span id="welcome-setup-title" class="setup-title">{{ t("welcome.startTitle") }}</span>
        <span class="note">{{ t("game.intro") }}</span>
      </div>

      <div class="field">
        <div class="lbl">
          <span>{{ t("settings.script") }}</span>
          <button
            type="button"
            class="small muted welcome-text-button"
            @click="emit('openSettings', 'scenario', $event.currentTarget as HTMLElement)"
          >
            {{ t("settings.credits") }}
          </button>
        </div>
        <SelectMenu
          class="welcome-script-select"
          :model-value="product.scriptId.value"
          :options="product.scripts.value.map((script) => ({
            value: script.scriptId,
            label: script.displayNames[locale] ?? script.title,
          }))"
          :label="t('settings.script')"
          :disabled="product.busy.value"
          @update:model-value="product.scriptId.value = $event"
        >
          <template #trigger>
            <span class="card script-card">
              <span class="script-ic" aria-hidden="true">
                <svg class="i lg" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                  <path d="M6 16v-5a6 6 0 0 1 12 0v5l1.5 2h-15z" />
                  <path d="M10 20a2 2 0 0 0 4 0" />
                </svg>
              </span>
              <span class="script-copy">
                <strong>{{ scenarioName }}</strong>
                <span>{{
                  t("welcome.selectedScenario", { id: scenario?.scriptId ?? "—" })
                }}</span>
              </span>
              <svg class="i sm muted-icon" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="m6 9 6 6 6-6" />
              </svg>
            </span>
          </template>
        </SelectMenu>
      </div>

      <div class="field">
        <div class="lbl"><span>{{ t("settings.model") }}</span></div>
        <div class="card model-card">
          <svg class="i" viewBox="0 0 24 24" fill="none" aria-hidden="true">
            <rect x="6" y="6" width="12" height="12" rx="2" />
            <path d="M10 3v3m4-3v3m-4 12v3m4-3v3M3 10h3m-3 4h3m12-4h3m-3 4h3" />
          </svg>
          <span class="model-copy">
            <span class="mono small model-name">{{ modelName }}</span>
            <span class="model-provider small muted">
              {{ providerName }}<span aria-hidden="true">·</span>
              <span
                class="led"
                :class="{ accent: product.keyStatus.value?.set }"
                aria-hidden="true"
              />
              <template v-if="product.keyStatusLoading.value">{{
                t("settings.keyChecking")
              }}</template>
              <template v-else-if="product.keyStatus.value?.set">{{
                t("settings.keySet")
              }}</template>
              <template v-else-if="product.keyStatus.value?.set === false">{{
                t("settings.keyMissing")
              }}</template>
              <template v-else>{{ t("settings.keyUnknown") }}</template>
            </span>
          </span>
          <button
            type="button"
            class="btn ghost model-settings"
            @click="emit('openSettings', 'api', $event.currentTarget as HTMLElement)"
          >
            {{ t("welcome.changeInSettings") }}
          </button>
        </div>
      </div>

      <p v-if="product.keyStatus.value?.set === false" class="welcome-notice" role="status">
        {{ t("settings.startNeedsKey") }}
        <button
          type="button"
          class="welcome-text-button"
          @click="emit('openSettings', 'api', $event.currentTarget as HTMLElement)"
        >
          {{ t("settings.openSettings") }}
        </button>
      </p>
      <p
        v-else-if="!product.keyStatusLoading.value && !product.keyStatus.value"
        class="welcome-notice"
        role="status"
      >
        {{ t("settings.keyUnknown") }}
      </p>
      <p v-if="errorMessage" class="welcome-notice" role="alert">{{ errorMessage }}</p>

      <div class="row wrap start-actions">
        <button
          type="button"
          class="btn primary lg welcome-start"
          @click="runsOpen = true"
        >
          {{ t("welcome.openRuns") }}
        </button>
        <button
          type="button"
          class="btn lg welcome-new-cycle"
          :disabled="!canStart"
          @click="beginNewCycle"
        >
          {{ t("game.newCycle") }}
        </button>
      </div>
      <div class="hr" />
      <button
        type="button"
        class="settings-hint"
        @click="emit('openSettings', 'appearance', $event.currentTarget as HTMLElement)"
      >
        <svg class="i sm" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <circle cx="12" cy="12" r="3" />
          <path d="M12 3v2.5M12 18.5V21M3 12h2.5M18.5 12H21M5.6 5.6l1.8 1.8M16.6 16.6l1.8 1.8M5.6 18.4l1.8-1.8M16.6 7.4l1.8-1.8" />
        </svg>
        <span>{{ t("welcome.settingsHint") }}</span>
        <span class="spacer" />
        <svg class="i sm" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path d="m9 6 6 6-6 6" />
        </svg>
      </button>
    </section>
    <div v-if="newCycleOpen" class="welcome-budget-backdrop">
      <section
        ref="budgetDialog"
        class="welcome-budget-dialog glass"
        role="dialog"
        aria-modal="true"
        tabindex="-1"
        :aria-label="t('welcome.newCycleBudgetTitle')"
        @keydown="onBudgetDialogKeydown"
      >
        <h2>{{ t("welcome.newCycleBudgetTitle") }}</h2>
        <p>{{ t("welcome.newCycleBudgetDescription") }}</p>
        <p class="welcome-budget-note">{{ t("welcome.newCyclePreparationOnly") }}</p>
        <label class="welcome-budget-field">
          {{ t("runs.cnyLimit") }}
          <input
            v-model="cnyLimit"
            inputmode="decimal"
            maxlength="32"
            autocomplete="off"
            :disabled="product.busy.value"
          />
        </label>
        <p
          v-if="(cnyLimit && !validBudget(cnyLimit)) || (usdLimit && !validBudget(usdLimit))"
          class="welcome-budget-error"
          role="alert"
        >
          {{ t("welcome.newCycleBudgetInvalid") }}
        </p>
        <p
          v-if="preparationFailed && product.error.value"
          class="welcome-budget-error"
          role="alert"
        >
          {{ product.error.value.message }}
        </p>
        <label class="welcome-budget-field">
          {{ t("runs.usdLimit") }}
          <input
            v-model="usdLimit"
            inputmode="decimal"
            maxlength="32"
            autocomplete="off"
            :disabled="product.busy.value"
          />
        </label>
        <label class="welcome-budget-ack">
          <input v-model="budgetAcknowledged" type="checkbox" :disabled="product.busy.value" />
          {{ t("welcome.newCycleBudgetAck") }}
        </label>
        <div class="row wrap welcome-budget-actions">
          <button
            type="button"
            class="btn"
            :disabled="product.busy.value"
            @click="newCycleOpen = false"
          >
            {{ t("common.cancel") }}
          </button>
          <button
            type="button"
            class="btn primary"
            :disabled="!canPrepareNewCycle"
            @click="prepareNewCycle"
          >
            {{ product.busy.value ? t("common.loading") : t("welcome.prepareNewCycle") }}
          </button>
        </div>
      </section>
    </div>
    <RunCatalogPanel
      :open="runsOpen"
      :scripts="product.scripts.value"
      :profiles="product.profiles.value"
      :locale="locale"
      @close="runsOpen = false"
      @restore="product.adoptOpenedRun($event)"
    />
  </div>
</template>

<style scoped>
.welcome-budget-backdrop { position: fixed; inset: 0; z-index: 100; display: grid; place-items: center; padding: 1rem; background: rgb(0 0 0 / 60%); }
.welcome-budget-dialog { width: min(100%, 28rem); padding: 1.25rem; border: 1px solid var(--hairline); border-radius: 1rem; background: var(--panel); box-shadow: 0 1.5rem 5rem rgb(0 0 0 / 40%); }
.welcome-budget-dialog h2 { margin: 0; font-size: 1.25rem; }
.welcome-budget-dialog p { color: var(--muted); font-size: .875rem; }
.welcome-budget-note { padding: .625rem .75rem; border-left: 3px solid var(--accent); background: color-mix(in srgb, var(--accent) 8%, transparent); }
.welcome-budget-error { color: var(--danger, #ff7b72); }
.welcome-budget-field { display: grid; gap: .375rem; margin-top: .875rem; font-size: .8125rem; }
.welcome-budget-field input { min-width: 0; border: 1px solid var(--hairline); border-radius: .5rem; padding: .625rem .75rem; color: var(--ink); background: var(--surface); font: inherit; }
.welcome-budget-ack { display: flex; gap: .5rem; align-items: flex-start; margin-top: 1rem; font-size: .8125rem; }
.welcome-budget-ack input { accent-color: var(--accent); margin-top: .2rem; }
.welcome-budget-actions { justify-content: flex-end; margin-top: 1rem; }
</style>
