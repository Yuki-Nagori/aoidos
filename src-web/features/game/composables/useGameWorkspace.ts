import { computed, nextTick, onMounted, onScopeDispose, ref, toValue, watch } from "vue";
import type { Ref } from "vue";
import type { MaybeRefOrGetter } from "vue";
import { useI18n } from "vue-i18n";
import type { OpenedSession } from "../../../api/engine";
import { useGameView } from "./useGameView";
import { useGameInput } from "./useGameInput";
import { useScenePresence } from "./useScenePresence";
import { useSceneHint } from "./useSceneHint";
import { useGameControls } from "./useGameControls";
import type { useUiPreferences } from "./useUiPreferences";
import { usePanelState } from "./usePanelState";
import { canSubmitComposer, composerRecipient, previewComposerInput } from "../utils/ui-shell";
import { submitInput } from "../../../api/engine";

type ComposerFocus = { focus(): void };

type Options = {
  session: MaybeRefOrGetter<OpenedSession>;
  productBusy: MaybeRefOrGetter<boolean>;
  preferences: MaybeRefOrGetter<ReturnType<typeof useUiPreferences>>;
  composer: Ref<ComposerFocus | undefined>;
  recordsFeed: Ref<HTMLElement | undefined>;
  confirmDialog: Ref<HTMLDialogElement | undefined>;
  emitError(error: unknown): void;
  emitHistoryRevision(revision: number): void;
};

/** 管理活动对局的输入、记录跟随、阶段控制与恢复交互。 */
export function useGameWorkspace(ports: Options) {
  const session = computed(() => toValue(ports.session));
  const productBusy = computed(() => toValue(ports.productBusy));
  const preferences = computed(() => toValue(ports.preferences));
  const { t } = useI18n();
  const sessionId = computed(() => session.value.sessionId);
  const game = useGameView(sessionId);
  const { phase, turn, records } = game;
  const current = computed(() => phase.state.value.snapshot);
  watch(current, (snapshot) => ports.emitHistoryRevision(snapshot?.historyRevision ?? 0), {
    immediate: true,
  });
  const presence = useScenePresence(sessionId, current);
  const sceneHint = useSceneHint(sessionId, current);
  const input = useGameInput(sessionId, (id, text) => {
    const recipient = composerRecipient(
      previewComposerInput(text).mode,
      presence.selectedCharacterId.value,
      presence.snapshot.value?.presenceRevision,
    );
    return submitInput(
      id,
      text,
      recipient.recipientCharacterId,
      recipient.expectedPresenceRevision,
    );
  });
  watch(input.error, (failure) => {
    if (
      failure?.detail?.reason === "stalePresenceRevision" ||
      failure?.detail?.reason === "targetUnavailable"
    ) {
      presence.refresh();
    }
  });
  const controls = useGameControls(current);

  const { composer, recordsFeed, confirmDialog } = ports;
  const loadingOlder = ref(false);
  const unreadRecords = ref(false);
  let followLatest = true;
  const readingDrag = ref(false);
  const panelProtected = ref(false);
  const confirmation = ref<{ kind: "regenerate" } | { kind: "rewind"; seq: number } | undefined>();
  const rewindSelectorOpen = ref(false);
  const slashMenuDismissed = ref(false);
  const panel = usePanelState({
    pinned: preferences.value.panelPinned,
    protectedFromClose: computed(() => panelProtected.value || input.composing.value),
    savePinned: (value) => void preferences.value.setPanelPinned(value),
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
        productBusy.value ||
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
  const connectionError = computed(
    () =>
      phase.connectionError.value ?? records.connectionError.value ?? turn.connectionError.value,
  );
  const error = computed(
    () =>
      input.error.value ??
      controls.error.value ??
      current.value?.lastOperation?.error ??
      turn.view.value.snapshot?.error ??
      phase.state.value.error ??
      records.state.value.error ??
      turn.view.value.recoveryError ??
      connectionError.value,
  );
  const scenePath = computed(() =>
    current.value?.scene?.path.map(({ title }) => title).join(" · "),
  );
  const olderCursor = computed(() =>
    records.state.value.pages.length
      ? records.state.value.pages.at(-1)?.nextCursor
      : records.state.value.view?.nextCursor,
  );
  let focusBeforeConfirmation: HTMLElement | null = null;
  let feedResizeObserver: ResizeObserver | undefined;
  let followFrame = 0;
  function hasTextSelection(): boolean {
    return Boolean(window.getSelection()?.toString());
  }
  function readingIsProtected(): boolean {
    return hasTextSelection() || records.state.value.body !== undefined || readingDrag.value;
  }
  function canAutoFollow(): boolean {
    return followLatest && panel.isOpen.value && !readingIsProtected();
  }
  function scheduleFollowLatest(feed = recordsFeed.value): void {
    if (!feed || !canAutoFollow() || typeof requestAnimationFrame === "undefined") return;
    if (followFrame) cancelAnimationFrame(followFrame);
    followFrame = requestAnimationFrame(() => {
      followFrame = 0;
      if (feed === recordsFeed.value && canAutoFollow()) feed.scrollTop = feed.scrollHeight;
    });
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
    feedResizeObserver?.disconnect();
    if (followFrame) cancelAnimationFrame(followFrame);
  });
  watch(
    recordsFeed,
    (feed) => {
      feedResizeObserver?.disconnect();
      if (!feed || typeof ResizeObserver === "undefined") return;
      feedResizeObserver = new ResizeObserver(() => scheduleFollowLatest(feed));
      feedResizeObserver.observe(feed);
    },
    { flush: "post", immediate: true },
  );
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
    if (event.key === "Escape" && rewindSelectorOpen.value) {
      event.preventDefault();
      rewindSelectorOpen.value = false;
      return;
    }
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
  function requestRegenerate(): void {
    if (
      current.value?.phase !== "idle" ||
      current.value.needsRecovery ||
      !current.value.lastOperation?.roundId
    ) {
      return;
    }
    confirmation.value = { kind: "regenerate" };
  }

  async function submitComposer(): Promise<void> {
    if (composerSubmitDisabled.value || input.composing.value || input.tooLarge.value) return;
    const raw = input.draft.value;
    const preview = composerPreview.value;
    if (preview.slashActive || preview.invalidSlash || preview.contentRequired) return;
    if (!preview.command) {
      followLatest = true;
      unreadRecords.value = false;
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
      requestRegenerate();
      return;
    } else if (name === "rewind") {
      rewindSelectorOpen.value = true;
      accepted = true;
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
    if (!confirmation.value) return;
    const selected = confirmation.value;
    const accepted =
      selected.kind === "regenerate"
        ? await controls.control("regenerate")
        : await controls.control("rewind", selected.seq);
    if (accepted) {
      confirmation.value = undefined;
      rewindSelectorOpen.value = false;
    }
  }
  function selectRewindTarget(seq: number): void {
    if (!rewindSelectorOpen.value) return;
    rewindSelectorOpen.value = false;
    confirmation.value = { kind: "rewind", seq };
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
  const preview = computed(() =>
    turn.view.value.snapshot?.turnId === game.latestPublicRecord.value?.turnId
      ? undefined
      : turn.view.value.snapshot,
  );
  const activeTurnId = computed(() => turn.view.value.snapshot?.turnId);
  watch(
    () => {
      const view = records.state.value.view;
      return [sessionId.value, view?.sessionId, view?.viewEpoch, view?.lastRecordSeq] as const;
    },
    (currentView, previousView) => {
      const shouldFollow = canAutoFollow();
      void nextTick(() => {
        const feed = recordsFeed.value;
        if (!feed) return;
        if (shouldFollow) scheduleFollowLatest(feed);
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
      const shouldFollow = canAutoFollow();
      void nextTick(() => {
        const feed = recordsFeed.value;
        if (feed && shouldFollow) scheduleFollowLatest(feed);
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

  watch(error, (value) => ports.emitError(value), { immediate: true });

  return {
    session,
    t,
    game,
    preferences,
    phase,
    connectionFailed: computed(() => connectionError.value !== undefined),
    records,
    current,
    presence,
    sceneHint,
    input,
    controls,
    loadingOlder,
    unreadRecords,
    readingDrag,
    panelProtected,
    confirmation,
    rewindSelectorOpen,
    slashMenuDismissed,
    panel,
    orderedRecords,
    composerPreview,
    composerSubmitDisabled,
    composerSubmitLabel,
    scenePath,
    olderCursor,
    preview,
    activeTurnId,
    hasTextSelection,
    updateFollowState,
    onReadingPointerDown,
    jumpToLatest,
    onComposerKeydown,
    interruptDraft,
    submitComposer,
    confirmOperation,
    requestRegenerate,
    selectRewindTarget,
    chooseSlashCommand,
    closePanel,
    loadOlder,
    phaseName,
    reconnect: game.reconnect,
  };
}
