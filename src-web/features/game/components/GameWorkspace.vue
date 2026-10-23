<script setup lang="ts">
import { computed, onScopeDispose, ref, toRef, watch } from "vue";
import ActionComposer from "./ActionComposer.vue";
import CheckDiceTray from "./CheckDiceTray.vue";
import GameStage from "./GameStage.vue";
import RecordFeed from "./RecordFeed.vue";
import type { OpenedSession, PhaseState } from "../../../api/engine";
import type { TurnSnapshot } from "../../../api/llm";
import type { RecordItem } from "../../../api/records";
import type { useUiPreferences } from "../composables/useUiPreferences";
import type { CommandSuggestion } from "../utils/ui-shell";
import { useGameWorkspace } from "../composables/useGameWorkspace";
import type { DiceRollFact } from "../utils/dice-visual";

type PlannedCheck = NonNullable<PhaseState["check"]>;
type CheckTrayState = Omit<PlannedCheck, "status"> & {
  status: "waiting" | "rolling" | "settled";
  rolls: DiceRollFact[];
  total?: number;
  result?: "success" | "costlySuccess" | "failure" | "criticalSuccess" | "criticalFailure";
};

const props = defineProps<{
  session: OpenedSession;
  scriptTitle: string;
  suggestedAction?: string;
  scriptSkinEnabled?: boolean;
  historyRevision?: number;
  productBusy: boolean;
  preferences: ReturnType<typeof useUiPreferences>;
  errorMessage?: string;
}>();
const emit = defineEmits<{
  error: [unknown];
  historyRevision: [number];
  openSettings: [trigger: HTMLElement];
}>();
const composer = ref<{ focus(): void }>();
const recordsFeed = ref<HTMLElement>();
const confirmDialog = ref<HTMLDialogElement>();
const {
  session, t, phase, records, current, presence, sceneHint, input, controls, preferences, loadingOlder, unreadRecords,
  panelProtected, confirmation, rewindSelectorOpen,
  slashMenuDismissed, panel, orderedRecords, composerPreview,
  composerSubmitDisabled, composerSubmitLabel, scenePath, olderCursor, game, preview, activeTurnId,
  hasTextSelection, updateFollowState, onReadingPointerDown, jumpToLatest, onComposerKeydown,
  interruptDraft, submitComposer, confirmOperation, requestRegenerate, selectRewindTarget, chooseSlashCommand, closePanel, loadOlder,
  phaseName, reconnect, connectionFailed,
} = useGameWorkspace({
  session: toRef(props, "session"),
  composer,
  recordsFeed,
  confirmDialog,
  productBusy: toRef(props, "productBusy"),
  preferences: toRef(props, "preferences"),
  emitError: (error) => emit("error", error),
  emitHistoryRevision: (revision) => emit("historyRevision", revision),
});
const availableCommands = computed<CommandSuggestion[]>(() => {
  const commands: CommandSuggestion[] = ["ooc", "pin", "latest"];
  if (current.value?.inFlight?.roundId && !current.value.needsRecovery) {
    commands.push("interrupt", "stop");
  }
  if (current.value?.resumeRequired && !current.value.needsRecovery) commands.push("resume");
  if (
    current.value?.phase === "idle" &&
    !current.value.needsRecovery &&
    !current.value.resumeRequired &&
    current.value.lastOperation?.roundId
  ) {
    commands.push("regenerate");
  }
  if (current.value?.phase === "idle" && !current.value.needsRecovery && !current.value.resumeRequired)
    commands.push("rewind");
  return commands;
});
const starterSuggestion = computed(() =>
  orderedRecords.value.some((item) => item.kind === "playerSpeech") ? undefined : props.suggestedAction,
);
const pendingCheck = ref<PlannedCheck>();
const settledCheck = ref<CheckTrayState>();
const dicePresentationPending = ref(false);
const narrationReplayActive = ref(false);
const pendingNarrationTurnId = ref<string>();
const heldNarration = ref<TurnSnapshot>();
const narrationBufferText = ref("");
const narrationDisplayedText = ref("");
const narrationStreamFinished = ref(false);
let settledCheckTimer: ReturnType<typeof setTimeout> | undefined;
let narrationFallbackTimer: ReturnType<typeof setTimeout> | undefined;
let narrationReplayTimer: ReturnType<typeof setInterval> | undefined;
function findPlanRecord(planId: string, kind: "dice" | "check"): RecordItem | undefined {
  return orderedRecords.value.find((item) =>
    item.kind === kind && item.body && "planId" in item.body && item.body.planId === planId,
  );
}
function settleVisibleCheck(): void {
  if (current.value?.check || !pendingCheck.value) return;
  const plan = pendingCheck.value;
  const diceRecord = findPlanRecord(plan.planId, "dice");
  const checkRecord = findPlanRecord(plan.planId, "check");
  if (!diceRecord?.body || !("rolls" in diceRecord.body) || !("total" in diceRecord.body)) return;
  if (!checkRecord?.body || !("result" in checkRecord.body)) return;
  settledCheck.value = {
    ...plan,
    status: "settled",
    rolls: diceRecord.body.rolls,
    total: diceRecord.body.total,
    result: checkRecord.body.result,
  };
  pendingCheck.value = undefined;
  if (settledCheckTimer) clearTimeout(settledCheckTimer);
  settledCheckTimer = undefined;
}
function captureNarration(snapshot: TurnSnapshot | undefined): void {
  if (!snapshot || (!dicePresentationPending.value && !narrationReplayActive.value)) return;
  if (pendingNarrationTurnId.value && snapshot.turnId !== pendingNarrationTurnId.value) return;
  pendingNarrationTurnId.value = snapshot.turnId;
  heldNarration.value = snapshot;
  narrationBufferText.value = snapshot.text;
  if (snapshot.outcome) narrationStreamFinished.value = true;
  if (narrationReplayActive.value) startNarrationReplay();
}
function revealNarrationChunk(): void {
  const characters = Array.from(narrationBufferText.value);
  const displayed = Array.from(narrationDisplayedText.value);
  if (displayed.length < characters.length) {
    narrationDisplayedText.value = characters.slice(0, displayed.length + 1).join("");
    return;
  }
  if (narrationStreamFinished.value) {
    if (narrationReplayTimer) clearInterval(narrationReplayTimer);
    narrationReplayTimer = undefined;
    narrationReplayActive.value = false;
    pendingNarrationTurnId.value = undefined;
    heldNarration.value = undefined;
  }
}
function startNarrationReplay(): void {
  if (!narrationReplayActive.value || narrationReplayTimer) return;
  narrationReplayTimer = setInterval(revealNarrationChunk, 45);
}
function onCheckPresentationSettled(): void {
  dicePresentationPending.value = false;
  if (narrationFallbackTimer) clearTimeout(narrationFallbackTimer);
  narrationFallbackTimer = undefined;
  narrationReplayActive.value = true;
  startNarrationReplay();
  if (!settledCheck.value || settledCheckTimer) return;
  settledCheckTimer = setTimeout(() => {
    settledCheck.value = undefined;
    settledCheckTimer = undefined;
  }, 1800);
}
watch(() => current.value?.check, (check) => {
  if (check) {
    if (check.status === "rolling") {
      dicePresentationPending.value = true;
      pendingNarrationTurnId.value = activeTurnId.value ?? preview.value?.turnId;
      narrationReplayActive.value = false;
      narrationBufferText.value = "";
      narrationDisplayedText.value = "";
      narrationStreamFinished.value = false;
      heldNarration.value = undefined;
      captureNarration(preview.value);
      if (narrationFallbackTimer) clearTimeout(narrationFallbackTimer);
      narrationFallbackTimer = setTimeout(onCheckPresentationSettled, 8000);
    }
    pendingCheck.value = check;
    settledCheck.value = undefined;
    if (settledCheckTimer) clearTimeout(settledCheckTimer);
    settledCheckTimer = undefined;
  } else {
    settleVisibleCheck();
  }
}, { immediate: true });
watch(orderedRecords, settleVisibleCheck, { deep: true });
watch(preview, captureNarration, { immediate: true });
watch(() => activeTurnId.value ?? preview.value?.turnId, (turnId) => {
  if (dicePresentationPending.value && turnId) pendingNarrationTurnId.value = turnId;
});
watch(() => preview.value?.outcome, (outcome) => {
  if (outcome) narrationStreamFinished.value = true;
});
watch(() => game.latestPublicRecord.value?.turnId, (turnId) => {
  if (turnId && turnId === pendingNarrationTurnId.value) {
    narrationStreamFinished.value = true;
    if (narrationReplayActive.value) startNarrationReplay();
  }
});
const trayCheck = computed<CheckTrayState | undefined>(() => {
  if (current.value?.check) return { ...current.value.check, rolls: [] };
  return settledCheck.value;
});
const visibleRecords = computed(() => {
  const turnId = pendingNarrationTurnId.value;
  if ((!dicePresentationPending.value && !narrationReplayActive.value) || !turnId)
    return orderedRecords.value;
  return orderedRecords.value.filter(
    (item) =>
      item.turnId !== turnId ||
      (item.kind !== "narration" && item.kind !== "characterSpeech"),
  );
});
const visiblePreview = computed(() =>
  dicePresentationPending.value
    ? undefined
    : narrationReplayActive.value && heldNarration.value
      ? { ...heldNarration.value, text: narrationDisplayedText.value }
      : narrationReplayActive.value
        ? undefined
        : preview.value,
);
onScopeDispose(() => {
  if (settledCheckTimer) clearTimeout(settledCheckTimer);
  if (narrationFallbackTimer) clearTimeout(narrationFallbackTimer);
  if (narrationReplayTimer) clearInterval(narrationReplayTimer);
});
function resetCheckTray(restoreCurrentCheck = false): void {
  pendingCheck.value = undefined;
  settledCheck.value = undefined;
  dicePresentationPending.value = false;
  narrationReplayActive.value = false;
  pendingNarrationTurnId.value = undefined;
  heldNarration.value = undefined;
  narrationBufferText.value = "";
  narrationDisplayedText.value = "";
  narrationStreamFinished.value = false;
  if (settledCheckTimer) clearTimeout(settledCheckTimer);
  settledCheckTimer = undefined;
  if (narrationFallbackTimer) clearTimeout(narrationFallbackTimer);
  narrationFallbackTimer = undefined;
  if (narrationReplayTimer) clearInterval(narrationReplayTimer);
  narrationReplayTimer = undefined;
  if (restoreCurrentCheck) pendingCheck.value = current.value?.check;
}
watch(() => props.session.sessionId, () => resetCheckTray(), { flush: "sync" });
watch(() => props.historyRevision, () => resetCheckTray(true), { flush: "post" });
const checkShortcut = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform)
  ? "⌘ Enter"
  : "Ctrl Enter";
defineExpose({ reconnect });
</script>
<template>
  <section class="contents">
    <GameStage
      :panel-state="panel.state.value"
      :panel-open="panel.isOpen.value"
      :modal-open="Boolean(confirmation)"
      :title="scriptTitle"
      :script-id="session.scriptId"
      :scene-available="Boolean(current?.scene)"
      :scene-label="t('game.scene')"
      :phase="current?.needsRecovery || current?.resumeRequired ? 'needsRecovery' : current?.phase ?? 'idle'"
      :phase-label="current?.needsRecovery ? t('game.needsRecovery') : current?.resumeRequired ? t('game.paused') : current ? phaseName(current.phase) : t('common.loading')"
      :trigger-label="t('game.openPanel')"
      :settings-label="t('settings.title')"
      :pin-label="t('game.pinPanel')"
      :unpin-label="t('game.unpinPanel')"
      :close-label="t('game.closePanel')"
      @trigger-enter="panel.enterTrigger"
      @trigger-leave="panel.leaveTrigger"
      @panel-enter="panel.enterPanel"
      @panel-leave="panel.leavePanel"
      @open="panel.open"
      @interact="panel.interact"
      @escape="panel.escape"
      @pin="panel.pin"
      @unpin="panel.unpin"
      @close="closePanel"
      @settings="emit('openSettings', $event)"
    >
      <template #summary>
        <slot name="summary" />
      </template>
      <template #scene>
        <CheckDiceTray
          v-if="panel.state.value === 'collapsed' && trayCheck"
          class="check-dice-collapsed"
          :expression="trayCheck.expression"
          :mode="trayCheck.mode"
          :status="trayCheck.status"
          :rule-id="trayCheck.ruleId"
          :keyboard-active="panel.state.value === 'collapsed'"
          :rolls="trayCheck.rolls"
          :modifier-total="trayCheck.modifierTotal"
          :total="trayCheck.total"
          :result="trayCheck.result"
          :disabled="controls.busy.value || (current?.needsRecovery ?? false)"
          :script-id="session.scriptId"
          :skin="preferences.diceSkin.value"
          :script-skin-enabled="props.scriptSkinEnabled ?? true"
          :shortcut="checkShortcut"
          @presentation-settled="onCheckPresentationSettled"
          @submit="controls.control('check')"
        />
        <div v-if="current?.scene" class="scene-hud">
          <p v-if="scenePath" class="scene-breadcrumb">{{ scenePath }}</p>
          <button
            class="scene-hint-button"
            type="button"
            :disabled="sceneHint.loading.value || !current?.scene || current.needsRecovery"
            @click="sceneHint.request"
          >
            {{ sceneHint.loading.value ? t("game.hintLoading") : t("game.requestHint") }}
          </button>
          <p
            v-for="hint in sceneHint.hints.value"
            :key="hint.hintId"
            class="scene-hint-result"
            role="status"
          >
            {{ hint.text }}
          </p>
          <p v-if="sceneHint.unavailable.value" class="scene-hint-result" role="status">
            {{ t("game.hintUnavailable") }}
          </p>
          <p v-if="sceneHint.failed.value" class="scene-hint-result scene-hint-error" role="alert">
            {{ t("game.hintFailed") }}
          </p>
          <p v-if="current?.resumeRequired" class="scene-hint-result scene-hint-error">
            {{ t("game.paused") }}
          </p>
        </div>
        <div v-else class="scene-empty-state">
          <h2>{{ props.scriptTitle }}</h2>
          <p>{{ current?.needsRecovery ? t("game.paused") : t("game.scenePrompt") }}</p>
        </div>
      </template>
      <template #panel-header>
        <h2 id="dialogue-panel-heading" class="min-w-0 flex-1 truncate text-sm font-semibold">
          {{ t("game.dialogue") }}
        </h2>
      </template>
      <template #panel>
        <!-- biome-ignore lint/a11y/noStaticElementInteractions: 这些事件只追踪文本选择和阅读状态，此面板不是激活目标。 -->
        <div
          class="dialogue-panel-content flex min-h-0 flex-1 flex-col"
          @focusin="panelProtected = true"
          @focusout="panelProtected = Boolean(
            ($event.relatedTarget as Node | null)?.parentElement?.closest('.dialogue-panel'),
          )"
          @mouseup="panelProtected = hasTextSelection()"
          @keyup="panelProtected = hasTextSelection()"
          @pointerdown="onReadingPointerDown"
        >
          <div class="dialogue-panel-history">
            <p
              v-if="props.errorMessage"
              class="mx-3 mt-3 rounded-lg border border-danger bg-panel p-3 text-sm text-danger"
              role="alert"
            >
              {{ props.errorMessage }}
              <button
                v-if="connectionFailed"
                type="button"
                class="ml-2 underline"
                :disabled="phase.connecting.value"
                @click="reconnect"
              >
                {{ t("common.reconnect") }}
              </button>
            </p>
            <div
              v-if="!game.historyPending.value"
              ref="recordsFeed"
              class="record-feed"
              :class="{ 'record-feed-selection-active': panelProtected }"
              @scroll="updateFollowState"
            >
              <RecordFeed
                :items="visibleRecords"
                :script-id="session.scriptId"
                :dice-skin="preferences.diceSkin.value"
                :script-skin-enabled="props.scriptSkinEnabled ?? true"
                :rewind-selector-open="rewindSelectorOpen"
                :on-retry="requestRegenerate"
                :actions-busy="controls.busy.value"
                :characters="presence.snapshot.value?.characters ?? []"
                :older-cursor="olderCursor"
                :loading-older="loadingOlder"
                :preview="visiblePreview"
                :expanded-body="records.state.value.body"
                @load-older="loadOlder"
                @load-body="records.loadBody"
                @rewind="selectRewindTarget"
              />
            </div>
            <p v-else role="status" class="m-3 rounded-lg bg-panel p-3 text-sm">
              {{ t("game.historyPending") }}
            </p>
            <button type="button" v-if="unreadRecords" class="jump-latest" @click="jumpToLatest">
              {{ t("game.newRecords") }}
            </button>
          </div>
          <dialog
            ref="confirmDialog"
            class="confirmation-card"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="confirmation-message"
            @cancel.prevent="confirmation = undefined"
          >
            <p id="confirmation-message">
              {{
                confirmation?.kind === "rewind" ? t("game.rewindConfirm") : t("game.regenerateConfirm")
              }}
            </p>
            <div class="mt-3 flex justify-end gap-2">
              <button type="button" class="btn" @click="confirmation = undefined">
                {{ t("common.cancel") }}
              </button>
              <button
                type="button"
                class="btn danger"
                :disabled="controls.busy.value"
                @click="confirmOperation"
              >
                {{ t("game.confirmDestructive") }}
              </button>
            </div>
          </dialog>
          <div class="dialogue-panel-composer shrink-0 border-t border-hairline p-3">
            <div class="game-composer-status text-xs text-muted">
              <span v-if="preferences.saveError.value" role="status">{{
                t("game.preferenceSaveFailed")
              }}</span>
              <span v-if="preferences.loadError.value" role="status">
                {{ t("game.preferenceLoadFailed") }}
                <button type="button" class="btn ghost btn-compact" @click="preferences.load">
                  <svg class="i sm" viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M20 12a8 8 0 1 1-2.4-5.7" />
                    <path d="M20 4v4.5h-4.5" />
                  </svg>
                  {{ t("common.retry") }}
                </button>
              </span>
            </div>
            <CheckDiceTray
              v-if="trayCheck"
              :expression="trayCheck.expression"
              :mode="trayCheck.mode"
              :status="trayCheck.status"
              :rule-id="trayCheck.ruleId"
              :keyboard-active="panel.isOpen.value"
              :rolls="trayCheck.rolls"
              :modifier-total="trayCheck.modifierTotal"
              :total="trayCheck.total"
              :result="trayCheck.result"
              :disabled="controls.busy.value || (current?.needsRecovery ?? false)"
              :script-id="session.scriptId"
              :skin="preferences.diceSkin.value"
              :script-skin-enabled="props.scriptSkinEnabled ?? true"
              :shortcut="checkShortcut"
              @presentation-settled="onCheckPresentationSettled"
              @submit="controls.control('check')"
            />
            <ActionComposer
              v-else
              ref="composer"
              v-model="input.draft.value"
              :suggested-action="starterSuggestion"
              :preview="composerPreview"
              :submit-label="composerSubmitLabel"
              :too-large="input.tooLarge.value"
              :slash-menu-dismissed="slashMenuDismissed"
              :rewind-selector-open="rewindSelectorOpen"
              :in-flight="Boolean(current?.inFlight)"
              :interrupt-available="Boolean(current?.inFlight?.roundId && input.draft.value.trim())"
              :available-commands="availableCommands"
              :control-disabled="controls.busy.value"
              :interrupt-disabled="controls.busy.value || input.composing.value"
              :submit-disabled="composerSubmitDisabled"
              :presence-has-scene="Boolean(current?.scene) && !current?.needsRecovery"
              :presence-ready="Boolean(presence.snapshot.value)"
              :presence-loading="presence.loading.value"
              :presence-unavailable="presence.unavailable.value"
              :presence-characters="presence.snapshot.value?.characters ?? []"
              :selected-character-id="presence.selectedCharacterId.value"
              @submit="submitComposer"
              @keydown="onComposerKeydown"
              @composition-start="input.composing.value = true"
              @composition-end="input.composing.value = false"
              @choose-command="chooseSlashCommand"
              @select-recipient="presence.select"
              @refresh-presence="presence.refresh"
              @stop="controls.control('cancel')"
              @interrupt="interruptDraft()"
            />
            <div class="mt-3 flex flex-wrap gap-2">
              <button
                type="button"
                class="btn primary"
                v-if="current?.resumeRequired"
                :disabled="controls.busy.value"
                @click="controls.control('resume')"
              >
                {{ t("game.resume") }}
              </button>
              <button
                type="button"
                class="btn"
                v-if="current?.needsRecovery"
                :disabled="phase.connecting.value || records.state.value.recovering"
                @click="game.recover"
              >
                {{ t("common.reconnect") }}
              </button>
            </div>
          </div>
        </div>
      </template>
    </GameStage>
  </section>
</template>
