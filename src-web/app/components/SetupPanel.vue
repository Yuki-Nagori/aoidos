<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import BillingPanel from "../../features/billing/components/BillingPanel.vue";
import { useI18n } from "vue-i18n";
import type { useProduct } from "../../features/product/composables/useProduct";
import type { useTheme } from "../../features/theme/composables/useTheme";
import type { useLocale } from "../../features/locale/composables/useLocale";
import type { useBilling } from "../../features/billing/composables/useBilling";
import type { useUiPreferences } from "../../features/game/composables/useUiPreferences";

const { t } = useI18n();
const { open, initialSection, product, theme, locale, billing, preferences, errorMessage } = defineProps<{
  open: boolean;
  initialSection: string;
  product: ReturnType<typeof useProduct>;
  theme: ReturnType<typeof useTheme>;
  locale: ReturnType<typeof useLocale>;
  billing: ReturnType<typeof useBilling>;
  preferences: ReturnType<typeof useUiPreferences>;
  errorMessage?: string;
}>();
const emit = defineEmits<{ reconnect: []; close: [] }>();
const panel = ref<HTMLElement>();
const activeSection = ref("appearance");
const sections = computed(() => [
  { id: "appearance", label: t("settings.sections.appearance") },
  { id: "api", label: t("settings.sections.api") },
  { id: "gameplay", label: t("settings.sections.gameplay") },
  { id: "billing", label: t("settings.sections.billing") },
  { id: "price", label: t("settings.sections.price") },
  { id: "balance", label: t("settings.sections.balance") },
  { id: "memory", label: t("settings.sections.memory") },
  { id: "memory-list", label: t("settings.sections.memory-list") },
  { id: "memory-pending", label: t("settings.sections.memory-pending") },
  { id: "scenario", label: t("settings.sections.scenario") },
]);
const activeSectionTitle = computed(
  () => sections.value.find((section) => section.id === activeSection.value)?.label ?? t("settings.title"),
);
watch(
  () => open,
  async (isOpen) => {
    if (!isOpen) return;
    activeSection.value = initialSection;
    await nextTick();
    panel.value?.querySelector<HTMLElement>("button, select, input")?.focus();
  },
);
function onKeydown(event: KeyboardEvent): void {
  event.stopPropagation();
  if (event.key === "Escape") {
    event.preventDefault();
    emit("close");
    return;
  }
  if (event.key !== "Tab" || !panel.value) return;
  const focusable = Array.from(
    panel.value.querySelectorAll<HTMLElement>(
      'button:not(:disabled), select:not(:disabled), input:not(:disabled), a[href], [tabindex]:not([tabindex="-1"])',
    ),
  ).filter((element) => getComputedStyle(element).display !== "none");
  const first = focusable[0];
  const last = focusable.at(-1);
  if (!first || !last) return;
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}
const selectedScript = computed(() =>
  product.scripts.value.find((script) => script.scriptId === product.scriptId.value),
);
const selectedProviderLabel = computed(
  () =>
    product.providerOptions.find((option) => option.id === product.providerId.value)?.label ??
    product.providerId.value,
);
const localeNotice = computed(() => {
  if (locale.notice.value === "pending") return t("settings.languagePending");
  if (locale.notice.value === "saveFailed") return t("settings.languageSaveFailed");
  if (locale.notice.value === "loadFailed") return t("settings.languageLoadFailed");
  if (locale.fallback.value) return t("settings.languageFallback");
  return undefined;
});
const themeErrorMessage = computed(() => {
  switch (theme.themeError.value) {
    case "storageUnavailable":
      return t("settings.themeErrors.storageUnavailable");
    case "invalidPreference":
      return t("settings.themeErrors.invalidPreference");
    case "themeUnavailable":
      return t("settings.themeErrors.themeUnavailable");
    case "saveFailed":
      return t("settings.themeErrors.saveFailed");
    case "readFailed":
      return t("settings.themeErrors.readFailed");
    case "catalogUnavailable":
      return t("settings.themeErrors.catalogUnavailable");
    default:
      return undefined;
  }
});
const skinErrorMessage = computed(() => {
  switch (theme.skinError.value) {
    case "invalidValue":
      return t("settings.skinErrors.invalidValue");
    case "unsupportedWebView":
      return t("settings.skinErrors.unsupportedWebView");
    case "applyFailed":
      return t("settings.skinErrors.applyFailed");
    case "readFailed":
      return t("settings.skinErrors.readFailed");
    default:
      return undefined;
  }
});
function themeName(id: string, fallback: string): string {
  switch (id) {
    case "dark":
      return t("settings.themeNames.dark");
    case "light":
      return t("settings.themeNames.light");
    case "light-purple":
      return t("settings.themeNames.lightPurple");
    default:
      return fallback;
  }
}
</script>
<template>
  <div v-if="open" class="settings-backdrop">
    <section
      ref="panel"
      class="settings-dialog"
      role="dialog"
      aria-modal="true"
      :aria-labelledby="'settings-heading'"
      tabindex="-1"
      @keydown="onKeydown"
    >
      <div class="settings-layout">
        <nav class="settings-nav" :aria-label="t('settings.title')">
          <div class="settings-nav-heading">
            <h2 id="settings-heading">{{ t("settings.title") }}</h2>
            <button
              type="button"
              class="settings-close"
              :aria-label="t('common.close')"
              @click="emit('close')"
            >
              <span aria-hidden="true">×</span>
            </button>
          </div>
          <button
            v-for="section in sections"
            :key="section.id"
            type="button"
            :aria-current="activeSection === section.id ? 'page' : undefined"
            :class="{ 'is-subsection': ['price', 'balance', 'memory-list', 'memory-pending'].includes(section.id) }"
            :aria-controls="['price', 'balance'].includes(section.id) ? 'billing' : ['memory-list', 'memory-pending'].includes(section.id) ? 'memory-query' : section.id"
            @click="activeSection = section.id"
          >
            {{ section.label }}
          </button>
        </nav>
        <div class="settings-content">
          <section v-show="activeSection === 'appearance'" id="appearance" class="settings-section">
            <h3>{{ t("settings.sections.appearance") }}</h3>
            <label>
              {{ t("settings.theme") }}
              <select
                :value="theme.theme.value"
                :aria-label="t('settings.theme')"
                :disabled="theme.saving.value"
                @change="theme.setTheme(($event.target as HTMLSelectElement).value)"
              >
                <option v-for="option in theme.themes.value" :key="option.id" :value="option.id">
                  {{ themeName(option.id, option.name) }}
                </option>
              </select>
            </label>
            <div v-if="product.scriptId.value === 'mistbell'" class="settings-skin card">
              <div class="settings-skin-copy">
                <strong>{{
                  selectedScript?.displayNames[locale.resolved.value] ?? selectedScript?.title
                }}</strong>
                <span class="text-sm text-muted">{{ t("settings.scriptSkin") }}</span>
              </div>
              <button
                type="button"
                role="switch"
                class="settings-switch"
                :aria-checked="!theme.disabled.value"
                :aria-label="t('settings.scriptSkin')"
                @click="theme.toggleSkin"
              >
                <span aria-hidden="true" />
              </button>
              <span class="text-sm">{{
                theme.disabled.value ? t("settings.enableSkin") : t("settings.disableSkin")
              }}</span>
            </div>
            <p v-if="themeErrorMessage" role="status">{{ themeErrorMessage }}</p>
            <p v-if="skinErrorMessage" role="status">{{ skinErrorMessage }}</p>
            <p v-else-if="theme.warningCount.value">
              {{ t("settings.skinWarnings", { count: theme.warningCount.value }) }}
            </p>
            <fieldset class="settings-choice-group">
              <legend>{{ t("settings.language") }}</legend>
              <div class="settings-segmented">
                <label
                  :class="{
                    'is-selected': locale.choice.value === 'system',
                    'is-disabled': !locale.ready.value || locale.saving.value,
                  }"
                >
                  <input
                    type="radio"
                    name="interface-language"
                    value="system"
                    :checked="locale.choice.value === 'system'"
                    :disabled="!locale.ready.value || locale.saving.value"
                    @change="locale.setChoice('system')"
                  />
                  <span>{{
                    t("settings.systemLanguageCurrent", {
                      locale: t(locale.resolved.value === "zh-Hans" ? "settings.chinese" : "settings.english"),
                    })
                  }}</span>
                </label>
                <label
                  :class="{
                    'is-selected': locale.choice.value === 'zh-Hans',
                    'is-disabled': !locale.ready.value || locale.saving.value,
                  }"
                >
                  <input
                    type="radio"
                    name="interface-language"
                    value="zh-Hans"
                    :checked="locale.choice.value === 'zh-Hans'"
                    :disabled="!locale.ready.value || locale.saving.value"
                    @change="locale.setChoice('zh-Hans')"
                  />
                  <span>{{ t("settings.chinese") }}</span>
                </label>
                <label
                  :class="{
                    'is-selected': locale.choice.value === 'en',
                    'is-disabled': !locale.ready.value || locale.saving.value,
                  }"
                >
                  <input
                    type="radio"
                    name="interface-language"
                    value="en"
                    :checked="locale.choice.value === 'en'"
                    :disabled="!locale.ready.value || locale.saving.value"
                    @change="locale.setChoice('en')"
                  />
                  <span>{{ t("settings.english") }}</span>
                </label>
              </div>
            </fieldset>
            <p v-if="localeNotice" role="status">{{ localeNotice }}</p>
          </section>

          <section v-show="activeSection === 'api'" id="api" class="settings-section">
            <h3>{{ t("settings.sections.api") }}</h3>
            <div class="settings-form-grid">
              <label>
                {{ t("settings.provider") }}
                <select
                  v-model="product.providerId.value"
                  :aria-label="t('settings.provider')"
                  :disabled="product.busy.value"
                >
                  <option
                    v-for="provider in product.providerOptions"
                    :key="provider.id"
                    :value="provider.id"
                  >
                    {{ provider.label }}
                  </option>
                </select>
              </label>
              <label>
                {{ t("settings.model") }}
                <select
                  v-model="product.model.value"
                  :aria-label="t('settings.model')"
                  :disabled="product.busy.value"
                >
                  <option v-for="model in product.models.value" :key="model.id" :value="model.id">
                    {{ model.label }}
                  </option>
                </select>
              </label>
            </div>
            <div
              class="settings-key-card card"
              :class="{ 'is-missing': !product.keyStatusLoading.value && product.keyStatus.value?.set !== true }"
            >
              <span
                class="led"
                :class="{
                  accent: product.keyStatus.value?.set === true,
                  warn: !product.keyStatusLoading.value && product.keyStatus.value?.set !== true,
                }"
                aria-hidden="true"
              />
              <span class="settings-skin-copy">
                <strong>{{ t("settings.keyStatus") }}</strong>
                <span class="text-sm text-muted" role="status">
                  {{
                    product.keyStatusLoading.value
                    ? t("settings.keyChecking")
                    : product.keyStatus.value?.set === true
                      ? t("settings.keySet")
                      : product.keyStatus.value?.set === false
                        ? t("settings.keyMissing")
                        : t("settings.keyUnknown")
                  }}
                </span>
              </span>
              <button
                type="button"
                class="btn"
                :disabled="product.busy.value"
                @click="product.configureKey"
              >
                {{ t("settings.setApiKey") }}
              </button>
              <button
                v-if="!product.keyStatus.value && !product.keyStatusLoading.value"
                type="button"
                class="btn ghost"
                @click="product.refreshKeyStatus()"
              >
                {{ t("common.retry") }}
              </button>
            </div>
          </section>

          <section v-show="activeSection === 'gameplay'" id="gameplay" class="settings-section">
            <h3>{{ t("settings.sections.gameplay") }}</h3>
            <label>
              {{ t("settings.script") }}
              <select v-model="product.scriptId.value" :disabled="product.busy.value">
                <option
                  v-for="script in product.scripts.value"
                  :key="script.scriptId"
                  :value="script.scriptId"
                >
                  {{ script.displayNames[locale.resolved.value] ?? script.title }}
                </option>
              </select>
            </label>
            <fieldset class="settings-choice-group">
              <legend>{{ t("game.diceMode") }}</legend>
              <div class="settings-segmented">
                <label
                  :class="{
                    'is-selected': preferences.diceMode.value === 'manual',
                    'is-disabled': !preferences.ready.value || preferences.saving.value,
                  }"
                >
                  <input
                    type="radio"
                    name="dice-mode"
                    value="manual"
                    :checked="preferences.diceMode.value === 'manual'"
                    :disabled="!preferences.ready.value || preferences.saving.value"
                    @change="preferences.setDiceMode('manual')"
                  />
                  <span>{{ t("game.manual") }}</span>
                </label>
                <label
                  :class="{
                    'is-selected': preferences.diceMode.value === 'auto',
                    'is-disabled': !preferences.ready.value || preferences.saving.value,
                  }"
                >
                  <input
                    type="radio"
                    name="dice-mode"
                    value="auto"
                    :checked="preferences.diceMode.value === 'auto'"
                    :disabled="!preferences.ready.value || preferences.saving.value"
                    @change="preferences.setDiceMode('auto')"
                  />
                  <span>{{ t("game.automatic") }}</span>
                </label>
              </div>
            </fieldset>
            <div class="settings-actions">
              <button
                type="button"
                :disabled="product.busy.value || product.keyStatus.value?.set !== true"
                @click="product.open(false)"
              >
                {{ t("game.restart") }}
              </button>
              <button
                type="button"
                :disabled="product.busy.value || product.keyStatus.value?.set !== true"
                @click="product.open(true)"
              >
                {{ t("game.newCycle") }}
              </button>
              <button type="button" @click="emit('reconnect')">{{ t("common.reconnect") }}</button>
            </div>
          </section>

          <section
            v-show="['billing', 'price', 'balance'].includes(activeSection)"
            id="billing"
            class="settings-section"
          >
            <h3>{{ activeSectionTitle }}</h3>
            <BillingPanel
              :billing="billing"
              :locale="locale.resolved.value"
              :active="Boolean(product.session.value)"
              :provider-id="product.providerId.value"
              :provider-label="selectedProviderLabel"
              :model-id="product.model.value"
              :view="activeSection === 'price' || activeSection === 'balance' ? activeSection : 'billing'"
            />
            <p v-if="!product.session.value" class="text-sm text-muted">
              {{ t("settings.billingAfterStart") }}
            </p>
          </section>

          <section v-show="activeSection === 'memory'" id="memory" class="settings-section">
            <h3>{{ t("settings.sections.memory") }}</h3>
            <slot name="memory-settings" />
            <p v-if="!product.session.value" class="text-sm text-muted">
              {{ t("settings.memoryAfterStart") }}
            </p>
          </section>

          <section
            v-show="activeSection === 'memory-list' || activeSection === 'memory-pending'"
            id="memory-query"
            class="settings-section"
          >
            <h3>{{ activeSectionTitle }}</h3>
            <slot name="memory-query" :section="activeSection" />
            <p v-if="!product.session.value" class="text-sm text-muted">
              {{ t("settings.memoryAfterStart") }}
            </p>
          </section>

          <section v-show="activeSection === 'scenario'" id="scenario" class="settings-section">
            <h3>{{ t("settings.sections.scenario") }}</h3>
            <div v-if="selectedScript" class="card scenario-script-card">
              <span class="script-ic" aria-hidden="true">
                <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
                  <path d="M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15z" />
                  <path d="M10 20a2 2 0 0 0 4 0" />
                </svg>
              </span>
              <div class="scenario-script-copy">
                <strong>{{
                  selectedScript.displayNames[locale.resolved.value] ?? selectedScript.title
                }}</strong>
                <code>{{ selectedScript.scriptId }}</code>
              </div>
            </div>
            <section
              v-if="selectedScript?.attributions"
              class="scenario-attributions"
              :aria-label="t('settings.credits')"
            >
              <h4>{{ t("settings.credits") }}</h4>
              <pre>{{ selectedScript.attributions }}</pre>
            </section>
          </section>
        </div>
      </div>
      <p v-if="errorMessage" class="settings-error" role="alert">{{ errorMessage }}</p>
    </section>
  </div>
</template>
