<script setup lang="ts">
import { computed, ref, onMounted, onScopeDispose, watch } from "vue";
import { useI18n } from "vue-i18n";
import { diceCheckTotal } from "../utils/dice-visual";
import type { DiceRollFact, DiceSkinId } from "../utils/dice-visual";
import DiceResultView from "./DiceResultView.vue";

const props = defineProps<{
  expression: string;
  mode: "manual" | "auto";
  status: "waiting" | "rolling" | "settled";
  disabled: boolean;
  scriptId: string;
  skin: DiceSkinId;
  scriptSkinEnabled: boolean;
  shortcut: string;
  keyboardActive?: boolean;
  rolls: readonly DiceRollFact[];
  modifierTotal: number;
  ruleId?: string;
  actionLabel?: string;
  total?: number;
  result?: "success" | "costlySuccess" | "failure" | "criticalSuccess" | "criticalFailure";
}>();
const emit = defineEmits<{
  submit: [gesture: { direction: [number, number]; power: number }];
  presentationSettled: [];
}>();
const { t } = useI18n();
const motion = ref<"idle" | "rolling" | "result">("idle");
// 与原型 roll({values}).then(...) 一致：事实返回才开始投掷，视觉落定后才显示结果。
watch(() => [props.status, props.rolls] as const, ([status, rolls], previous) => {
  if (status === "waiting") motion.value = "idle";
  else if (rolls.length && (previous?.[0] !== "settled" || motion.value === "rolling")) motion.value = "rolling";
  else if (status === "settled") motion.value = "result";
}, { immediate: true });
const presentationStatus = computed(() => props.status === "settled" && motion.value !== "result" ? "rolling" : props.status);
function onDiceSettled(): void {
  motion.value = "result";
  emit("presentationSettled");
}
const interactive = computed(() => props.mode === "manual" && props.status === "waiting" && !props.disabled);
const diceTotal = computed(() => props.rolls.reduce((sum, roll) => sum + roll.value, 0));
const checkTotal = computed(() => diceCheckTotal(props.rolls, props.modifierTotal, props.total));
const isPbta = computed(() => props.ruleId === "pbta-explore-v1" || props.ruleId === "2d6-pbta-v1");
const ruleLabel = computed(() => isPbta.value ? t("game.checkRules.pbtaExplore") : t("game.checkTray.unknownRule"));
const modifierLabel = computed(() => `${props.modifierTotal >= 0 ? "+" : "−"}${Math.abs(props.modifierTotal)}`);
const pbtaOutcomes: Readonly<Record<number, readonly [number, number, number]>> = {
  [-3]: [30, 6, 0], [-2]: [26, 9, 1], [-1]: [21, 12, 3],
  0: [15, 15, 6], 1: [10, 16, 10], 2: [6, 15, 15], 3: [3, 12, 21],
};
function diceRange(min: number, max: number): string {
  const lower = Math.max(2, min), upper = Math.min(12, max);
  return lower === upper ? String(lower) : `${lower}–${upper}`;
}
const odds = computed(() => {
  const counts = isPbta.value ? pbtaOutcomes[props.modifierTotal] : undefined;
  if (!counts) return [];
  const mod = props.modifierTotal;
  return [
    { kind: "failure", count: counts[0], label: t("game.checkResults.failure"), range: diceRange(2, 6 - mod) },
    { kind: "costlySuccess", count: counts[1], label: t("game.checkResults.costlySuccess"), range: diceRange(7 - mod, 9 - mod) },
    { kind: "success", count: counts[2], label: t("game.checkResults.success"), range: diceRange(10 - mod, 12) },
  ].filter((outcome) => outcome.count > 0);
});
const oddsSummary = computed(() => odds.value.map((outcome) => `${outcome.label} ${Math.round(outcome.count / 36 * 100)}%`).join(" · "));
const resultLabel = computed(() => {
  switch (props.result) {
    case "success": return t("game.checkResults.success");
    case "costlySuccess": return t("game.checkResults.costlySuccess");
    case "failure": return t("game.checkResults.failure");
    case "criticalSuccess": return t("game.checkResults.criticalSuccess");
    case "criticalFailure": return t("game.checkResults.criticalFailure");
    default: return "";
  }
});

const lastGesture = ref<{ direction: [number, number]; power: number }>();
function submitGesture(gesture: { direction: [number, number]; power: number }): void {
  if (!interactive.value) return;
  lastGesture.value = gesture;
  emit("submit", gesture);
}
function submitDefault(): void {
  submitGesture({ direction: [-1, 0.3], power: 0.6 });
}

function submitAccessible(event: MouseEvent): void {
  if (event.detail === 0) submitDefault();
}

function onKeydown(event: KeyboardEvent): void {
  if (
    event.defaultPrevented ||
    event.key !== "Enter" ||
    event.repeat ||
    event.isComposing ||
    event.keyCode === 229 ||
    !interactive.value
  ) return;
  const isMac = /Mac|iPhone|iPad/.test(navigator.platform);
  if (!(isMac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey)) return;
  event.preventDefault();
  submitDefault();
}

onMounted(() => {
  if (props.keyboardActive) window.addEventListener("keydown", onKeydown);
});
watch(() => props.keyboardActive, (active) => {
  if (active) window.addEventListener("keydown", onKeydown);
  else window.removeEventListener("keydown", onKeydown);
}, { immediate: true });
onScopeDispose(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <section
    class="check-dice-card"
    :data-state="presentationStatus"
    :data-mode="props.mode"
    :aria-busy="presentationStatus === 'rolling'"
    :aria-label="t('game.checkTray.eyebrow')"
  >
    <span class="sr-only" role="status" aria-live="polite" aria-atomic="true">
      {{
        presentationStatus === "settled"
        ? t("game.checkTray.settledSummary", { expression: props.expression, total: checkTotal, result: resultLabel })
        : ""
      }}
    </span>
    <header class="check-dice-header">
      <span class="check-dice-label">{{ t("game.checkTray.eyebrow") }}</span>
      <strong class="check-dice-action">{{ props.actionLabel || ruleLabel }}</strong>
      <span v-if="props.mode === 'auto'" class="check-dice-auto">{{ t("game.automatic") }}</span>
      <span class="check-dice-formula">
        <svg class="i sm" viewBox="0 0 24 24" aria-hidden="true">
          <rect x="4" y="4" width="16" height="16" rx="3.5" />
          <circle cx="9" cy="9" r=".9" fill="currentColor" />
          <circle cx="15" cy="15" r=".9" fill="currentColor" />
          <circle cx="15" cy="9" r=".9" fill="currentColor" />
          <circle cx="9" cy="15" r=".9" fill="currentColor" />
        </svg>
        {{ props.expression }}
      </span>
    </header>
    <div class="check-dice-body">
      <button
        type="button"
        class="check-dice-tray"
        :class="{ 'check-dice-tray-disabled': !interactive }"
        :disabled="!interactive"
        :aria-label="presentationStatus === 'settled' ? t('game.checkTray.settledSummary', { expression: props.expression, total: checkTotal, result: resultLabel }) : t('game.checkTray.formula', { expression: props.expression })"
        @click="submitAccessible"
      >
        <DiceResultView
          class="check-dice-visual"
          :show-pop="true"
          :expression="props.expression"
          :rolls="props.rolls"
          :script-id="props.scriptId"
          :skin="props.skin"
          :script-skin-enabled="props.scriptSkinEnabled"
          :motion="motion"
          :interactive="interactive"
          :gesture="lastGesture"
          :effect="props.result === 'criticalSuccess' ? 'crit' : props.result === 'criticalFailure' ? 'fumble' : ''"
          @submit="submitGesture"
          @settled="onDiceSettled"
        />
      </button>
      <div class="check-dice-info">
        <dl class="check-dice-kv">
          <dt>{{ t("game.checkTray.rule") }}</dt>
          <dd>{{ ruleLabel }}</dd>
          <dt>{{ t("game.checkTray.modifier") }}</dt>
          <dd class="mono">{{ modifierLabel }}</dd>
          <template v-if="isPbta">
            <dt>{{ t("game.checkTray.target") }}</dt>
            <dd>{{ t("game.checkTray.pbtaTarget") }}</dd>
          </template>
        </dl>
        <div
          v-if="odds.length"
          class="check-dice-odds"
          role="img"
          :aria-label="`${t('game.checkTray.oddsLabel')}: ${oddsSummary}`"
        >
          <div class="check-dice-odds-bar" aria-hidden="true">
            <span
              v-for="outcome in odds"
              :key="outcome.kind"
              :class="`odds-${outcome.kind}`"
              :style="{ flex: outcome.count }"
            >
              {{ outcome.range }}
            </span>
          </div>
          <div class="check-dice-odds-legend">
            <span v-for="outcome in odds" :key="outcome.kind">
              <i :class="`odds-${outcome.kind}`" aria-hidden="true" />
              {{ outcome.label }} <span class="mono">{{
                  Math.round(outcome.count / 36 * 100)
                }}%</span>
            </span>
          </div>
        </div>
        <p v-if="isPbta" class="check-dice-critical-note">{{ t("game.checkTray.noCritical") }}</p>
      </div>
    </div>
    <footer class="check-dice-footer">
      <div
        v-if="props.mode === 'manual' && presentationStatus === 'waiting'"
        class="check-dice-ready"
      >
        <p class="check-dice-hint">{{ t("game.checkTray.ready") }}</p>
        <button
          type="button"
          class="check-dice-submit"
          :disabled="!interactive"
          @click="submitDefault"
        >
          <svg class="i sm" viewBox="0 0 24 24" aria-hidden="true">
            <rect x="4" y="4" width="16" height="16" rx="3.5" />
            <circle cx="9" cy="9" r=".9" fill="currentColor" />
            <circle cx="15" cy="15" r=".9" fill="currentColor" />
          </svg>
          <span>{{ t("game.checkTray.rollFormula", { expression: props.expression }) }}</span>
          <kbd>{{ props.shortcut }}</kbd>
        </button>
      </div>
      <p v-else-if="presentationStatus !== 'settled'" class="check-dice-rolling">
        <span class="led accent" aria-hidden="true" />{{ t("game.checkTray.rolling") }}
      </p>
      <div v-else class="check-dice-settlement">
        <span class="check-dice-equation">
          {{ diceTotal }}
          <template v-if="props.modifierTotal !== 0">
            {{ props.modifierTotal > 0 ? "+" : "−" }} {{ Math.abs(props.modifierTotal) }}
          </template>
          = <strong>{{ checkTotal }}</strong>
        </span>
        <strong
          class="check-dice-result record-result"
          :class="`result-${props.result ?? 'failure'}`"
        >{{ resultLabel }}</strong>
      </div>
    </footer>
    <p
      class="check-dice-lock"
      :class="{ 'check-dice-lock-hidden': presentationStatus === 'settled' }"
    >
      <template v-if="presentationStatus !== 'settled'">
        <svg class="i sm" viewBox="0 0 24 24" aria-hidden="true">
          <rect x="5" y="11" width="14" height="9" rx="2" />
          <path d="M8 11V8a4 4 0 0 1 8 0v3" />
        </svg>
        {{ t("game.checkTray.locked") }}
      </template>
    </p>
  </section>
</template>
