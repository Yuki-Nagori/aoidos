<script setup lang="ts">
import { useI18n } from "vue-i18n";
import type { RecordItem } from "../api/records";

const props = defineProps<{
  items: RecordItem[];
  olderCursor?: string;
  loadingOlder: boolean;
  preview?: { text: string; outcome?: string; finishReason?: string };
  expandedBody?: string;
  generation?: { outcome: string; finishReason?: string };
  canRewind: boolean;
}>();
const emit = defineEmits<{
  loadOlder: [];
  loadBody: [bodyRef: string];
  rewind: [recordSeq: number];
}>();
const { t } = useI18n();

function recordKindName(kind: string): string {
  switch (kind) {
    case "playerSpeech":
      return t("game.recordKinds.playerSpeech");
    case "narration":
      return t("game.recordKinds.narration");
    case "characterSpeech":
      return t("game.recordKinds.character");
    case "recap":
      return t("game.recordKinds.recap");
    case "dice":
      return t("game.recordKinds.dice");
    case "system":
      return t("game.recordKinds.system");
    case "check":
      return t("game.recordKinds.check");
    case "header":
      return t("game.recordKinds.header");
    default:
      return kind;
  }
}

function checkResultName(result: string): string {
  switch (result) {
    case "success":
      return t("game.checkResults.success");
    case "costlySuccess":
      return t("game.checkResults.costlySuccess");
    case "failure":
      return t("game.checkResults.failure");
    case "criticalSuccess":
      return t("game.checkResults.criticalSuccess");
    case "criticalFailure":
      return t("game.checkResults.criticalFailure");
    default:
      return t("game.compatRecord");
  }
}

function outcomeName(outcome: string): string {
  switch (outcome) {
    case "completed":
      return t("game.outcomes.completed");
    case "cancelled":
      return t("game.outcomes.cancelled");
    case "failed":
      return t("game.outcomes.failed");
    default:
      return outcome;
  }
}

function finishReasonName(reason: string): string {
  switch (reason) {
    case "stop":
      return t("game.finishReasons.stop");
    case "guard":
      return t("game.finishReasons.guard");
    case "length":
      return t("game.finishReasons.length");
    default:
      return reason;
  }
}
</script>

<template>
  <button
    v-if="props.olderCursor"
    class="mb-3 w-full rounded-lg border border-hairline px-3 py-2 text-xs text-muted"
    :disabled="props.loadingOlder"
    @click="emit('loadOlder')"
  >
    {{ t("game.earlierRecords") }}
  </button>
  <ol class="record-list" :aria-label="t('game.dialogue')">
    <li v-if="!props.items.length" class="record-system">{{ t("game.noRecords") }}</li>
    <li
      v-for="item in props.items"
      :key="item.recordSeq"
      class="record-entry"
      :class="
        item.kind === 'playerSpeech'
          ? 'record-player'
          : item.kind === 'narration'
            ? 'record-narrator'
            : item.kind === 'characterSpeech'
              ? 'record-story'
              : 'record-system'
      "
    >
      <span class="record-kind">{{
        t("game.recordKind", { kind: recordKindName(item.kind) })
      }}</span>
      <details v-if="item.kind === 'recap' && item.body && 'text' in item.body">
        <summary>{{ t("game.recap") }}</summary>
        <p class="mt-2 whitespace-pre-wrap break-words">{{ item.body.text }}</p>
      </details>
      <p v-else-if="item.body && 'text' in item.body" class="whitespace-pre-wrap break-words">
        {{ item.body.text }}
      </p>
      <div v-else-if="item.body && 'expression' in item.body" class="whitespace-pre-wrap">
        {{
          t("game.rollSummary", {
            expression: item.body.expression,
            total: item.body.total,
            rolls: item.body.rolls
              .map((roll) => t("game.rollDie", { sides: roll.sides, value: roll.value }))
              .join(", "),
          })
        }}
      </div>
      <div v-else-if="item.body && 'result' in item.body" class="whitespace-pre-wrap">
        {{
          t("game.checkSummary", {
            result: checkResultName(item.body.result),
            dc: item.body.dc ?? t("game.noDifficulty"),
          })
        }}
      </div>
      <button
        v-else-if="item.bodyRef"
        class="text-xs text-ink underline"
        @click="emit('loadBody', item.bodyRef)"
      >
        {{ t("common.readBody") }}
      </button>
      <p v-else-if="item.body && 'code' in item.body && item.body.code === 'checkPlanned'">
        {{ t("game.checkPlanned") }}
      </p>
      <p v-else-if="item.body && 'code' in item.body && item.body.code === 'checkSkipped'">
        {{ t("game.checkSkipped") }}
      </p>
      <p v-else class="whitespace-pre-wrap break-words">{{ t("game.compatRecord") }}</p>
      <button
        v-if="
          props.canRewind &&
          item.kind === 'system' &&
          item.body &&
          'code' in item.body &&
          ['checkPlanned', 'checkSkipped'].includes(item.body.code)
        "
        class="mt-2 text-xs text-danger"
        @click="emit('rewind', item.recordSeq)"
      >
        {{ t("game.rewindToHere") }}
      </button>
    </li>
  </ol>
  <pre
    v-if="props.preview"
    class="record-entry record-story whitespace-pre-wrap break-words"
    data-testid="live-text"
    >{{ props.preview.text }}</pre>
  <pre
    v-if="props.expandedBody !== undefined"
    class="record-expanded whitespace-pre-wrap break-words"
    >{{ props.expandedBody }}</pre>
  <p v-if="props.generation" class="record-system">
    {{ t("game.generation", { outcome: outcomeName(props.generation.outcome) }) }}
    <span v-if="props.generation.finishReason">{{
      finishReasonName(props.generation.finishReason)
    }}</span>
  </p>
  <p v-else-if="props.preview?.outcome" class="record-system">
    {{ t("game.generation", { outcome: outcomeName(props.preview.outcome) }) }}
    {{ finishReasonName(props.preview.finishReason ?? "") }}
  </p>
</template>
