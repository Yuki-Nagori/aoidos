<script setup lang="ts">
import { computed } from "vue";
import BillingPanel from "../../features/billing/components/BillingPanel.vue";
import { useI18n } from "vue-i18n";
import type { useProduct } from "../../features/product/composables/useProduct";
import type { useTheme } from "../../features/theme/composables/useTheme";
import type { useLocale } from "../../features/locale/composables/useLocale";
import type { useBilling } from "../../features/billing/composables/useBilling";

const { t } = useI18n();
const { product, theme, locale, billing, errorMessage } = defineProps<{
  product: ReturnType<typeof useProduct>;
  theme: ReturnType<typeof useTheme>;
  locale: ReturnType<typeof useLocale>;
  billing: ReturnType<typeof useBilling>;
  errorMessage?: string;
}>();
const emit = defineEmits<{ reconnect: [] }>();
const selectedScript = computed(() =>
  product.scripts.value.find((script) => script.scriptId === product.scriptId.value),
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
  <details class="glass-panel" :open="!product.session.value">
    <summary class="setup-summary">
      <h1 class="text-2xl font-semibold">{{ t("common.appName") }}</h1>
      <p class="text-sm text-muted">{{ t("game.intro") }}</p>
    </summary>
    <div class="flex gap-3">
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
      <button
        type="button"
        v-if="product.scriptId.value === 'mistbell'"
        @click="theme.toggleSkin"
      >
        {{ theme.disabled.value ? t("settings.enableSkin") : t("settings.disableSkin") }}
      </button>
    </div>
    <p v-if="themeErrorMessage" role="status">{{ themeErrorMessage }}</p>
    <p v-if="skinErrorMessage" role="status">{{ skinErrorMessage }}</p>
    <p v-else-if="theme.warningCount.value">
      {{ t("settings.skinWarnings", { count: theme.warningCount.value }) }}
    </p>
    <label>
      {{ t("settings.language") }}
      <select
        :value="locale.choice.value"
        :aria-label="t('settings.language')"
        :disabled="!locale.ready.value || locale.saving.value"
        @change="locale.setChoice(
          ($event.target as HTMLSelectElement).value as 'system' | 'zh-Hans' | 'en',
        )"
      >
        <option value="system">
          {{
            t("settings.systemLanguageCurrent", {
              locale: t(
                locale.resolved.value === "zh-Hans" ? "settings.chinese" : "settings.english",
              ),
            })
          }}
        </option>
        <option value="zh-Hans">{{ t("settings.chinese") }}</option>
        <option value="en">{{ t("settings.english") }}</option>
      </select>
    </label>
    <p v-if="localeNotice" role="status">{{ localeNotice }}</p>
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
      </select></label>
    <button type="button" :disabled="product.busy.value" @click="product.configureKey">
      {{ t("settings.setApiKey") }}
    </button>
    <p v-if="product.keyStatus.value">
      {{ product.keyStatus.value.set ? t("settings.keySet") : t("settings.keyMissing") }}
    </p>
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
      </select></label>
    <slot name="memory-settings" />
    <div class="flex gap-4">
      <button type="button" :disabled="product.busy.value" @click="product.open(false)">
        {{ t("game.restart") }}
      </button>
      <button type="button" :disabled="product.busy.value" @click="product.open(true)">
        {{ t("game.newCycle") }}
      </button>
      <button type="button" @click="emit('reconnect')">{{ t("common.reconnect") }}</button>
    </div>
    <BillingPanel
      :billing="billing"
      :locale="locale.resolved.value"
      :active="Boolean(product.session.value)"
      :provider-id="product.providerId.value"
    />
    <details v-if="selectedScript">
      <summary>{{ t("settings.credits") }}</summary>
      <pre class="whitespace-pre-wrap text-sm">{{ selectedScript.attributions }}</pre>
    </details>
    <p v-if="errorMessage" role="alert">{{ errorMessage }}</p>
  </details>
</template>
