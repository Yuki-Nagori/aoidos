<script setup lang="ts">
import { ref, toRef } from "vue";
import ActionComposer from "./ActionComposer.vue";
import GameStage from "./GameStage.vue";
import RecordFeed from "./RecordFeed.vue";
import type { OpenedSession } from "../../../api/engine";
import type { useUiPreferences } from "../composables/useUiPreferences";
import { useGameWorkspace } from "../composables/useGameWorkspace";

const props = defineProps<{
  session: OpenedSession;
  productBusy: boolean;
  preferences: ReturnType<typeof useUiPreferences>;
}>();
const emit = defineEmits<{
  error: [unknown];
  historyRevision: [number];
  openSettings: [];
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
  phaseName, reconnect,
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
defineExpose({ reconnect });
</script>
<template>
  <section class="contents">
    <GameStage
      :panel-state="panel.state.value"
      :panel-open="panel.isOpen.value"
      :modal-open="Boolean(confirmation)"
      :title="session.title"
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
      @settings="emit('openSettings')"
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
          <h2>{{ session.title }}</h2>
          <p>{{ current?.needsRecovery ? t("game.paused") : t("game.scenePrompt") }}</p>
        </div>
      </template>
      <template #panel-header>
        <h2 id="dialogue-panel-heading" class="truncate text-sm font-semibold">
          {{ t("game.dialogue") }}
        </h2>
        <button type="button" class="ml-auto text-xs text-muted" @click="jumpToLatest">
          {{ t("game.latest") }}
        </button>
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
            <div class="mb-2 flex flex-wrap items-center gap-2 text-xs text-muted">
              <label class="flex items-center gap-2">
                {{ t("game.diceMode") }}
                <select
                  :value="preferences.diceMode.value"
                  :disabled="preferences.saving.value"
                  @change="preferences.setDiceMode(
                    ($event.target as HTMLSelectElement).value as 'manual' | 'auto',
                  )"
                >
                  <option value="manual">{{ t("game.manual") }}</option>
                  <option value="auto">{{ t("game.automatic") }}</option>
                </select>
              </label>
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
              :preview="composerPreview"
              :submit-label="composerSubmitLabel"
              :too-large="input.tooLarge.value"
              :slash-menu-dismissed="slashMenuDismissed"
              :rewind-selector-open="rewindSelectorOpen"
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
            />
            <div class="mt-3 flex flex-wrap gap-2">
              <button
                type="button"
                v-if="current?.inFlight"
                :disabled="controls.busy.value"
                @click="controls.control('cancel')"
              >
                {{ t("game.stopTurn") }}
              </button>
              <button
                type="button"
                v-if="current?.inFlight?.roundId && input.draft.value.trim()"
                :disabled="controls.busy.value || input.composing.value"
                @click="interruptDraft()"
              >
                {{ t("game.interrupt") }}
              </button>
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
