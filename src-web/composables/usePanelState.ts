import {
  computed,
  nextTick,
  onScopeDispose,
  readonly,
  ref,
  toValue,
  watch,
  type MaybeRefOrGetter,
} from "vue";
import { transitionPanel, type PanelEvent, type PanelState } from "../utils/ui-shell";

interface Options {
  pinned: MaybeRefOrGetter<boolean>;
  protectedFromClose: MaybeRefOrGetter<boolean>;
  savePinned(value: boolean): void;
  focusComposer(): void;
}

/** 管理面板的短时计时器与 DOM 生命周期；面板状态不依赖舞台几何。 */
export function usePanelState(options: Options) {
  const state = ref<PanelState>("collapsed");
  const triggerHovered = ref(false);
  const panelHovered = ref(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let initialized = false;
  let disposed = false;

  function clearTimer(): void {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  }

  function transition(event: PanelEvent): void {
    if (disposed) return;
    const result = transitionPanel(state.value, event, toValue(options.protectedFromClose));
    state.value = result.state;
    clearTimer();
    if (result.timer === "open") {
      timer = setTimeout(() => transition("openDelay"), 180);
    } else if (result.timer === "close" && !toValue(options.protectedFromClose)) {
      timer = setTimeout(() => transition("closeDelay"), 350);
    }
    if (event === "pin") options.savePinned(true);
    if (event === "unpin") options.savePinned(false);
    if (result.focusComposer) void nextTick(() => !disposed && options.focusComposer());
  }

  function enterTrigger(): void {
    triggerHovered.value = true;
    if (state.value === "hover") clearTimer();
    else transition("triggerEnter");
  }

  function leaveTrigger(): void {
    triggerHovered.value = false;
    if (state.value === "collapsed") clearTimer();
    else scheduleCloseIfOutside();
  }

  function enterPanel(): void {
    panelHovered.value = true;
    if (state.value === "hover") transition("panelEnter");
  }

  function leavePanel(): void {
    panelHovered.value = false;
    scheduleCloseIfOutside();
  }

  function scheduleCloseIfOutside(): void {
    if (state.value !== "hover" || triggerHovered.value || panelHovered.value) return;
    if (toValue(options.protectedFromClose)) {
      clearTimer();
      return;
    }
    transition("panelLeave");
  }

  watch(
    () => toValue(options.pinned),
    (pinned) => {
      if (!initialized) {
        initialized = true;
        state.value = pinned ? "pinned" : "collapsed";
      } else if (pinned && state.value !== "pinned") {
        state.value = "pinned";
        clearTimer();
      } else if (!pinned && state.value === "pinned") {
        state.value = "expanded";
      }
    },
    { immediate: true, flush: "sync" },
  );

  watch(
    () => toValue(options.protectedFromClose),
    (protectedFromClose) => {
      if (!protectedFromClose) scheduleCloseIfOutside();
      else if (timer !== undefined && state.value === "hover") clearTimer();
    },
    { flush: "sync" },
  );

  onScopeDispose(() => {
    disposed = true;
    clearTimer();
  });

  return {
    state: readonly(state),
    isOpen: computed(() => state.value !== "collapsed"),
    enterTrigger,
    leaveTrigger,
    enterPanel,
    leavePanel,
    open: () => transition("open"),
    interact: () => transition("interact"),
    close: () => transition("close"),
    escape: () => transition("escape"),
    pin: () => transition("pin"),
    unpin: () => transition("unpin"),
  };
}
