<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import BillingPanel from "../../features/billing/components/BillingPanel.vue";
import SettingsSectionHeading from "./SettingsSectionHeading.vue";
import SettingsSectionIcon from "./SettingsSectionIcon.vue";
import SelectMenu from "../../shared/components/SelectMenu.vue";
import KeyboardShortcuts from "./KeyboardShortcuts.vue";
import { useI18n } from "vue-i18n";
import type { useProduct } from "../../features/product/composables/useProduct";
import type { useTheme } from "../../features/theme/composables/useTheme";
import type { useLocale } from "../../features/locale/composables/useLocale";
import type { useBilling } from "../../features/billing/composables/useBilling";
import type { useUiPreferences } from "../../features/game/composables/useUiPreferences";
import { parseAttributionDocuments } from "../utils/attributions";

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
const selectedPriceTemplate = computed(() =>
  product.priceTemplates.value.find((price) => price.currency === product.billingCurrency.value),
);
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
  { id: "shortcuts", label: t("settings.sections.shortcuts") },
] as const);
const activeSectionTitle = computed(
  () => sections.value.find((section) => section.id === activeSection.value)?.label ?? t("settings.title"),
);
watch(
  () => open,
  async (isOpen) => {
    if (!isOpen) return;
    activeSection.value = initialSection;
    await nextTick();
    focusableElements()[0]?.focus();
  },
);
function isVisible(element: HTMLElement): boolean {
  const collapsedDetails = element.closest("details:not([open])");
  if (collapsedDetails && !element.matches("summary")) return false;
  let current: HTMLElement | null = element;
  while (current && panel.value?.contains(current)) {
    const style = getComputedStyle(current);
    if (
      current.hidden ||
      current.inert ||
      style.display === "none" ||
      style.visibility === "hidden" ||
      style.visibility === "collapse"
    ) {
      return false;
    }
    current = current.parentElement;
  }
  return true;
}
function focusableElements(): HTMLElement[] {
  if (!panel.value) return [];
  return Array.from(
    panel.value.querySelectorAll<HTMLElement>(
      'button:not(:disabled), input:not(:disabled), textarea:not(:disabled), a[href], summary, [contenteditable="true"], [tabindex]:not([tabindex="-1"])',
    ),
  ).filter(isVisible);
}
function onKeydown(event: KeyboardEvent): void {
  event.stopPropagation();
  if (event.key === "Escape") {
    if (isComposingEvent(event)) return;
    event.preventDefault();
    emit("close");
    return;
  }
  if (event.key !== "Tab" || !panel.value) return;
  const focusable = focusableElements();
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
function isComposingEvent(event: KeyboardEvent): boolean {
  return event.isComposing || event.key === "Process";
}
const selectedScript = computed(() =>
  product.scripts.value.find((script) => script.scriptId === product.scriptId.value),
);
const attributionDocuments = computed(() => {
  const source = selectedScript.value?.attributions.trim();
  return source ? parseAttributionDocuments(source) : [];
});
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
            <SettingsSectionIcon :name="section.id" />
            {{ section.label }}
          </button>
        </nav>
        <div class="settings-content">
          <section v-show="activeSection === 'appearance'" id="appearance" class="settings-section">
            <SettingsSectionHeading
              :title="t('settings.sections.appearance')"
              @close="emit('close')"
            />
            <fieldset class="settings-theme-field">
              <legend class="settings-field-label">{{ t("settings.theme") }}</legend>
              <div class="settings-theme-grid">
                <button
                  v-for="option in theme.themes.value"
                  :key="option.id"
                  type="button"
                  class="settings-theme-card"
                  :class="{ 'is-selected': theme.theme.value === option.id }"
                  :aria-label="themeName(option.id, option.name)"
                  :aria-pressed="theme.theme.value === option.id"
                  :disabled="theme.saving.value"
                  @click="theme.setTheme(option.id)"
                >
                  <span class="settings-theme-preview" :data-theme="option.id" aria-hidden="true">
                    <span />
                    <span />
                  </span>
                  <span class="settings-theme-name">
                    {{ themeName(option.id, option.name) }}
                    <svg
                      v-if="theme.theme.value === option.id"
                      class="settings-theme-check"
                      viewBox="0 0 24 24"
                      aria-hidden="true"
                    >
                      <path d="m5 12.5 4.5 4.5L19 7.5" />
                    </svg>
                  </span>
                </button>
              </div>
            </fieldset>
            <section
              v-if="product.scriptId.value === 'mistbell'"
              class="settings-skin-section"
              :aria-label="t('settings.scriptSkin')"
            >
              <div class="settings-skin-heading">
                <strong>{{ t("settings.scriptSkin") }}</strong>
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
              </div>
              <div class="settings-skin card">
                <span class="script-ic" aria-hidden="true">
                  <svg class="i" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                    <path d="M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15z" />
                    <path d="M10 20a2 2 0 0 0 4 0" />
                  </svg>
                </span>
                <div class="settings-skin-copy">
                  <strong>{{
                    selectedScript?.displayNames[locale.resolved.value] ?? selectedScript?.title
                  }}</strong>
                  <span v-if="theme.skinTokenCount.value !== null" class="text-sm text-muted">
                    {{ t("settings.skinTokenSummary", { count: theme.skinTokenCount.value }) }}
                  </span>
                </div>
                <button type="button" class="btn" @click="theme.toggleSkin">
                  {{ theme.disabled.value ? t("settings.enableSkin") : t("settings.disableSkin") }}
                </button>
              </div>
              <div v-if="theme.warningCount.value" class="settings-skin-warning">
                <svg class="i" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                  <path d="M12 4 21 19H3z" />
                  <path d="M12 10v4M12 16.6v.1" />
                </svg>
                <span>{{ t("settings.skinWarnings", { count: theme.warningCount.value }) }}</span>
              </div>
            </section>
            <p v-if="themeErrorMessage" role="status">{{ themeErrorMessage }}</p>
            <p v-if="skinErrorMessage" role="status">{{ skinErrorMessage }}</p>
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
            <SettingsSectionHeading :title="t('settings.sections.api')" @close="emit('close')" />
            <div class="settings-form-grid">
              <div class="settings-field">
                {{ t("settings.provider") }}
                <SelectMenu
                  :model-value="product.providerId.value"
                  :options="product.providerOptions.map((provider) => ({
                    value: provider.id,
                    label: provider.label,
                  }))"
                  :label="t('settings.provider')"
                  :disabled="product.busy.value"
                  @update:model-value="product.providerId.value = $event"
                />
              </div>
              <div class="settings-field">
                {{ t("settings.model") }}
                <SelectMenu
                  :model-value="product.model.value"
                  :options="product.models.value.map((model) => ({ value: model.id, label: model.label }))"
                  :label="t('settings.model')"
                  :disabled="product.busy.value"
                  @update:model-value="product.model.value = $event"
                />
              </div>
            </div>
            <div
              v-if="product.providerId.value === 'deepseek' && product.model.value === 'deepseek-flash'"
              class="settings-currency-card card"
            >
              <div class="settings-currency-heading">
                <span class="settings-skin-copy">
                  <strong>{{ t("settings.billingCurrency") }}</strong>
                  <span class="text-sm text-muted">{{ t("settings.billingCurrencyHint") }}</span>
                </span>
                <SelectMenu
                  :model-value="product.billingCurrency.value"
                  :options="[
                    { value: 'CNY', label: t('settings.billingCurrencyCny') },
                    { value: 'USD', label: t('settings.billingCurrencyUsd') },
                  ]"
                  :label="t('settings.billingCurrency')"
                  :disabled="product.busy.value"
                  @update:model-value="product.setBillingCurrency($event)"
                />
              </div>
              <fieldset v-if="selectedPriceTemplate" class="settings-price-preview">
                <legend>{{ t("settings.pricePreviewTitle") }}</legend>
                <dl>
                  <div>
                    <dt>{{ t("settings.priceInputUncached") }}</dt>
                    <dd>{{ selectedPriceTemplate.inputUncached }}</dd>
                  </div>
                  <div>
                    <dt>{{ t("settings.priceInputCached") }}</dt>
                    <dd>{{ selectedPriceTemplate.inputCached }}</dd>
                  </div>
                  <div>
                    <dt>{{ t("settings.priceInputCachedWrite") }}</dt>
                    <dd>{{ selectedPriceTemplate.inputCachedWrite ?? "—" }}</dd>
                  </div>
                  <div>
                    <dt>{{ t("settings.priceOutput") }}</dt>
                    <dd>{{ selectedPriceTemplate.output }}</dd>
                  </div>
                  <template v-if="selectedPriceTemplate.schedule">
                    <div>
                      <dt>{{ t("settings.pricePeakInputUncached") }}</dt>
                      <dd>{{ selectedPriceTemplate.schedule.peak.inputUncached }}</dd>
                    </div>
                    <div>
                      <dt>{{ t("settings.pricePeakInputCached") }}</dt>
                      <dd>{{ selectedPriceTemplate.schedule.peak.inputCached }}</dd>
                    </div>
                    <div>
                      <dt>{{ t("settings.pricePeakInputCachedWrite") }}</dt>
                      <dd>{{ selectedPriceTemplate.schedule.peak.inputCachedWrite ?? "—" }}</dd>
                    </div>
                    <div>
                      <dt>{{ t("settings.pricePeakOutput") }}</dt>
                      <dd>{{ selectedPriceTemplate.schedule.peak.output }}</dd>
                    </div>
                  </template>
                </dl>
                <p class="text-sm text-muted">
                  {{
                    t("settings.priceCheckedAt", { date: selectedPriceTemplate.checkedAt.slice(0, 10) })
                  }}
                  ·
                  <a :href="selectedPriceTemplate.sourceUrl" target="_blank" rel="noreferrer">
                    {{ t("settings.priceSource") }}
                  </a>
                </p>
              </fieldset>
              <p v-else class="text-sm text-muted" role="status">
                {{ t("settings.priceTemplateUnavailable") }}
              </p>
              <button
                type="button"
                class="btn ghost"
                :disabled="product.busy.value || !selectedPriceTemplate"
                @click="product.confirmBillingPrice"
              >
                {{ t("settings.confirmBillingPrice") }}
              </button>
            </div>
            <div
              class="settings-key-card card"
              :class="{ 'is-missing': !product.keyStatusLoading.value && product.keyStatus.value?.set === false }"
            >
              <span
                class="led"
                :class="{
                  accent: product.keyStatus.value?.set === true,
                  warn: !product.keyStatusLoading.value && product.keyStatus.value?.set === false,
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
            <SettingsSectionHeading
              :title="t('settings.sections.gameplay')"
              @close="emit('close')"
            />
            <div class="settings-field">
              {{ t("settings.script") }}
              <SelectMenu
                :model-value="product.scriptId.value"
                :options="product.scripts.value.map((script) => ({
                  value: script.scriptId,
                  label: script.displayNames[locale.resolved.value] ?? script.title,
                }))"
                :label="t('settings.script')"
                :disabled="product.busy.value"
                @update:model-value="product.scriptId.value = $event"
              />
            </div>
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
                :disabled="product.busy.value || product.keyStatusLoading.value || product.keyStatus.value?.set === false"
                @click="product.open(false)"
              >
                {{ t("game.restart") }}
              </button>
              <button
                type="button"
                :disabled="product.busy.value || product.keyStatusLoading.value || product.keyStatus.value?.set === false"
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
            <SettingsSectionHeading :title="activeSectionTitle" @close="emit('close')" />
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
            <SettingsSectionHeading :title="t('settings.sections.memory')" @close="emit('close')" />
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
            <SettingsSectionHeading :title="activeSectionTitle" @close="emit('close')" />
            <slot name="memory-query" :section="activeSection" />
            <p v-if="!product.session.value" class="text-sm text-muted">
              {{ t("settings.memoryAfterStart") }}
            </p>
          </section>

          <section v-show="activeSection === 'scenario'" id="scenario" class="settings-section">
            <SettingsSectionHeading
              :title="t('settings.sections.scenario')"
              @close="emit('close')"
            />
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
              <section
                v-for="document in attributionDocuments"
                :key="document.id"
                class="scenario-attribution-section"
              >
                <div class="scenario-attribution-heading">
                  {{ document.heading ?? t("settings.credits") }}
                </div>
                <details class="scenario-attribution-document">
                  <summary>{{ t("settings.viewFullAttribution") }}</summary>
                  <div class="scenario-attribution-body">
                    <template v-for="(block, index) in document.blocks" :key="index">
                      <h6 v-if="block.kind === 'heading'">
                        <template v-for="(part, partIndex) in block.parts" :key="partIndex">
                          <a
                            v-if="part.kind === 'link'"
                            :href="part.href"
                            target="_blank"
                            rel="noopener noreferrer"
                          >{{ part.value }}</a>
                          <strong v-else-if="part.kind === 'strong'">{{ part.value }}</strong>
                          <code v-else-if="part.kind === 'code'">{{ part.value }}</code>
                          <template v-else>{{ part.value }}</template>
                        </template>
                      </h6>
                      <p v-else-if="block.kind === 'paragraph'">
                        <template v-for="(part, partIndex) in block.parts" :key="partIndex">
                          <a
                            v-if="part.kind === 'link'"
                            :href="part.href"
                            target="_blank"
                            rel="noopener noreferrer"
                          >{{ part.value }}</a>
                          <strong v-else-if="part.kind === 'strong'">{{ part.value }}</strong>
                          <code v-else-if="part.kind === 'code'">{{ part.value }}</code>
                          <template v-else>{{ part.value }}</template>
                        </template>
                      </p>
                      <ul v-else>
                        <li v-for="(item, itemIndex) in block.items" :key="itemIndex">
                          <template v-for="(part, partIndex) in item" :key="partIndex">
                            <a
                              v-if="part.kind === 'link'"
                              :href="part.href"
                              target="_blank"
                              rel="noopener noreferrer"
                            >{{ part.value }}</a>
                            <strong v-else-if="part.kind === 'strong'">{{ part.value }}</strong>
                            <code v-else-if="part.kind === 'code'">{{ part.value }}</code>
                            <template v-else>{{ part.value }}</template>
                          </template>
                        </li>
                      </ul>
                    </template>
                  </div>
                </details>
              </section>
            </section>
            <section
              v-if="selectedScript?.scriptId === 'mistbell'"
              class="scenario-resource-section"
            >
              <h4>{{ t("settings.resources") }}</h4>
              <table class="scenario-resource-table">
                <tbody>
                  <tr>
                    <td>{{ t("settings.stageArt") }}</td>
                    <td class="text-muted">{{ t("settings.resourcePlaceholder") }}</td>
                  </tr>
                  <tr>
                    <td>{{ t("settings.scriptSkin") }}</td>
                    <td class="mono">
                      {{ t("settings.skinFile") }}<template
                        v-if="theme.skinTokenCount.value !== null"
                      >
                        · {{ theme.skinTokenCount.value }} {{ t("settings.tokens") }}
                      </template>
                    </td>
                  </tr>
                </tbody>
              </table>
            </section>
          </section>

          <section v-show="activeSection === 'shortcuts'" id="shortcuts" class="settings-section">
            <SettingsSectionHeading
              :title="t('settings.sections.shortcuts')"
              @close="emit('close')"
            />
            <KeyboardShortcuts />
          </section>
        </div>
      </div>
      <p v-if="errorMessage" class="settings-error" role="alert">{{ errorMessage }}</p>
    </section>
  </div>
</template>
