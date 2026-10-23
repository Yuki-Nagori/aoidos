<script setup lang="ts">
import { computed, onMounted, onScopeDispose, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { PriceVersion } from "../../../api/billing";
import type { Locale } from "../../../api/locale";
import { formatExactCurrency } from "../../../shared/utils/format";
import { nextPriceTransition, priceTierAt, priceWeek, resolvePriceTimeZone, isDoublePrice } from "../utils/price-schedule";

const props = defineProps<{ price?: PriceVersion | null; locale: Locale; timeZone: string; providerLabel: string; preview?: boolean }>();
const { t } = useI18n();
const now = ref(new Date());
const showUtc = ref(false);
let timer: ReturnType<typeof setInterval> | undefined;
onMounted(() => { timer = setInterval(() => { now.value = new Date(); }, 60_000); });
onScopeDispose(() => clearInterval(timer));
const zone = computed(() => showUtc.value ? "UTC" : resolvePriceTimeZone(props.timeZone));
const version = computed(() => props.price?.schedule?.version ?? "");
const tier = computed(() => priceTierAt(version.value, now.value));
const week = computed(() => priceWeek(version.value, zone.value, now.value));
const ribbon = computed(() => priceWeek("opencode-go-deepseek-utc-weekday-v1", zone.value, now.value)[1]!.hours);
const doublePrices = computed(() => props.price?.schedule && rows.value.every((row) => isDoublePrice(props.price![row.key], props.price!.schedule!.peak[row.key])));
const next = computed(() => nextPriceTransition(version.value, now.value));
const nextMinutes = computed(() => next.value ? Math.ceil((next.value.getTime() - now.value.getTime()) / 60_000) : 0);
const weekdays = computed(() => [t("rates.wd.mon"), t("rates.wd.tue"), t("rates.wd.wed"), t("rates.wd.thu"), t("rates.wd.fri"), t("rates.wd.sat"), t("rates.wd.sun")]);
const rows = computed(() => [
  { key: "inputCached", label: t("rates.rowCached") },
  { key: "inputUncached", label: t("rates.rowUncached") },
  { key: "inputCachedWrite", label: t("rates.rowWrite") },
  { key: "output", label: t("rates.rowOutput") },
] as const);
function clock(date: Date): string {
  return new Intl.DateTimeFormat(props.locale, { timeZone: zone.value, hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(date);
}
const currentLabel = computed(() => new Intl.DateTimeFormat(props.locale, { timeZone: zone.value, weekday: "short", hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(now.value));
const nowPosition = computed(() => {
  const parts = new Intl.DateTimeFormat("en", { timeZone: zone.value, hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).formatToParts(now.value);
  const part = (type: Intl.DateTimeFormatPartTypes) => Number(parts.find((p) => p.type === type)?.value ?? 0);
  return (part("hour") * 60 + part("minute")) / 1440 * 100;
});
function amount(value: string | null | undefined): string {
  return value == null || !props.price ? "—" : formatExactCurrency(value, props.price.currency, props.locale);
}
const source = computed(() => {
  try {
    const url = new URL(props.price?.sourceUrl ?? "");
    return ["https:", "http:"].includes(url.protocol) ? { href: url.href, label: url.hostname } : undefined;
  } catch { return undefined; }
});
</script>

<template>
  <div v-if="price" class="billing-rates">
    <div v-if="!preview" class="billing-rates-tags">
      <span class="tag">{{ providerLabel }}</span><span class="tag">{{ price.modelId }}</span>
      <span class="tag accent">{{
        t(price.sourceKind === "official-provider-price" ? "rates.official" : "billing.priceReady")
      }}</span>
    </div>
    <template v-if="tier && !preview">
      <div class="billing-rates-nowcard" :class="{ 'is-peak': tier === 'peak' }">
        <div class="billing-rates-now-icon">
          <svg class="i lg" viewBox="0 0 24 24" aria-hidden="true">
            <template
              v-if="tier === 'off'"
            ><path d="M19 14.5A7.5 7.5 0 0 1 9.5 5a7.5 7.5 0 1 0 9.5 9.5z" /></template>
            <template v-else><circle
                cx="12"
                cy="12"
                r="3.5"
              /><path d="M12 3v2M12 19v2M3 12h2M19 12h2M5.6 5.6l1.4 1.4M17 17l1.4 1.4M5.6 18.4 7 17M17 7l1.4-1.4" /></template>
          </svg>
        </div>
        <div class="billing-rates-now-copy">
          <span class="text-sm text-muted">{{
            t("rates.nowLabel", { time: currentLabel, timeZone: zone })
          }}</span>
          <strong>{{ t(tier === 'peak' ? 'rates.nowPeak' : 'rates.nowOff') }}</strong>
          <span v-if="next" class="text-sm text-muted">{{
            t(tier === 'off' ? 'rates.nextPeak' : 'rates.nextOff', { time: clock(next), hours: Math.floor(nextMinutes / 60), minutes: nextMinutes % 60 })
          }}</span>
        </div>
        <div v-if="doublePrices" class="billing-rates-multiplier">
          <strong class="mono">{{ tier === 'peak' ? '×2' : '×1' }}</strong><span
            class="text-sm text-muted"
          >{{ t("rates.nowMultHint") }}</span>
        </div><span v-else class="billing-rates-tier" :class="{ 'is-peak': tier === 'peak' }">{{
          t(tier === 'peak' ? 'rates.peak' : 'rates.offPeak')
        }}</span>
      </div>
      <section class="billing-rates-section">
        <div class="billing-rates-heading">
          <h3>{{ t("rates.weekTitle") }}</h3>
          <div class="billing-rates-segment">
            <button type="button" :aria-pressed="!showUtc" @click="showUtc = false">
              {{ resolvePriceTimeZone(timeZone) }}
            </button>
            <button
              v-if="resolvePriceTimeZone(timeZone) !== 'UTC'"
              type="button"
              :aria-pressed="showUtc"
              @click="showUtc = true"
            >
              UTC
            </button>
          </div>
        </div>
        <div class="billing-rates-week" :aria-label="t('rates.weekTitle')">
          <div class="billing-rates-day-labels">
            <span v-for="day in week" :key="day.date" :class="{ today: day.today }">{{
              weekdays[day.weekday]
            }}</span>
          </div>
          <div class="billing-rates-week-grid">
            <div
              v-for="day in week"
              :key="day.date"
              class="billing-rates-week-row"
              :class="{ today: day.today }"
            >
              <i
                v-for="(hour, index) in day.hours"
                :key="index"
                :class="{ peak: hour.peak === 1, partial: hour.peak > 0 && hour.peak < 1, past: hour.past }"
                :style="{ '--peak-start': `${hour.peakStart}%`, '--peak-end': `${hour.peakEnd}%` }"
                :title="`${day.date} ${String(index).padStart(2, '0')}:00 ${zone}`"
              />
            </div>
            <span class="billing-rates-now-line" :style="{ left: `${nowPosition}%` }"><b
                class="mono"
              >{{ clock(now) }}</b></span>
          </div>
          <span /><div class="billing-rates-axis mono">
            <span v-for="hour in ['00', '06', '12', '18', '24']" :key="hour">{{ hour }}</span>
          </div>
        </div>
        <div class="billing-rates-legend text-sm text-muted">
          <span><i class="billing-rates-swatch peak" />{{ t("rates.peak") }}</span>
          <span><i class="billing-rates-swatch" />{{ t("rates.offPeak") }}</span>
        </div>
        <p class="note">
          {{ t("rates.tzNote", { timeZone: zone }) }}
        </p>
      </section>
    </template>
    <section class="billing-rates-section">
      <div class="billing-rates-heading">
        <h3>{{ t("rates.tableTitle") }}</h3><span class="tag mono">{{ price.currency }}</span>
      </div>
      <table class="billing-rates-table">
        <thead>
          <tr>
            <th>
              {{
                price.unitTokens === 1000000 ? t("rates.perMillion") : `${t("billing.unitTokens")}: ${price.unitTokens.toLocaleString(locale)}`
              }}
            </th>
            <th>
              <span class="billing-rates-tier">{{
                t(price.schedule ? 'rates.offPeak' : 'settings.priceRegularPeriod')
              }}</span>
            </th>
            <th v-if="price.schedule">
              <span class="billing-rates-tier is-peak">{{ t("rates.peak") }}</span>
            </th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="row in rows" :key="row.key">
            <td>{{ row.label }}</td>
            <td class="mono">{{ amount(price[row.key]) }}</td>
            <td v-if="price.schedule" class="mono">
              {{ amount(price.schedule.peak[row.key]) }} <span
                v-if="isDoublePrice(price[row.key], price.schedule.peak[row.key])"
                class="billing-rates-x2"
              >×2</span>
            </td>
          </tr>
        </tbody>
      </table>
      <div v-if="preview && tier" class="billing-rates-ribbon">
        <p class="text-sm text-muted">{{ t("tpl.weekday", { timeZone: zone }) }}</p>
        <div class="billing-rates-week-row">
          <i
            v-for="(hour, index) in ribbon"
            :key="index"
            :class="{ peak: hour.peak === 1, partial: hour.peak > 0 && hour.peak < 1 }"
            :style="{ '--peak-start': `${hour.peakStart}%`, '--peak-end': `${hour.peakEnd}%` }"
          />
        </div>
        <div class="billing-rates-axis mono">
          <span v-for="hour in ['00', '06', '12', '18', '24']" :key="hour">{{ hour }}</span>
        </div>
      </div>
      <dl class="billing-rates-metadata">
        <dt>{{ t("rates.version") }}</dt>
        <dd class="mono">{{ price.versionId }}</dd>
        <dt>{{ t("rates.checkedAt") }}</dt>
        <dd class="mono">{{ price.checkedAt.slice(0, 10) }}</dd>
        <template v-if="source"><dt>{{ t("rates.source") }}</dt><dd>
            <a :href="source.href" target="_blank" rel="noreferrer">{{ source.label }} ↗</a>
          </dd></template>
      </dl>
    </section>
    <section v-if="!preview" class="billing-rates-section">
      <h3>{{ t("rates.splitTitle") }}</h3>
      <p class="note">{{ t("rates.splitUnavailable") }}</p>
    </section>
    <section
      v-if="!preview && version === 'deepseek-api-utc-weekday-holiday-2026-v1'"
      class="billing-rates-section"
    >
      <h3>{{ t("rates.holidayTitle") }}</h3>
      <div class="card billing-rates-holiday">
        <div>
          <span class="mono">{{ version }}</span><p class="text-sm text-muted">
            {{ t("rates.holidayCover") }}
          </p>
        </div><span class="tag accent">2026</span>
      </div>
      <div class="billing-rates-calendar-note">
        <strong>{{ t("rates.uncoveredTitle") }}</strong><p class="text-sm text-muted">
          {{ t("billing.deepseekHolidayNote") }}
        </p>
      </div>
      <p class="note">{{ t("rates.dispatchNote") }}</p>
    </section>
  </div>
  <p v-else class="note" role="status">{{ t("billing.priceMissing") }}</p>
</template>
