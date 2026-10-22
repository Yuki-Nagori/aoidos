<script setup lang="ts">
import { useI18n } from "vue-i18n";
import type { RecordItem } from "../../../api/records";

const props = defineProps<{
  items: RecordItem[];
  characters?: readonly { id: string; canonicalName: string }[];
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

function characterName(id: string): string {
  return props.characters?.find((character) => character.id === id)?.canonicalName ?? id;
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
      return t("game.unknownCheckResult", { result: safeToken(result) });
  }
}

function safeToken(value: string): string {
  const safe = Array.from(value)
    .filter((character) => {
      const codePoint = character.codePointAt(0) ?? 0;
      return codePoint >= 0x20 && !(codePoint >= 0x7f && codePoint <= 0x9f);
    })
    .join("")
    .trim();
  const characters = Array.from(safe);
  return characters.length > 80 ? `${characters.slice(0, 80).join("")}…` : safe;
}

function systemRecordMessage(message: string): string {
  const safeMessage = Array.from(message)
    .filter((character) => {
      const codePoint = character.codePointAt(0) ?? 0;
      return codePoint === 0x09 || codePoint === 0x0a || (codePoint >= 0x20 && !(codePoint >= 0x7f && codePoint <= 0x9f));
    })
    .join("")
    .trim();
  if (!safeMessage) return t("game.systemRecordUnavailable");
  const characters = Array.from(safeMessage);
  return characters.length > 2_000 ? `${characters.slice(0, 2_000).join("")}…` : safeMessage;
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
      return safeToken(outcome);
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
      return safeToken(reason);
  }
}

function relatedDice(item: RecordItem) {
  if (!item.body || !("diceSeq" in item.body)) return undefined;
  const recordSeq = item.body.diceSeq;
  const body = props.items.find((candidate) => candidate.recordSeq === recordSeq)?.body;
  return body && "expression" in body ? body : undefined;
}

function isOutOfCharacter(item: RecordItem): boolean {
  return !!item.body && "mode" in item.body && item.body.mode === "outOfCharacter";
}

function checkDifficulty(dc: number | undefined): string {
  return t("game.checkDifficulty", {
    dc: dc === undefined ? t("game.noDifficulty") : dc,
  });
}
</script>

<template>
  <button
    v-if="props.olderCursor"
    type="button"
    class="record-load-older"
    :disabled="props.loadingOlder"
    @click="emit('loadOlder')"
  >
    {{ t("game.earlierRecords") }}
  </button>
  <ol class="record-list" :aria-label="t('game.dialogue')">
    <li v-if="!props.items.length" class="record-empty">{{ t("game.noRecords") }}</li>
    <li v-for="item in props.items" :key="item.recordSeq" class="record-row">
      <div
        v-if="item.kind === 'playerSpeech' && item.body && 'text' in item.body"
        class="record-player"
      >
        <span v-if="isOutOfCharacter(item)" class="record-ooc">
          {{ t("game.recordOocTag") }}
        </span>
        <p class="record-player-bubble">{{ item.body.text }}</p>
      </div>
      <div
        v-else-if="item.kind === 'narration' && item.body && 'text' in item.body"
        class="record-narrator"
      >
        <p>{{ item.body.text }}</p>
        <span
          v-if="item.outcome || ('outcome' in item.body && item.body.outcome)"
          class="record-terminal"
        >
          {{ outcomeName(item.outcome ?? ("outcome" in item.body ? item.body.outcome : "")) }}
          <template v-if="'finishReason' in item.body && item.body.finishReason">
            · {{ finishReasonName(item.body.finishReason ?? "") }}
          </template>
        </span>
      </div>
      <div
        v-else-if="item.kind === 'characterSpeech' && item.body && 'text' in item.body"
        class="record-character"
      >
        <div class="record-nameplate">
          <span class="record-avatar" aria-hidden="true" />
          <strong>{{
            "speakerId" in item.body
            ? characterName(item.body.speakerId ?? "")
            : t("game.recordKinds.character")
          }}</strong>
        </div>
        <p class="record-character-bubble">{{ item.body.text }}</p>
        <span v-if="item.outcome" class="record-terminal">
          {{ outcomeName(item.outcome) }}
          <template v-if="'finishReason' in item.body && item.body.finishReason">
            · {{ finishReasonName(item.body.finishReason) }}
          </template>
        </span>
      </div>
      <div
        v-else-if="item.kind === 'dice' && item.body && 'expression' in item.body"
        class="record-machine mono"
      >
        <span class="led" aria-hidden="true" />
        <strong>{{ item.body.expression }}</strong>
        <span
          v-for="(roll, index) in item.body.rolls"
          :key="`${item.recordSeq}:${index}`"
          class="record-die"
        >
          {{ roll.value }}
        </span>
        <span class="record-machine-total">= {{ item.body.total }}</span>
      </div>
      <div
        v-else-if="item.kind === 'check' && item.body && 'result' in item.body"
        class="record-machine mono"
      >
        <span class="led" aria-hidden="true" />
        <span>{{ item.body.ruleId }}</span>
        <template v-if="relatedDice(item)">
          <span>{{ relatedDice(item)!.expression }}</span>
          <span
            v-for="(roll, index) in relatedDice(item)!.rolls"
            :key="`${item.recordSeq}:${index}`"
            class="record-die"
          >
            {{ roll.value }}
          </span>
          <span class="record-machine-total">= {{ relatedDice(item)!.total }}</span>
        </template>
        <span class="record-result" :class="`result-${item.body.result}`">
          {{ checkResultName(item.body.result) }}
        </span>
        <span class="record-machine-muted">
          · {{ checkDifficulty(item.body.dc) }}
        </span>
      </div>
      <details
        v-else-if="item.kind === 'recap' && item.body && 'text' in item.body"
        class="record-recap"
      >
        <summary>{{ t("game.recap") }}</summary>
        <p>{{ item.body.text }}</p>
      </details>
      <button
        v-else-if="item.bodyRef"
        type="button"
        class="record-read-body"
        @click="emit('loadBody', item.bodyRef)"
      >
        {{ t("common.readBody") }}
      </button>
      <p
        v-else-if="item.body && 'code' in item.body && item.body.code === 'checkPlanned'"
        class="record-machine mono"
      >
        <span class="led accent" aria-hidden="true" />{{ t("game.checkPlanned") }}
      </p>
      <p
        v-else-if="item.body && 'code' in item.body && item.body.code === 'checkSkipped'"
        class="record-machine mono"
      >
        <span class="led" aria-hidden="true" />{{ t("game.checkSkipped") }}
      </p>
      <p
        v-else-if="item.kind === 'system' && item.body && 'code' in item.body"
        class="record-system"
      >
        <span class="led" aria-hidden="true" />
        {{ systemRecordMessage(item.body.message) }}
      </p>
      <p v-else class="record-compat">{{ t("game.recordUnavailable") }}</p>
      <button
        v-if="props.canRewind && item.kind === 'system' && item.body && 'code' in item.body && ['checkPlanned', 'checkSkipped'].includes(item.body.code)"
        type="button"
        class="record-rewind"
        @click="emit('rewind', item.recordSeq)"
      >
        {{ t("game.rewindToHere") }}
      </button>
    </li>
  </ol>
  <p v-if="props.preview" class="record-narrator record-live" data-testid="live-text">
    {{ props.preview.text }}<span class="record-cursor" aria-hidden="true" />
    <span v-if="props.preview.outcome" class="record-terminal">
      {{ outcomeName(props.preview.outcome) }}
      <template v-if="props.preview.finishReason">
        · {{ finishReasonName(props.preview.finishReason) }}
      </template>
    </span>
  </p>
  <p v-if="props.expandedBody !== undefined" class="record-expanded">{{ props.expandedBody }}</p>
  <p v-if="props.generation" class="record-system">
    <span class="led" aria-hidden="true" />
    {{ t("game.generation", { outcome: outcomeName(props.generation.outcome) }) }}
    <template v-if="props.generation.finishReason">
      · {{ finishReasonName(props.generation.finishReason) }}
    </template>
  </p>
</template>
