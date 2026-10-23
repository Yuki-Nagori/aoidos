<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { DiceRollFact, DiceSkinId } from "../utils/dice-visual";
import { diceSkins, resolveDiceSkin } from "../utils/dice-visual";
import DiceResultView from "./DiceResultView.vue";

type PreviewFormula = "2d6" | "1d20" | "1d8";
type PreviewEffect = "" | "crit" | "fumble";
const props = withDefaults(defineProps<{
  selected: DiceSkinId;
  disabled: boolean;
  scriptId?: string;
  scriptSkinEnabled?: boolean;
}>(), {
  scriptId: "",
  scriptSkinEnabled: true,
});
const emit = defineEmits<{ select: [skin: DiceSkinId] }>();
const { t } = useI18n();
const formula = ref<PreviewFormula>("2d6");
const values = ref<readonly number[]>([4, 6]);
const effect = ref<PreviewEffect>("");
const previewRevision = ref(0);
const previewMotion = ref<"idle" | "rolling" | "result">("idle");
const previewIndex = ref(0);
const formulas: readonly PreviewFormula[] = ["2d6", "1d20", "1d8"];
const previewRolls = computed<DiceRollFact[]>(() => values.value.map((value) => ({
  sides: formula.value === "2d6" ? 6 : formula.value === "1d20" ? 20 : 8,
  value,
})));
const resolvedAutoLabel = computed(() => skinLabel(resolveDiceSkin("auto", props.scriptId, props.scriptSkinEnabled).id));
const options = computed(() => [
  { id: "auto" as const, label: t("settings.diceSkins.auto"), hint: t("settings.diceSkins.autoHint") },
  ...diceSkins.map((skin) => ({
    id: skin.id,
    label: skinLabel(skin.id),
    hint: skin.id === "mistbell" ? t("settings.diceSkins.mistbellHint") : undefined,
  })),
]);

function skinLabel(id: DiceSkinId): string {
  switch (id) {
    case "theme": return t("settings.diceSkins.theme");
    case "obsidian": return t("settings.diceSkins.obsidian");
    case "brass": return t("settings.diceSkins.brass");
    case "jade": return t("settings.diceSkins.jade");
    case "crystal": return t("settings.diceSkins.crystal");
    case "bone": return t("settings.diceSkins.bone");
    case "mistbell": return t("settings.diceSkins.mistbell");
    case "auto": return t("settings.diceSkins.auto");
  }
}

function selectFormula(next: PreviewFormula): void {
  previewMotion.value = "idle";
  formula.value = next;
  values.value = next === "2d6" ? [4, 6] : next === "1d20" ? [20] : [8];
  effect.value = "";
  previewRevision.value += 1;
}

function previewRoll(nextEffect: PreviewEffect): void {
  previewMotion.value = "rolling";
  effect.value = nextEffect;
  if (nextEffect === "crit") values.value = formula.value === "2d6" ? [6, 6] : formula.value === "1d20" ? [20] : [8];
  else if (nextEffect === "fumble") values.value = formula.value === "2d6" ? [1, 1] : [1];
  else {
    const alternate = previewIndex.value++ % 2 !== 0;
    values.value = formula.value === "2d6" ? alternate ? [3, 5] : [4, 6] : formula.value === "1d20" ? alternate ? [7] : [14] : alternate ? [3] : [6];
  }
  previewRevision.value += 1;
}

function selectSkinWithKeyboard(event: KeyboardEvent, index: number): void {
  if (props.disabled || !["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) return;
  event.preventDefault();
  const nextIndex = event.key === "Home" ? 0 : event.key === "End" ? options.value.length - 1 : (index + (event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 1) + options.value.length) % options.value.length;
  const next = options.value[nextIndex];
  if (!next) return;
  emit("select", next.id);
  const buttons = (event.currentTarget as HTMLElement).parentElement?.querySelectorAll<HTMLButtonElement>(".dice-skin-card");
  buttons?.[nextIndex]?.focus();
}
</script>

<template>
  <div class="dice-appearance">
    <section class="dice-preview-section" :aria-label="t('settings.dicePreview.title')">
      <h4 class="dice-section-heading">{{ t("settings.dicePreview.title") }}</h4>
      <div class="dice-preview">
        <div class="dice-preview-stage">
          <DiceResultView
            class="dice-preview-visual"
            :expression="formula"
            :rolls="previewRolls"
            :script-id="props.scriptId"
            :skin="props.selected"
            :script-skin-enabled="props.scriptSkinEnabled"
            :show-pop="true"
            :effect="effect"
            :motion="previewMotion"
            @settled="previewMotion = 'result'"
            :seed="`settings-preview-${previewRevision}`"
          />
        </div>
        <div class="dice-preview-controls">
          <fieldset class="dice-preview-formulas">
            <legend class="sr-only">{{ t("settings.dicePreview.formula") }}</legend>
            <button
              v-for="option in formulas"
              :key="option"
              type="button"
              :class="{ 'is-selected': formula === option }"
              :aria-pressed="formula === option"
              @click="selectFormula(option)"
            >
              {{ option }}<template v-if="option === '2d6' && props.scriptId">
                · {{ t("settings.dicePreview.fromScript") }}</template>
            </button>
          </fieldset>
          <div class="dice-preview-actions">
            <button type="button" class="btn" @click="previewRoll('')">
              <svg class="i sm" viewBox="0 0 24 24" aria-hidden="true">
                <rect x="4" y="4" width="16" height="16" rx="3.5" />
                <path d="M9 9h.01M15 15h.01M15 9h.01M9 15h.01" />
              </svg>
              {{ t("settings.dicePreview.roll") }}
            </button>
            <button type="button" class="btn" @click="previewRoll('crit')">
              {{ t("settings.dicePreview.crit") }}
            </button>
            <button type="button" class="btn" @click="previewRoll('fumble')">
              {{ t("settings.dicePreview.fumble") }}
            </button>
          </div>
          <p class="dice-preview-note">{{ t("settings.dicePreview.note") }}</p>
        </div>
      </div>
    </section>
    <fieldset class="dice-skin-picker">
      <legend class="dice-section-heading">{{ t("settings.dicePreview.skins") }}</legend>
      <div class="dice-skin-grid">
        <button
          v-for="(option, index) in options"
          :key="option.id"
          type="button"
          class="dice-skin-card"
          :class="{ 'is-selected': props.selected === option.id }"
          :aria-pressed="props.selected === option.id"
          :tabindex="props.selected === option.id ? 0 : -1"
          :title="option.hint"
          :disabled="props.disabled"
          @click="emit('select', option.id)"
          @keydown="selectSkinWithKeyboard($event, index)"
        >
          <DiceResultView
            class="dice-skin-preview"
            expression="1d20"
            :rolls="[{ sides: 20, value: 20 }]"
            :script-id="props.scriptId"
            :skin="option.id"
            :script-skin-enabled="props.scriptSkinEnabled"
            :seed="`settings-skin-${option.id}`"
            aria-hidden="true"
          />
          <span class="dice-skin-copy">
            <strong>{{ option.label }}</strong>
            <small v-if="option.id === 'auto'">{{
              t("settings.dicePreview.resolvedAuto", { skin: resolvedAutoLabel })
            }}</small>
          </span>
        </button>
      </div>
      <p class="dice-skin-description">{{ t("settings.diceSkinDescription") }}</p>
    </fieldset>
  </div>
</template>
