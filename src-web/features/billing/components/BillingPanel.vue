<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { Locale } from "../../../api/locale";
import { formatDate, formatExactCurrency } from "../../../shared/utils/format";
import type { useBilling } from "../composables/useBilling";
import { summarizeRunUsage } from "../utils/run-usage";
import { billingErrorDescriptor } from "../utils/billing-error";
import { formatPeakSchedule } from "../utils/price-schedule";
import SelectMenu from "../../../shared/components/SelectMenu.vue";

const { t } = useI18n();
const props = defineProps<{
  billing: ReturnType<typeof useBilling>;
  locale: Locale;
  active: boolean;
  providerId: string;
  providerLabel?: string;
  modelId?: string;
  view?: "all" | "billing" | "price" | "balance";
}>();
const billing = computed(() => props.billing);
const locale = computed(() => props.locale);
const active = computed(() => props.active);
const providerId = computed(() => props.providerId);
const modelId = computed(() => props.modelId);
const view = computed(() => props.view ?? "all");
const runTotals = computed(() =>
  summarizeRunUsage(props.billing.period.value?.items ?? [], {
    CNY: props.billing.settings.value?.cnyLimit ?? null,
    USD: props.billing.settings.value?.usdLimit ?? null,
  }),
);
function billingAmount(amount: string | null, currency: "CNY" | "USD"): string {
  return amount === null
    ? t("billing.unknown")
    : formatExactCurrency(amount, currency, props.locale);
}
function providerBalanceStatus(status: "available" | "unavailable" | "unknown"): string {
  const label = {
    available: t("billing.balanceAvailable"),
    unavailable: t("billing.balanceUnavailable"),
    unknown: t("billing.balanceUnknown"),
  };
  return label[status];
}
function providerBalanceDate(queriedAt: string): string {
  return formatDate(queriedAt, props.locale, props.billing.monthly.value?.timeZone ?? "UTC")
    .text;
}
function providerBalanceAmount(amount: string, currency: string): string {
  if (currency !== "CNY" && currency !== "USD") return t("billing.unknown");
  return formatExactCurrency(amount, currency, props.locale);
}
const billingErrorMessage = computed(() => {
  const failure = props.billing.error.value;
  if (!failure) return undefined;
  const descriptor = billingErrorDescriptor(failure);
  return "values" in descriptor ? t(descriptor.key, descriptor.values) : t(descriptor.key);
});
</script>
<template>
  <section v-if="active" class="billing-panel grid gap-4">
    <h4 v-if="view === 'all' || view === 'billing'" class="font-semibold">
      {{ t("billing.title") }}
    </h4>
    <template v-if="view === 'all' || view === 'billing'">
      <p class="text-sm text-muted">{{ t("billing.explanation") }}</p>
      <p class="text-sm text-muted">{{ t("billing.referenceHint") }}</p>
      <div v-if="runTotals.length" class="billing-summary-grid" :aria-label="t('billing.usage')">
        <article v-for="total in runTotals" :key="total.currency" class="billing-summary-card">
          <span class="billing-summary-label">
            {{ t(total.currency === "CNY" ? "billing.currencyCny" : "billing.currencyUsd") }}
          </span>
          <strong class="billing-summary-amount mono">
            {{ billingAmount(total.amount, total.currency) }}
          </strong>
          <span v-if="total.limit" class="billing-summary-limit mono">
            / {{ billingAmount(total.limit, total.currency) }}
          </span>
        </article>
      </div>
    </template>
    <section
      v-if="(view === 'all' || view === 'balance') && providerId === 'deepseek'"
      class="billing-balance-section grid gap-3"
    >
      <button
        type="button"
        class="btn"
        :disabled="billing.balanceBusy.value"
        @click="billing.queryProviderBalance"
      >
        {{ t("billing.queryBalance") }}
      </button>
      <p class="text-sm text-muted">{{ t("billing.balanceCacheHint") }}</p>
      <article v-if="billing.providerBalance.value" class="billing-balance-card">
        <header class="billing-balance-header">
          <strong>{{ providerLabel ?? providerId }}</strong>
          <span role="status" class="billing-price-status">
            {{
              t("billing.balanceStatus", {
                status: providerBalanceStatus(billing.providerBalance.value.status),
              })
            }}
          </span>
        </header>
        <p class="billing-balance-date">
          {{
            t("billing.balanceQueriedAt", {
              queriedAt: providerBalanceDate(billing.providerBalance.value.queriedAt),
            })
          }}
        </p>
        <p v-if="billing.providerBalance.value.stale" role="status" class="text-sm text-muted">
          {{ t("billing.balanceStale") }}
        </p>
        <div v-if="billing.providerBalance.value.accounts.length" class="billing-balance-accounts">
          <section
            v-for="account in billing.providerBalance.value.accounts"
            :key="account.currency"
            class="billing-balance-account"
          >
            <strong class="mono">{{ account.currency }}</strong>
            <p>
              {{
                t("billing.balanceAccount", {
                  currency: account.currency,
                  total: providerBalanceAmount(account.total, account.currency),
                  granted: providerBalanceAmount(account.granted, account.currency),
                  toppedUp: providerBalanceAmount(account.toppedUp, account.currency),
                })
              }}
            </p>
          </section>
        </div>
      </article>
    </section>
    <p v-else-if="view === 'balance'" class="text-sm text-muted">
      {{ t("billing.balanceUnsupported") }}
    </p>
    <template v-if="view === 'all' || view === 'billing'">
      <form class="billing-form grid gap-3" @submit.prevent="billing.saveSettings">
        <label>
          {{ t("billing.cnyLimit") }}
          <input v-model="billing.cnyLimit.value" inputmode="decimal" autocomplete="off" />
        </label>
        <label>
          {{ t("billing.usdLimit") }}
          <input v-model="billing.usdLimit.value" inputmode="decimal" autocomplete="off" />
        </label>
        <button type="submit" class="btn primary" :disabled="billing.busy.value">
          {{ billing.settings.value ? t("billing.saveLimits") : t("billing.setLimits") }}
        </button>
      </form>
      <p v-if="billing.settings.value" class="text-sm text-muted">
        {{ t("billing.revision", { revision: billing.settings.value.revision }) }}
      </p>
      <section class="grid gap-2" :aria-label="t('billing.usage')">
        <h2 class="font-semibold">{{ t("billing.usage") }}</h2>
        <p v-if="!billing.period.value?.items.length">{{ t("billing.noUsage") }}</p>
        <article
          v-for="item in billing.period.value?.items"
          :key="`${item.providerId}:${item.currency}:${item.modelId}`"
          class="rounded border border-hairline p-3"
        >
          <h3>
            {{
              t("billing.itemTitle", {
                provider: item.providerId,
                model: item.modelId,
                currency: item.currency,
              })
            }}
          </h3>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.settled"),
                value: billingAmount(item.settledAmount, item.currency),
              })
            }}
          </p>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.reserved"),
                value: billingAmount(item.reservedAmount, item.currency),
              })
            }}
          </p>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.tokens"),
                value: item.knownTokens ?? t("billing.unknown"),
              })
            }}
          </p>
          <p v-if="item.incompleteRequests">
            {{ t("billing.incomplete", { count: item.incompleteRequests }) }}
          </p>
        </article>
        <p
          v-for="warning in billing.period.value?.warnings"
          :key="warning.currency"
          role="status"
        >
          {{
            t("billing.warning", {
              currency: warning.currency,
              percent: warning.thresholdPercent,
            })
          }}
        </p>
        <button
          type="button"
          class="btn ghost"
          :disabled="billing.busy.value"
          @click="billing.loadPeriod"
        >
          {{ t("billing.refreshUsage") }}
        </button>
        <hr class="my-2 border-hairline" />
        <h2 class="font-semibold">{{ t("billing.requests") }}</h2>
        <article
          v-for="item in billing.requests.value?.items"
          :key="item.requestId"
          class="rounded border border-hairline p-3"
        >
          <h3>
            {{
              t("billing.itemTitle", {
                provider: item.providerId,
                model: item.modelId,
                currency: item.currency,
              })
            }}
          </h3>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.requestDate"),
                value: item.dispatchAt,
              })
            }}
          </p>
          <p>
            {{ t("billing.labeledValue", { label: t("billing.requestState"), value: item.state }) }}
          </p>
          <p v-if="item.settlementPending || item.diagnosticCode" role="status">
            {{
              t(
                item.diagnosticCode === "budget.conflict"
                  ? "billing.settlementConflict"
                  : "billing.settlementPending",
              )
            }}
            <span v-if="item.diagnosticCode">{{ item.diagnosticCode }}</span>
          </p>
          <button
            v-if="item.settlementPending"
            type="button"
            :disabled="billing.retryingRequestId.value !== undefined"
            @click="billing.retrySettlement(item.requestId)"
          >
            {{
              billing.retryingRequestId.value === item.requestId
              ? t("billing.settlementRetrying")
              : t("billing.retrySettlement")
            }}
          </button>
          <button
            v-if="item.state === 'unconfirmed' &&
            !item.actualAmount &&
            !item.settlementPending &&
            item.inputTotal !== null &&
            item.inputCached !== null &&
            item.inputCachedWrite !== null &&
            item.outputTotal !== null"
            type="button"
            :disabled="billing.backfillingRequestId.value !== undefined"
            @click="billing.previewBackfill(item.requestId)"
          >
            {{ t("billing.previewBackfill") }}
          </button>
          <!-- biome-ignore lint/a11y/useSemanticElements: This groups preview status and controls, not form fields. -->
          <div
            v-if="billing.backfillPreview.value?.requestId === item.requestId"
            class="grid gap-2 rounded border border-hairline p-3"
            role="group"
          >
            <p>
              {{
                t("billing.backfillPreview", {
                  priceVersion: billing.backfillPreview.value.priceVersionId,
                  amount: billingAmount(
                    billing.backfillPreview.value.estimatedAmount,
                    billing.backfillPreview.value.currency,
                  ),
                  currency: billing.backfillPreview.value.currency,
                })
              }}
            </p>
            <p class="text-sm text-muted">{{ t("billing.backfillUnavailable") }}</p>
            <button
              type="button"
              :disabled="billing.backfillingRequestId.value !== undefined"
              @click="billing.commitBackfill"
            >
              {{ t("billing.commitBackfill") }}
            </button>
          </div>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.requestPrice"),
                value: item.priceVersionId,
              })
            }}
          </p>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.reserved"),
                value: billingAmount(item.reservedAmount, item.currency),
              })
            }}
          </p>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.requestCost"),
                value: billingAmount(item.actualAmount, item.currency),
              })
            }}
          </p>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.requestInput"),
                value: item.inputTotal ?? t("billing.unknown"),
              })
            }}
          </p>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.requestCached"),
                value: item.inputCached ?? t("billing.unknown"),
              })
            }}
          </p>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.requestCacheWrite"),
                value: item.inputCachedWrite ?? t("billing.unknown"),
              })
            }}
          </p>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.requestOutput"),
                value: item.outputTotal ?? t("billing.unknown"),
              })
            }}
          </p>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.requestReasoning"),
                value: item.reasoningIncluded ?? t("billing.unknown"),
              })
            }}
          </p>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.tokens"),
                value: item.totalTokens ?? t("billing.unknown"),
              })
            }}
          </p>
        </article>
        <div class="flex gap-3">
          <button type="button" @click="billing.loadRequests(true)">
            {{ t("billing.restartRequests") }}
          </button>
          <button
            v-if="billing.requests.value?.nextCursor"
            type="button"
            @click="billing.loadRequests()"
          >
            {{ t("billing.loadMore") }}
          </button>
        </div>
        <hr class="my-2 border-hairline" />
        <h2 class="font-semibold">
          {{ t("billing.monthly", { month: billing.monthly.value?.month ?? "—" }) }}
        </h2>
        <p v-if="billing.monthly.value">
          {{ t("billing.timeZone", { timeZone: billing.monthly.value.timeZone }) }}
        </p>
        <p v-if="!billing.monthly.value?.items.length">{{ t("billing.noUsage") }}</p>
        <article
          v-for="item in billing.monthly.value?.items"
          :key="`month:${item.providerId}:${item.currency}:${item.modelId}`"
          class="rounded border border-hairline p-3"
        >
          <h3>
            {{
              t("billing.itemTitle", {
                provider: item.providerId,
                model: item.modelId,
                currency: item.currency,
              })
            }}
          </h3>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.settled"),
                value: billingAmount(item.settledAmount, item.currency),
              })
            }}
          </p>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.reserved"),
                value: billingAmount(item.reservedAmount, item.currency),
              })
            }}
          </p>
          <p>
            {{
              t("billing.labeledValue", {
                label: t("billing.tokens"),
                value: item.knownTokens ?? t("billing.unknown"),
              })
            }}
          </p>
          <p v-if="item.incompleteRequests">
            {{ t("billing.incomplete", { count: item.incompleteRequests }) }}
          </p>
        </article>
        <button
          type="button"
          class="btn ghost"
          :disabled="billing.busy.value"
          @click="billing.loadMonthly"
        >
          {{ t("billing.refreshMonthly") }}
        </button>
      </section>
    </template>
    <template v-if="view === 'all' || view === 'price'">
      <div class="billing-price-context">
        <div class="billing-price-identity">
          <h4>{{ t("billing.savePrice") }}</h4>
          <p class="mono">
            {{
              t("billing.itemTitle", {
                provider: props.providerLabel ?? providerId,
                model: modelId ?? "—",
                currency: billing.priceCurrency.value,
              })
            }}
          </p>
          <p class="billing-price-note">{{ t("billing.priceImmutableNote") }}</p>
        </div>
        <span role="status" class="billing-price-status">
          {{ billing.priceReady.value ? t("billing.priceReady") : t("billing.priceMissing") }}
        </span>
      </div>
      <form class="billing-price-form grid gap-3" @submit.prevent="billing.savePrice">
        <div class="billing-price-base-grid">
          <div class="billing-price-field">
            <span>{{ t("billing.currency") }}</span>
            <SelectMenu
              :model-value="billing.priceCurrency.value"
              :options="[
                { value: 'CNY', label: t('billing.currencyCny') },
                { value: 'USD', label: t('billing.currencyUsd') },
              ]"
              :label="t('billing.currency')"
              @update:model-value="billing.priceCurrency.value = $event"
            />
          </div>
          <label class="billing-price-field">
            <span>{{ t("billing.unitTokens") }}</span>
            <input v-model="billing.unitTokens.value" inputmode="numeric" autocomplete="off" />
          </label>
        </div>
        <div class="billing-price-rates-grid">
          <label class="billing-price-field">
            <span>{{ t("billing.inputUncached") }}</span>
            <input v-model="billing.inputUncached.value" inputmode="decimal" autocomplete="off" />
          </label>
          <label class="billing-price-field">
            <span>{{ t("billing.inputCached") }}</span>
            <input v-model="billing.inputCached.value" inputmode="decimal" autocomplete="off" />
          </label>
          <label class="billing-price-field">
            <span>{{ t("billing.inputCachedWrite") }}</span>
            <input
              v-model="billing.inputCachedWrite.value"
              inputmode="decimal"
              autocomplete="off"
            />
          </label>
        </div>
        <div class="billing-price-rates-grid">
          <label class="billing-price-field">
            <span>{{ t("billing.output") }}</span>
            <input v-model="billing.output.value" inputmode="decimal" autocomplete="off" />
          </label>
        </div>
        <fieldset
          v-if="billing.requiresScheduledPrice.value"
          class="billing-scheduled-prices"
        >
          <legend>{{ t("billing.goPeakPrices") }}</legend>
          <p class="note">
            {{
              t("billing.goPeakSchedule", {
                timeZone: billing.monthly.value?.timeZone ?? "UTC",
                windows: formatPeakSchedule(
                  billing.monthly.value?.timeZone ?? "UTC",
                  locale,
                ),
                provider: t(
                  billing.isDeepseekApiSchedule.value
                    ? "billing.deepseekApi"
                    : "billing.openCodeGo",
                ),
                holidayNote: billing.isDeepseekApiSchedule.value
                  ? t("billing.deepseekHolidayNote")
                  : "",
              })
            }}
          </p>
          <div class="billing-price-rates-grid">
            <label class="billing-price-field">
              <span>{{ t("billing.inputUncached") }}</span>
              <input
                v-model="billing.peakInputUncached.value"
                inputmode="decimal"
                autocomplete="off"
              />
            </label>
            <label class="billing-price-field">
              <span>{{ t("billing.inputCached") }}</span>
              <input
                v-model="billing.peakInputCached.value"
                inputmode="decimal"
                autocomplete="off"
              />
            </label>
            <label class="billing-price-field">
              <span>{{ t("billing.inputCachedWrite") }}</span>
              <input
                v-model="billing.peakInputCachedWrite.value"
                inputmode="decimal"
                autocomplete="off"
              />
            </label>
            <label class="billing-price-field">
              <span>{{ t("billing.output") }}</span>
              <input v-model="billing.peakOutput.value" inputmode="decimal" autocomplete="off" />
            </label>
          </div>
        </fieldset>
        <label class="billing-price-field">
          <span>{{ t("billing.sourceUrl") }}</span>
          <input v-model="billing.sourceUrl.value" type="url" autocomplete="off" />
        </label>
        <button
          type="submit"
          class="btn primary"
          :disabled="billing.busy.value ||
          !billing.inputCachedWrite.value.trim() ||
          !billing.scheduledPriceComplete.value"
        >
          {{ t("billing.savePrice") }}
        </button>
      </form>
    </template>
    <p v-if="billingErrorMessage" role="alert">{{ billingErrorMessage }}</p>
  </section>
</template>
