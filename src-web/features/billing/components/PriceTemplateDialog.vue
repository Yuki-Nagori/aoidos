<script setup lang="ts">
import { computed, nextTick, onMounted, onScopeDispose, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { BillingCurrency, PriceVersion } from "../../../api/billing";
import type { Locale } from "../../../api/locale";
import BillingRates from "./BillingRates.vue";
const props = defineProps<{ prices: PriceVersion[]; currency: BillingCurrency; registeredCurrency?: BillingCurrency; locale: Locale; busy: boolean; error?: string }>();
const emit = defineEmits<{ close: []; manual: []; confirm: []; currency: [BillingCurrency] }>();
const { t } = useI18n();
const panel = ref<HTMLElement>();
const cancelButton = ref<HTMLButtonElement>();
const previousFocus = document.activeElement as HTMLElement | null;
const selected = computed(() => props.prices.find((price) => price.currency === props.currency));
const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
onMounted(async () => { await nextTick(); cancelButton.value?.focus(); });
onScopeDispose(() => previousFocus?.focus());
function onKeydown(event: KeyboardEvent): void {
  event.stopPropagation();
  if (event.key === "Escape" && !event.isComposing && !props.busy) { event.preventDefault(); emit("close"); }
  if (event.key !== "Tab") return;
  const elements = panel.value?.querySelectorAll<HTMLElement>('button:not(:disabled), a[href]');
  const first = elements?.[0]; const last = elements?.[elements.length - 1];
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
}
</script>
<template>
  <div class="billing-template-overlay">
    <section
      ref="panel"
      class="billing-template-dialog glass"
      role="dialog"
      aria-modal="true"
      aria-labelledby="billing-template-title"
      @keydown="onKeydown"
    >
      <h2 id="billing-template-title">
        <svg class="i lg" viewBox="0 0 24 24" aria-hidden="true">
          <ellipse cx="12" cy="7" rx="7" ry="3" />
          <path d="M5 7v5c0 1.7 3.1 3 7 3s7-1.3 7-3V7M5 12v5c0 1.7 3.1 3 7 3s7-1.3 7-3v-5" />
        </svg>{{ t("tpl.title") }}
      </h2>
      <p>{{ t("tpl.desc", { model: selected?.modelId ?? '—' }) }}</p>
      <div class="billing-rates-heading">
        <span class="text-sm text-muted">{{
          t("tpl.currency")
        }}</span><div class="billing-rates-segment">
          <button
            v-for="value in (['CNY', 'USD'] as const)"
            :key="value"
            type="button"
            :aria-pressed="currency === value"
            :disabled="busy"
            @click="emit('currency', value)"
          >
            {{ value }}
          </button>
        </div>
      </div>
      <p class="text-sm text-muted">
        {{
          registeredCurrency ? t("tpl.currencyPrevious", { currency: registeredCurrency }) : t("tpl.currencyDefault")
        }}
      </p>
      <BillingRates
        v-if="selected"
        :price="selected"
        :locale="locale"
        :time-zone="timeZone"
        provider-label="DeepSeek API"
        preview
      />
      <p v-else role="status" class="note">{{ t("settings.priceTemplateUnavailable") }}</p>
      <p class="note">{{ t("tpl.note") }}</p>
      <p v-if="error" role="alert">{{ error }}</p>
      <div class="billing-template-actions">
        <button type="button" class="btn ghost" :disabled="busy" @click="emit('manual')">
          {{ t("tpl.manual") }}
        </button><span /><button
          ref="cancelButton"
          type="button"
          class="btn"
          :disabled="busy"
          @click="emit('close')"
        >
          {{ t("common.cancel") }}<kbd class="kbd dim">Esc</kbd>
        </button>
        <button
          type="button"
          class="btn primary"
          :disabled="busy || !selected"
          @click="emit('confirm')"
        >
          {{ t("tpl.confirm") }}
        </button>
      </div>
    </section>
  </div>
</template>
