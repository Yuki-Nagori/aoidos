<script setup lang="ts">
import { computed, ref } from "vue";
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
          :disabled="!canStart"
          @click="product.open(false)"
        >
          {{ product.busy.value ? t("common.loading") : t("game.restart") }}
        </button>
        <button
          type="button"
          class="btn lg welcome-new-cycle"
          :disabled="!canStart"
          @click="product.open(true)"
        >
          {{ t("game.newCycle") }}
        </button>
      </div>
      <button
        type="button"
        class="welcome-text-button welcome-runs-button"
        @click="runsOpen = true"
      >
        {{ t("welcome.openRuns") }}
      </button>
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
