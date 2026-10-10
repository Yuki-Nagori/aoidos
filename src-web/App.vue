<script setup lang="ts">
import { computed, nextTick, onMounted, onScopeDispose, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import GameStage from "./components/GameStage.vue";
import { useProduct } from "./composables/useProduct";
import { useGameView } from "./composables/useGameView";
import { useGameInput } from "./composables/useGameInput";
import { useGameControls } from "./composables/useGameControls";
import { useTheme } from "./composables/useTheme";
import { useLocale } from "./composables/useLocale";
import { useUiPreferences } from "./composables/useUiPreferences";
import { usePanelState } from "./composables/usePanelState";
import { canSubmitComposer, previewComposerInput } from "./utils/ui-shell";
import RecordFeed from "./components/RecordFeed.vue";
import ActionComposer from "./components/ActionComposer.vue";

const { t } = useI18n();
const product = useProduct();
const theme = useTheme(product.scriptId);
const locale = useLocale();
const sessionId = computed(() => product.session.value?.sessionId);
const game = useGameView(sessionId);
const { phase, turn, records } = game;
const current = computed(() => phase.state.value.snapshot);
const input = useGameInput(sessionId);
const controls = useGameControls(current);
const preferences = useUiPreferences();
const composer = ref<InstanceType<typeof ActionComposer>>();
const recordsFeed = ref<HTMLElement>();
const confirmDialog = ref<HTMLDialogElement>();
const loadingOlder = ref(false);
const unreadRecords = ref(false);
let followLatest = true;
const readingDrag = ref(false);
const panelProtected = ref(false);
const confirmation = ref<{ kind: "regenerate" } | { kind: "rewind"; seq: number }>();
const rewindSelectorOpen = ref(false);
const slashMenuDismissed = ref(false);
const panel = usePanelState({
  pinned: preferences.panelPinned,
  protectedFromClose: computed(() => panelProtected.value || input.composing.value),
  savePinned: (value) => void preferences.setPanelPinned(value),
  focusComposer: () => composer.value?.focus(),
});
const orderedRecords = computed(() => [...records.items.value].reverse());
const composerPreview = computed(() => previewComposerInput(input.draft.value));
const composerSubmitDisabled = computed(
  () =>
    !canSubmitComposer(
      composerPreview.value,
      current.value
        ? {
            phase: current.value.phase,
            resumeRequired: current.value.resumeRequired,
            needsRecovery: current.value.needsRecovery,
            hasInFlight: Boolean(current.value.inFlight?.roundId),
            hasLastOperation: Boolean(current.value.lastOperation?.roundId),
          }
        : undefined,
      product.busy.value ||
        input.busy.value ||
        input.tooLarge.value ||
        input.composing.value ||
        controls.busy.value,
    ),
);
const composerSubmitLabel = computed(() => {
  switch (composerPreview.value.command?.name) {
    case "interrupt":
      return t("game.interrupt");
    case "stop":
      return t("game.stopTurn");
    case "resume":
      return t("game.resume");
    case "regenerate":
      return t("game.regenerate");
    case "rewind":
      return t("game.chooseRewindTarget");
    case "latest":
      return t("game.latest");
    case "pin":
      return t("game.pinPanel");
    default:
      return t("game.submitAction");
  }
});
const scenePath = computed(() => current.value?.scene?.path.map(({ title }) => title).join(" · "));
const olderCursor = computed(() =>
  records.state.value.pages.length
    ? records.state.value.pages.at(-1)?.nextCursor
    : records.state.value.view?.nextCursor,
);
let focusBeforeConfirmation: HTMLElement | null = null;
function hasTextSelection(): boolean {
  return Boolean(window.getSelection()?.toString());
}
function readingIsProtected(): boolean {
  return hasTextSelection() || records.state.value.body !== undefined || readingDrag.value;
}
function canAutoFollow(): boolean {
  return (
    followLatest &&
    panel.isOpen.value &&
    document.hasFocus() &&
    document.visibilityState === "visible" &&
    !readingIsProtected()
  );
}
function updateFollowState(): void {
  const feed = recordsFeed.value;
  if (!feed) return;
  followLatest =
    feed.scrollHeight - feed.scrollTop - feed.clientHeight <= 24 && !readingIsProtected();
  if (followLatest) unreadRecords.value = false;
}
function onReadingPointerDown(): void {
  readingDrag.value = true;
}
function onReadingPointerEnd(): void {
  readingDrag.value = false;
  updateFollowState();
}
onMounted(() => {
  window.addEventListener("pointerup", onReadingPointerEnd);
  window.addEventListener("pointercancel", onReadingPointerEnd);
  window.addEventListener("blur", onReadingPointerEnd);
});
onScopeDispose(() => {
  window.removeEventListener("pointerup", onReadingPointerEnd);
  window.removeEventListener("pointercancel", onReadingPointerEnd);
  window.removeEventListener("blur", onReadingPointerEnd);
});
function jumpToLatest(): void {
  const feed = recordsFeed.value;
  if (!feed) return;
  records.clearBody();
  followLatest = true;
  unreadRecords.value = false;
  const reduceMotion = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
  feed.scrollTo({ top: feed.scrollHeight, behavior: reduceMotion ? "auto" : "smooth" });
}
function onComposerKeydown(event: KeyboardEvent): void {
  if (event.isComposing || input.composing.value) return;
  if (event.key === "Escape" && composerPreview.value.slashActive && !slashMenuDismissed.value) {
    event.preventDefault();
    event.stopPropagation();
    slashMenuDismissed.value = true;
    return;
  }
  if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    void submitComposer();
  }
}
async function interruptDraft(raw = input.draft.value): Promise<void> {
  const command = previewComposerInput(raw).command;
  const text = command?.name === "interrupt" ? command.argument : raw;
  if (!text.trim() || input.tooLarge.value) return;
  const ticket = input.captureDraft(raw);
  const accepted = await controls.control("interrupt", text);
  if (accepted) input.clearDraftIfCurrent(ticket);
}
async function submitComposer(): Promise<void> {
  if (input.composing.value || input.tooLarge.value) return;
  const raw = input.draft.value;
  const preview = composerPreview.value;
  if (preview.slashActive || preview.invalidSlash || preview.contentRequired) return;
  if (!preview.command) {
    await input.send();
    return;
  }
  const { name, argument } = preview.command;
  const ticket = input.captureDraft(raw);
  const argumentRequired = name === "interrupt";
  if (argumentRequired && !argument.trim()) return;
  if (!argumentRequired && argument.trim()) return;
  let accepted = false;
  if (name === "interrupt") accepted = await controls.control("interrupt", argument);
  else if (name === "stop") accepted = await controls.control("cancel");
  else if (name === "resume") accepted = await controls.control("resume");
  else if (name === "regenerate") {
    if (
      current.value?.phase !== "idle" ||
      current.value.needsRecovery ||
      !current.value.lastOperation?.roundId
    )
      return;
    confirmation.value = { kind: "regenerate" };
    return;
  } else if (name === "rewind") {
    rewindSelectorOpen.value = true;
    return;
  } else if (name === "pin") {
    panel.pin();
    accepted = true;
  } else if (name === "latest") {
    jumpToLatest();
    accepted = true;
  }
  if (accepted) input.clearDraftIfCurrent(ticket);
}
async function confirmOperation(): Promise<void> {
  const operation = confirmation.value;
  if (!operation) return;
  const accepted =
    operation.kind === "regenerate"
      ? await controls.control("regenerate")
      : await controls.control("rewind", operation.seq);
  if (accepted) confirmation.value = undefined;
}
function chooseSlashCommand(command: string): void {
  if (input.composing.value) return;
  input.draft.value = `/${command} `;
  void nextTick(() => composer.value?.focus());
}
function closePanel(): void {
  if (panel.state.value === "pinned") panel.unpin();
  panel.close();
}
async function loadOlder(): Promise<void> {
  if (!olderCursor.value || loadingOlder.value) return;
  loadingOlder.value = true;
  const feed = recordsFeed.value;
  const previousHeight = feed?.scrollHeight ?? 0;
  try {
    await records.loadPage(olderCursor.value);
    await nextTick(() => {
      if (feed) feed.scrollTop += feed.scrollHeight - previousHeight;
    });
  } finally {
    loadingOlder.value = false;
  }
}
const error = computed(
  () =>
    product.error.value ??
    input.error.value ??
    controls.error.value ??
    current.value?.lastOperation?.error ??
    turn.view.value.snapshot?.error ??
    phase.state.value.error ??
    records.state.value.error ??
    turn.view.value.recoveryError ??
    phase.connectionError.value ??
    records.connectionError.value ??
    turn.connectionError.value,
);
const generation = computed(() => {
  const record = game.latestPublicRecord.value;
  const body = record?.body;
  if (body && "outcome" in body)
    return {
      outcome: body.outcome,
      finishReason: "finishReason" in body ? body.finishReason : undefined,
    };
  return record?.outcome ? { outcome: record.outcome } : undefined;
});
const preview = computed(() =>
  turn.view.value.snapshot?.turnId === game.latestPublicRecord.value?.turnId
    ? undefined
    : turn.view.value.snapshot,
);
const selectedScript = computed(() =>
  product.scripts.value.find((script) => script.scriptId === product.scriptId.value),
);
const errorMessage = computed(() => {
  if (!error.value) return undefined;
  switch (error.value.code) {
    case "app.not-ready":
      return t("errors.appNotReady");
    case "app.not-found":
      return t("errors.appNotFound");
    case "app.bad-request":
      return t("errors.badRequest");
    case "store.corrupt":
      return t("errors.storeCorrupt");
    case "store.io":
      return t("errors.storeIo");
    case "app.event-failed":
      return t("errors.appEventFailed");
    case "app.busy":
      return t("errors.appBusy");
    default:
      return t("errors.unknownWithCode", { code: error.value.code });
  }
});
const localeNotice = computed(() => {
  if (locale.notice.value === "pending") return t("settings.languagePending");
  if (locale.notice.value === "saveFailed") return t("settings.languageSaveFailed");
  if (locale.notice.value === "loadFailed") return t("settings.languageLoadFailed");
  if (locale.fallback.value) return t("settings.languageFallback");
  return undefined;
});
const themeErrorMessage = computed(() => {
  switch (theme.themeError.value) {
    case "storageUnavailable":
      return t("settings.themeErrors.storageUnavailable");
    case "invalidPreference":
      return t("settings.themeErrors.invalidPreference");
    case "themeUnavailable":
      return t("settings.themeErrors.themeUnavailable");
    case "saveFailed":
      return t("settings.themeErrors.saveFailed");
    case "readFailed":
      return t("settings.themeErrors.readFailed");
    case "catalogUnavailable":
      return t("settings.themeErrors.catalogUnavailable");
    default:
      return undefined;
  }
});
const skinErrorMessage = computed(() => {
  switch (theme.skinError.value) {
    case "invalidValue":
      return t("settings.skinErrors.invalidValue");
    case "unsupportedWebView":
      return t("settings.skinErrors.unsupportedWebView");
    case "applyFailed":
      return t("settings.skinErrors.applyFailed");
    case "readFailed":
      return t("settings.skinErrors.readFailed");
    default:
      return undefined;
  }
});
watch(
  () => {
    const view = records.state.value.view;
    return [sessionId.value, view?.sessionId, view?.viewEpoch, view?.lastRecordSeq] as const;
  },
  (currentView, previousView) => {
    void nextTick(() => {
      const feed = recordsFeed.value;
      if (!feed) return;
      if (canAutoFollow()) feed.scrollTop = feed.scrollHeight;
      else if (
        currentView[1] === currentView[0] &&
        currentView[1] === previousView[1] &&
        currentView[2] !== undefined &&
        currentView[2] === previousView[2] &&
        currentView[3] !== undefined &&
        previousView[3] !== undefined &&
        currentView[3] > previousView[3]
      )
        unreadRecords.value = true;
    });
  },
);
watch(
  () => [records.state.value.body, preview.value?.text] as const,
  () => {
    void nextTick(() => {
      const feed = recordsFeed.value;
      if (feed && canAutoFollow()) feed.scrollTop = feed.scrollHeight;
      else if (preview.value?.text) unreadRecords.value = true;
    });
  },
);
watch(
  () => [sessionId.value, records.state.value.view?.viewEpoch] as const,
  () => {
    followLatest = true;
    unreadRecords.value = false;
    loadingOlder.value = false;
    readingDrag.value = false;
    void nextTick(() => {
      const feed = recordsFeed.value;
      if (feed) feed.scrollTop = feed.scrollHeight;
    });
  },
  { flush: "sync" },
);
watch(
  () => input.draft.value,
  () => {
    slashMenuDismissed.value = false;
  },
);
watch(
  confirmation,
  async (value) => {
    await nextTick();
    const dialog = confirmDialog.value;
    if (!dialog) return;
    if (value && !dialog.open) {
      focusBeforeConfirmation =
        document.activeElement instanceof HTMLElement ? document.activeElement : null;
      dialog.showModal();
    } else if (!value && dialog.open) {
      dialog.close();
      focusBeforeConfirmation?.focus();
      focusBeforeConfirmation = null;
    }
  },
  { flush: "post" },
);
function themeName(id: string, fallback: string): string {
  switch (id) {
    case "dark":
      return t("settings.themeNames.dark");
    case "light":
      return t("settings.themeNames.light");
    case "light-purple":
      return t("settings.themeNames.lightPurple");
    default:
      return fallback;
  }
}
function phaseName(phase: string | undefined): string {
  switch (phase) {
    case "idle":
      return t("game.phases.idle");
    case "generating":
      return t("game.phases.generating");
    case "awaitingCheck":
      return t("game.phases.awaitingCheck");
    case "settling":
      return t("game.phases.settling");
    case "advancing":
      return t("game.phases.advancing");
    default:
      return t("common.unknownError");
  }
}
void product.load();
void preferences.load();
</script>

<template>
  <main class="mx-auto grid min-h-screen max-w-4xl gap-6 p-6 text-ink">
    <details class="glass-panel" :open="!product.session.value">
      <summary class="setup-summary">
        <h1 class="text-2xl font-semibold">{{ t("common.appName") }}</h1>
        <p class="text-sm text-muted">{{ t("game.intro") }}</p>
      </summary>
      <div class="flex gap-3">
        <label>
          {{ t("settings.theme") }}
          <select
            :value="theme.theme.value"
            :aria-label="t('settings.theme')"
            :disabled="theme.saving.value"
            @change="theme.setTheme(($event.target as HTMLSelectElement).value)"
          >
            <option v-for="option in theme.themes.value" :key="option.id" :value="option.id">
              {{ themeName(option.id, option.name) }}
            </option>
          </select>
        </label>
        <button v-if="product.scriptId.value === 'mistbell'" @click="theme.toggleSkin">
          {{ theme.disabled.value ? t("settings.enableSkin") : t("settings.disableSkin") }}
        </button>
      </div>
      <p v-if="themeErrorMessage" role="status">{{ themeErrorMessage }}</p>
      <p v-if="skinErrorMessage" role="status">{{ skinErrorMessage }}</p>
      <p v-else-if="theme.warningCount.value">
        {{ t("settings.skinWarnings", { count: theme.warningCount.value }) }}
      </p>
      <label>
        {{ t("settings.language") }}
        <select
          :value="locale.choice.value"
          :aria-label="t('settings.language')"
          :disabled="!locale.ready.value || locale.saving.value"
          @change="
            locale.setChoice(
              ($event.target as HTMLSelectElement).value as 'system' | 'zh-Hans' | 'en',
            )
          "
        >
          <option value="system">
            {{
              t("settings.systemLanguageCurrent", {
                locale: t(
                  locale.resolved.value === "zh-Hans" ? "settings.chinese" : "settings.english",
                ),
              })
            }}
          </option>
          <option value="zh-Hans">{{ t("settings.chinese") }}</option>
          <option value="en">{{ t("settings.english") }}</option>
        </select>
      </label>
      <p v-if="localeNotice" role="status">{{ localeNotice }}</p>
      <label>
        {{ t("settings.model") }}
        <select
          v-model="product.model.value"
          :aria-label="t('settings.model')"
          :disabled="product.busy.value"
        >
          <option v-for="model in product.models" :key="model.id" :value="model.id">
            {{ model.label }}
          </option>
        </select></label
      >
      <button :disabled="product.busy.value" @click="product.configureKey">
        {{ t("settings.setApiKey") }}
      </button>
      <p v-if="product.keyStatus.value">
        {{ product.keyStatus.value.set ? t("settings.keySet") : t("settings.keyMissing") }}
      </p>
      <label>
        {{ t("settings.script") }}
        <select v-model="product.scriptId.value" :disabled="product.busy.value">
          <option
            v-for="script in product.scripts.value"
            :key="script.scriptId"
            :value="script.scriptId"
          >
            {{ script.displayNames[locale.resolved.value] ?? script.title }}
          </option>
        </select></label
      >
      <div class="flex gap-4">
        <button :disabled="product.busy.value" @click="product.open(false)">
          {{ t("game.restart") }}</button
        ><button :disabled="product.busy.value" @click="product.open(true)">
          {{ t("game.newCycle") }}</button
        ><button @click="game.reconnect">{{ t("common.reconnect") }}</button>
      </div>
      <details v-if="selectedScript">
        <summary>{{ t("settings.credits") }}</summary>
        <pre class="whitespace-pre-wrap text-sm">{{ selectedScript.attributions }}</pre>
      </details>
      <p v-if="errorMessage" role="alert">{{ errorMessage }}</p>
    </details>
    <GameStage
      v-if="product.session.value"
      :panel-state="panel.state.value"
      :panel-open="panel.isOpen.value"
      :modal-open="Boolean(confirmation)"
      :title="product.session.value.title"
      :scene-label="t('game.scene')"
      :trigger-label="t('game.openPanel')"
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
    >
      <template #status>
        <span class="ml-auto truncate text-xs text-muted">
          {{ t("game.phase", { phase: current ? phaseName(current.phase) : t("common.loading") }) }}
        </span>
      </template>
      <template #scene>
        <div class="max-w-lg text-center">
          <div class="scene-star mb-5 text-5xl text-accent-violet" aria-hidden="true" />
          <h2 class="text-2xl font-semibold">{{ product.session.value.title }}</h2>
          <p v-if="scenePath" class="mt-2 text-sm text-muted">{{ scenePath }}</p>
          <p class="mt-3 text-sm text-muted">{{ t("game.scenePrompt") }}</p>
          <p v-if="current?.resumeRequired" class="mt-4 text-sm text-warning">
            {{ t("game.paused") }}
          </p>
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
        <div
          class="flex min-h-0 flex-1 flex-col"
          @focusin="panelProtected = true"
          @focusout="
            panelProtected = Boolean(
              ($event.relatedTarget as Node | null)?.parentElement?.closest('.dialogue-panel'),
            )
          "
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
          <button v-if="unreadRecords" class="jump-latest" @click="jumpToLatest">
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
              <button @click="confirmation = undefined">{{ t("common.cancel") }}</button>
              <button class="text-danger" :disabled="controls.busy.value" @click="confirmOperation">
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
                  @change="
                    preferences.setDiceMode(
                      ($event.target as HTMLSelectElement).value as 'manual' | 'auto',
                    )
                  "
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
                <button class="text-ink underline" @click="preferences.load">
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
              @submit="submitComposer"
              @keydown="onComposerKeydown"
              @composition-start="input.composing.value = true"
              @composition-end="input.composing.value = false"
              @choose-command="chooseSlashCommand"
            />
            <div class="mt-3 flex flex-wrap gap-2">
              <button
                v-if="current?.inFlight"
                :disabled="controls.busy.value"
                @click="controls.control('cancel')"
              >
                {{ t("game.stopTurn") }}
              </button>
              <button
                v-if="current?.inFlight?.roundId && input.draft.value.trim()"
                :disabled="controls.busy.value || input.composing.value"
                @click="interruptDraft()"
              >
                {{ t("game.interrupt") }}
              </button>
              <button
                v-if="current?.resumeRequired"
                :disabled="controls.busy.value"
                @click="controls.control('resume')"
              >
                {{ t("game.resume") }}
              </button>
              <button
                v-if="
                  current?.check?.status === 'waiting' &&
                  current.check.mode === 'manual' &&
                  current.inFlight?.roundId
                "
                :disabled="controls.busy.value"
                @click="controls.control('check')"
              >
                {{ t("game.confirmCheck", { expression: current.check.expression }) }}
              </button>
              <button
                v-if="current?.needsRecovery"
                :disabled="phase.connecting.value || records.state.value.recovering"
                @click="game.recover"
              >
                {{ t("common.reconnect") }}
              </button>
              <button
                v-if="
                  current?.phase === 'idle' &&
                  current.lastOperation?.roundId &&
                  !current.needsRecovery
                "
                @click="confirmation = { kind: 'regenerate' }"
              >
                {{ t("game.regenerate") }}
              </button>
            </div>
          </div>
        </div>
      </template>
    </GameStage>
  </main>
</template>
