<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { RecordItem } from "../../../api/records";
import type { DiceSkinId } from "../utils/dice-visual";

const props = withDefaults(defineProps<{
  items: RecordItem[];
  scriptId?: string;
  diceSkin?: DiceSkinId;
  scriptSkinEnabled?: boolean;
  characters?: readonly { id: string; canonicalName: string }[];
  olderCursor?: string;
  loadingOlder: boolean;
  preview?: { text: string; outcome?: string; error?: { code: string; message: string } };
  expandedBody?: string;
  rewindSelectorOpen?: boolean;
  onRetry?: () => void;
  onRequestRewind?: () => void;
  actionsBusy?: boolean;
}>(), {
  diceSkin: "auto",
  scriptSkinEnabled: true,
});
const emit = defineEmits<{
  loadOlder: [];
  loadBody: [bodyRef: string];
  rewind: [recordSeq: number];
}>();
const { t } = useI18n();
const copiedCode = ref<string>();
const copyFailed = ref(false);
async function copyErrorCode(code: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(safeToken(code));
    copiedCode.value = code;
    copyFailed.value = false;
  } catch {
    copiedCode.value = undefined;
    copyFailed.value = true;
  }
}
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
function displayText(text: string): string {
  return text.replace(/\s+$/u, "");
}
const livePreviewText = computed(() => displayText(props.preview?.text ?? ""));

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
    <li v-if="!displayItems.length && !props.preview" class="record-empty">
      {{ t("game.noRecords") }}
    </li>
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
        <p class="record-player-bubble">{{ displayText(item.body.text) }}</p>
      </div>
      <div
        v-else-if="item.kind === 'narration' && item.body && 'text' in item.body"
        class="record-narrator"
      >
        <p>{{ displayText(item.body.text) }}</p>
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
        <p class="record-character-bubble">{{ displayText(item.body.text) }}</p>
      </div>
      <div
        v-else-if="item.kind === 'dice' && item.body && 'expression' in item.body"
        class="record-machine mono"
      >
        <span class="led" aria-hidden="true" />
        <strong>{{ item.body.expression }}</strong>
        <span v-for="(roll, index) in item.body.rolls" :key="index" class="record-dice-value">{{
          roll.value
        }}</span>
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
        <p>{{ displayText(item.body.text) }}</p>
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
      <div v-if="isFailed(item)" class="record-failure" role="status">
        <div class="record-failure-heading">
          <strong>{{ t("game.operationFailed") }}</strong>
          <span v-if="failureMessage(item)"> · {{ failureMessage(item) }}</span>
        </div>
        <div class="record-failure-meta">
          <span
            v-if="item.body && 'error' in item.body && item.body.error.code"
            class="record-failure-code mono"
          >
            {{ t("game.workErrorCode", { code: safeToken(item.body.error.code) }) }}
            <button
              type="button"
              class="btn ghost btn-compact"
              :aria-label="t('game.copyErrorCode')"
              @click.stop="copyErrorCode(item.body.error.code)"
            >
              ⧉
            </button>
          </span>
          <span class="text-xs text-muted">{{ t("game.failureCostUnknown") }}</span>
        </div>
        <div
          v-if="item === displayItems.at(-1) && (onRetry || onRequestRewind)"
          class="record-failure-actions"
        >
          <button
            v-if="onRetry"
            type="button"
            class="btn primary"
            :disabled="actionsBusy"
            @click.stop="onRetry()"
          >
            {{ t("common.retry") }}
          </button>
          <button
            v-if="onRequestRewind"
            type="button"
            class="btn ghost"
            :disabled="actionsBusy"
            @click.stop="onRequestRewind()"
          >
            {{ t("game.rewindToHere") }}
          </button>
        </div>
        <p class="text-xs text-muted">{{ t("game.failedPartialKept") }}</p>
      </div>
    </li>
    <li v-if="props.preview" class="record-row record-live-row">
      <div class="record-narrator record-live" data-testid="live-text">
        {{ livePreviewText }}<span class="record-cursor" aria-hidden="true" />
        <div v-if="props.preview.outcome === 'failed'" class="record-failure" role="status">
          <strong>{{ t("game.operationFailed") }}</strong>
          <template v-if="props.preview.error?.message">
            · {{ props.preview.error.message }}</template>
          <span v-if="props.preview.error?.code" class="record-failure-code mono">
            {{ t("game.workErrorCode", { code: safeToken(props.preview.error.code) }) }}
            <button
              type="button"
              class="btn ghost btn-compact"
              :aria-label="t('game.copyErrorCode')"
              @click="copyErrorCode(props.preview.error.code)"
            >
              ⧉
            </button>
          </span>
          <p class="text-xs text-muted">{{ t("game.failureCostUnknown") }}</p>
          <div v-if="onRetry || onRequestRewind" class="record-failure-actions">
            <button
              v-if="onRetry"
              type="button"
              class="btn primary"
              :disabled="actionsBusy"
              @click="onRetry()"
            >
              {{ t("common.retry") }}
            </button>
            <button
              v-if="onRequestRewind"
              type="button"
              class="btn ghost"
              :disabled="actionsBusy"
              @click="onRequestRewind()"
            >
              {{ t("game.rewindToHere") }}
            </button>
          </div>
          <p class="text-xs text-muted">{{ t("game.failedPartialKept") }}</p>
        </div>
      </div>
    </li>
  </ol>
  <p v-if="copiedCode" class="text-xs text-muted" role="status">{{ t("game.errorCodeCopied") }}</p>
  <p v-if="copyFailed" class="text-xs text-muted" role="status">
    {{ t("game.errorCodeCopyFailed") }}
  </p>
  <p v-if="props.expandedBody !== undefined" class="record-expanded">
    {{ displayText(props.expandedBody) }}
  </p>
</template>

<style scoped>
.record-failure { display: grid; gap: .5rem; border: 1px solid color-mix(in srgb, var(--diff-remove) 35%, transparent); background: color-mix(in srgb, var(--diff-remove) 7%, transparent); border-radius: .75rem; padding: .625rem .75rem; margin-top: .5rem; }
.record-failure-heading, .record-failure-meta, .record-failure-actions { display: flex; gap: .5rem; align-items: center; flex-wrap: wrap; }
.record-failure-heading strong { color: var(--diff-remove); }
.record-failure-heading { color: var(--ink); font-size: .8125rem; }
.record-failure-code { display: inline-flex; gap: .25rem; align-items: center; border: 1px solid var(--hairline); border-radius: .4375rem; padding-left: .5rem; color: var(--muted); font-size: .6875rem; overflow-wrap: anywhere; }
</style>
