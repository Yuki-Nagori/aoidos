<script setup lang="ts">
import { computed, ref, toRef, watch } from "vue";
import ActionComposer from "./ActionComposer.vue";
import GameStage from "./GameStage.vue";
import RecordFeed from "./RecordFeed.vue";
import type { OpenedSession } from "../../../api/engine";
import type { useUiPreferences } from "../composables/useUiPreferences";
import type { CommandSuggestion } from "../utils/ui-shell";
import { useGameWorkspace } from "../composables/useGameWorkspace";

const props = defineProps<{
  session: OpenedSession;
  scriptTitle: string;
  suggestedAction?: string;
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
  panelProtected, confirmation,
  rewindSelectorOpen, slashMenuDismissed, panel, orderedRecords, composerPreview,
  composerSubmitDisabled, composerSubmitLabel, scenePath, olderCursor, game, generation, preview,
  hasTextSelection, updateFollowState, onReadingPointerDown, jumpToLatest, onComposerKeydown,
  interruptDraft, submitComposer, confirmOperation, chooseSlashCommand, closePanel, loadOlder,
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
  if (
    current.value &&
    current.value.phase === "idle" &&
    !current.value.needsRecovery &&
    !current.value.resumeRequired &&
    !current.value.inFlight
  ) {
    commands.push("rewind");
  }
  return commands;
});
watch(
  [() => props.session.sessionId, () => props.suggestedAction],
  () => {
    if (!input.draft.value.trim() && props.suggestedAction) {
      input.draft.value = props.suggestedAction;
    }
  },
  { immediate: true, flush: "sync" },
);
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
        <!-- biome-ignore lint/a11y/noStaticElementInteractions: These handlers track text selection and reading state; this panel is not an activation target. -->
        <div
          class="flex min-h-0 flex-1 flex-col"
          @focusin="panelProtected = true"
          @focusout="panelProtected = Boolean(
            ($event.relatedTarget as Node | null)?.parentElement?.closest('.dialogue-panel'),
          )"
          @mouseup="panelProtected = hasTextSelection()"
          @keyup="panelProtected = hasTextSelection()"
          @pointerdown="onReadingPointerDown"
        >
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
              :items="orderedRecords"
              :older-cursor="olderCursor"
              :loading-older="loadingOlder"
              :preview="preview"
              :expanded-body="records.state.value.body"
              :generation="generation"
              :can-rewind="current?.phase === 'idle' && !current.needsRecovery"
              @load-older="loadOlder"
              @load-body="records.loadBody"
              @rewind="confirmation = { kind: 'rewind', seq: $event }"
            />
          </div>
          <p v-else role="status" class="m-3 rounded-lg bg-panel p-3 text-sm">
            {{ t("game.historyPending") }}
          </p>
          <button type="button" v-if="unreadRecords" class="jump-latest" @click="jumpToLatest">
            {{ t("game.newRecords") }}
          </button>
          <details v-if="panel.state.value !== 'hover' && current" class="work">
            <summary class="work-head mono">
              <span
                class="led"
                :class="current.needsRecovery || current.resumeRequired
                ? 'warn'
                : current.phase === 'idle'
                  ? ''
                  : 'active'"
                aria-hidden="true"
              />
              <span class="work-title">{{ t("game.workProcess") }} · {{
                  phaseName(current.phase)
                }}</span>
              <svg class="i sm work-chevron" viewBox="0 0 24 24" aria-hidden="true">
                <path d="m6 9 6 6 6-6" />
              </svg>
            </summary>
            <ul class="mono">
              <li>
                <span>{{ t("game.workPhase") }}:</span>
                <span class="work-value">{{ phaseName(current.phase) }}</span>
              </li>
              <li v-if="scenePath">
                <span>{{ t("game.workScene") }}:</span>
                <span class="work-value">{{ scenePath }}</span>
              </li>
              <li v-if="current.check">
                <span>{{ t("game.workCheck") }}:</span>
                <span class="work-value">
                  {{
                    current.check.status === "rolling"
                    ? t("game.workCheckRolling", { expression: current.check.expression })
                    : t("game.workCheckWaiting", { expression: current.check.expression })
                  }}
                </span>
              </li>
              <li v-if="current.lastOperation && current.lastOperation.outcome !== 'accepted'">
                <span>{{ t("game.workOperation") }}:</span>
                <span class="work-value">
                  {{
                    current.lastOperation.outcome === "completed"
                    ? t("game.outcomes.completed")
                    : current.lastOperation.outcome === "cancelled"
                      ? t("game.outcomes.cancelled")
                      : t("game.outcomes.failed")
                  }}
                </span>
              </li>
              <li v-if="current.lastOperation?.error">
                <span>{{
                  t("game.workErrorCode", { code: current.lastOperation.error.code })
                }}</span>
              </li>
            </ul>
          </details>
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
                confirmation?.kind === "regenerate"
                ? t("game.regenerateConfirm")
                : confirmation
                  ? t("game.rewindConfirm", { seq: confirmation.seq })
                  : ""
              }}
            </p>
            <div class="mt-3 flex justify-end gap-2">
              <button type="button" @click="confirmation = undefined">
                {{ t("common.cancel") }}
              </button>
              <button
                type="button"
                class="text-danger"
                :disabled="controls.busy.value"
                @click="confirmOperation"
              >
                {{ t("game.confirmDestructive") }}
              </button>
            </div>
          </dialog>
          <div v-if="panel.state.value !== 'hover'" class="border-t border-hairline p-3">
            <div class="game-composer-status text-xs text-muted">
              <span v-if="preferences.saveError.value" role="status">{{
                t("game.preferenceSaveFailed")
              }}</span>
              <span v-if="preferences.loadError.value" role="status">
                {{ t("game.preferenceLoadFailed") }}
                <button type="button" class="text-ink underline" @click="preferences.load">
                  {{ t("common.retry") }}
                </button>
              </span>
            </div>
            <ActionComposer
              ref="composer"
              v-model="input.draft.value"
              :suggested-action="props.suggestedAction"
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
                v-if="current?.resumeRequired"
                :disabled="controls.busy.value"
                @click="controls.control('resume')"
              >
                {{ t("game.resume") }}
              </button>
              <button
                type="button"
                v-if="current?.check?.status === 'waiting' &&
                current.check.mode === 'manual' &&
                current.inFlight?.roundId"
                :disabled="controls.busy.value"
                @click="controls.control('check')"
              >
                {{ t("game.confirmCheck", { expression: current.check.expression }) }}
              </button>
              <button
                type="button"
                v-if="current?.needsRecovery"
                :disabled="phase.connecting.value || records.state.value.recovering"
                @click="game.recover"
              >
                {{ t("common.reconnect") }}
              </button>
              <button
                type="button"
                v-if="current?.phase === 'idle' &&
                current.lastOperation?.roundId &&
                !current.needsRecovery"
                @click="confirmation = { kind: 'regenerate' }"
              >
                {{ t("game.regenerate") }}
              </button>
            </div>
          </div>
        </div>
      </template>
    </GameStage>
  </section>
</template>
