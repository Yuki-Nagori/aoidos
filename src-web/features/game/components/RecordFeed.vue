<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { RecordItem } from "../../../api/records";

const props = defineProps<{
  items: RecordItem[];
  characters?: readonly { id: string; canonicalName: string }[];
  olderCursor?: string;
  loadingOlder: boolean;
  preview?: { text: string; outcome?: string; error?: { code: string; message: string } };
  expandedBody?: string;
  rewindSelectorOpen?: boolean;
}>();
const emit = defineEmits<{
  loadOlder: [];
  loadBody: [bodyRef: string];
  rewind: [recordSeq: number];
}>();
const { t } = useI18n();
const displayItems = computed(() => {
  return props.items.filter(
    (item) =>
      !(
        item.kind === "system" &&
        item.body &&
        "code" in item.body &&
        !["checkPlanned", "checkSkipped"].includes(item.body.code) &&
        item.body.message === "引擎确认回合事实"
      ),
  );
});

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

function checkRuleName(ruleId: string): string {
  return ruleId === "pbta-explore-v1"
    ? t("game.checkRules.pbtaExplore")
    : safeToken(ruleId);
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

function isFailed(item: RecordItem): boolean {
  return (
    item.outcome === "failed" ||
    (!!item.body && "outcome" in item.body && item.body.outcome === "failed")
  );
}

function isRewindTarget(item: RecordItem): boolean {
  return !!item.body && "code" in item.body && ["checkPlanned", "checkSkipped"].includes(item.body.code);
}

function selectRewindTarget(item: RecordItem): void {
  if (props.rewindSelectorOpen && isRewindTarget(item)) emit("rewind", item.recordSeq);
}

function failureMessage(item: RecordItem): string | undefined {
  const body = item.body;
  if (!body || !("error" in body)) return undefined;
  const message = systemRecordMessage(body.error.message);
  return message === t("game.systemRecordUnavailable") ? undefined : message;
}

function isOutOfCharacter(item: RecordItem): boolean {
  return !!item.body && "mode" in item.body && item.body.mode === "outOfCharacter";
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
    <li v-if="!displayItems.length" class="record-empty">{{ t("game.noRecords") }}</li>
    <li
      v-for="item in displayItems"
      :key="item.recordSeq"
      class="record-row"
      :class="{ 'record-rewind-target': props.rewindSelectorOpen && isRewindTarget(item) }"
      :tabindex="props.rewindSelectorOpen && isRewindTarget(item) ? 0 : undefined"
      :aria-label="props.rewindSelectorOpen && isRewindTarget(item) ? t('game.rewindToHere') : undefined"
      @click="selectRewindTarget(item)"
      @keydown.enter.space.prevent="selectRewindTarget(item)"
    >
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
        <span>{{ checkRuleName(item.body.ruleId) }}</span>
        <span class="record-result" :class="`result-${item.body.result}`">
          {{ checkResultName(item.body.result) }}
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
        <span>{{ systemRecordMessage(item.body.message) }}</span>
      </p>
      <p v-else class="record-compat">{{ t("game.recordUnavailable") }}</p>
      <p v-if="isFailed(item)" class="record-failure" role="status">
        <strong>{{ t("game.operationFailed") }}</strong>
        <span v-if="failureMessage(item)"> · {{ failureMessage(item) }}</span>
        <span v-if="item.body && 'error' in item.body && item.body.error.code">
          · {{ t("game.workErrorCode", { code: safeToken(item.body.error.code) }) }}
        </span>
      </p>
    </li>
  </ol>
  <p v-if="props.preview" class="record-narrator record-live" data-testid="live-text">
    {{ props.preview.text }}<span class="record-cursor" aria-hidden="true" />
    <span v-if="props.preview.outcome === 'failed'" class="record-failure" role="status">
      <strong>{{ t("game.operationFailed") }}</strong>
      <template v-if="props.preview.error?.message"> · {{ props.preview.error.message }}</template>
      <template v-if="props.preview.error?.code">
        · {{ t("game.workErrorCode", { code: safeToken(props.preview.error.code) }) }}
      </template>
    </span>
  </p>
  <p v-if="props.expandedBody !== undefined" class="record-expanded">{{ props.expandedBody }}</p>
</template>
