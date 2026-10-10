import { effectScope, ref } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";
import { usePanelState } from "../../../src-web/composables/usePanelState";

describe("usePanelState", () => {
  afterEach(() => vi.useRealTimers());

  it("opens after the protected hover delay and closes after leaving both targets", () => {
    vi.useFakeTimers();
    const scope = effectScope();
    const pinned = ref(false);
    const state = scope.run(() =>
      usePanelState({
        pinned,
        protectedFromClose: ref(false),
        savePinned: (value) => (pinned.value = value),
        focusComposer: vi.fn(),
      }),
    )!;

    state.enterTrigger();
    vi.advanceTimersByTime(179);
    expect(state.state.value).toBe("collapsed");
    vi.advanceTimersByTime(1);
    expect(state.state.value).toBe("hover");
    state.leaveTrigger();
    vi.advanceTimersByTime(349);
    expect(state.isOpen.value).toBe(true);
    vi.advanceTimersByTime(1);
    expect(state.state.value).toBe("collapsed");
    scope.stop();
  });

  it("keeps focused panels open and persists pin state", () => {
    const scope = effectScope();
    const pinned = ref(false);
    const protectedFromClose = ref(false);
    const state = scope.run(() =>
      usePanelState({
        pinned,
        protectedFromClose,
        savePinned: (value) => (pinned.value = value),
        focusComposer: vi.fn(),
      }),
    )!;

    state.open();
    state.pin();
    expect(state.state.value).toBe("pinned");
    expect(pinned.value).toBe(true);
    state.escape();
    expect(state.state.value).toBe("pinned");
    state.unpin();
    expect(state.state.value).toBe("expanded");
    expect(pinned.value).toBe(false);
    scope.stop();
  });

  it("restarts the close delay only after hover protection is released", () => {
    vi.useFakeTimers();
    const scope = effectScope();
    const protectedFromClose = ref(true);
    const state = scope.run(() =>
      usePanelState({
        pinned: ref(false),
        protectedFromClose,
        savePinned: vi.fn(),
        focusComposer: vi.fn(),
      }),
    )!;

    state.enterTrigger();
    vi.advanceTimersByTime(180);
    state.leaveTrigger();
    vi.advanceTimersByTime(500);
    expect(state.state.value).toBe("hover");
    protectedFromClose.value = false;
    vi.advanceTimersByTime(349);
    expect(state.state.value).toBe("hover");
    vi.advanceTimersByTime(1);
    expect(state.state.value).toBe("collapsed");
    scope.stop();
  });
});

it("cancels hover timers when moving between the edge and the panel", async () => {
  vi.useFakeTimers();
  const scope = effectScope();
  const focusComposer = vi.fn();
  const protection = ref(false);
  const state = scope.run(() =>
    usePanelState({
      pinned: false,
      protectedFromClose: protection,
      savePinned: vi.fn(),
      focusComposer,
    }),
  )!;
  state.enterTrigger();
  state.leaveTrigger();
  vi.advanceTimersByTime(180);
  expect(state.state.value).toBe("collapsed");
  state.enterPanel();
  state.leavePanel();
  state.enterTrigger();
  vi.advanceTimersByTime(180);
  state.enterPanel();
  state.leaveTrigger();
  vi.advanceTimersByTime(350);
  expect(state.state.value).toBe("hover");
  state.leavePanel();
  vi.advanceTimersByTime(100);
  state.enterTrigger();
  vi.advanceTimersByTime(350);
  expect(state.state.value).toBe("hover");
  state.leaveTrigger();
  protection.value = true;
  vi.advanceTimersByTime(350);
  expect(state.state.value).toBe("hover");
  protection.value = false;
  state.interact();
  expect(state.state.value).toBe("expanded");
  state.close();
  state.open();
  await Promise.resolve();
  expect(focusComposer).toHaveBeenCalledOnce();
  scope.stop();
  state.open();
  expect(focusComposer).toHaveBeenCalledOnce();
  vi.useRealTimers();
});

it("restores saved pinning and responds to persisted preference changes", () => {
  const scope = effectScope();
  const pinned = ref(true);
  const state = scope.run(() =>
    usePanelState({
      pinned,
      protectedFromClose: false,
      savePinned: vi.fn(),
      focusComposer: vi.fn(),
    }),
  )!;
  expect(state.state.value).toBe("pinned");
  pinned.value = false;
  expect(state.state.value).toBe("expanded");
  pinned.value = true;
  expect(state.state.value).toBe("pinned");
  state.escape();
  expect(state.state.value).toBe("pinned");
  scope.stop();
});

it("releases pending hover and focus effects on disposal", async () => {
  vi.useFakeTimers();
  const scope = effectScope();
  const focusComposer = vi.fn();
  const protection = ref(false);
  const state = scope.run(() =>
    usePanelState({
      pinned: false,
      protectedFromClose: protection,
      savePinned: vi.fn(),
      focusComposer,
    }),
  )!;
  protection.value = true;
  state.enterTrigger();
  scope.stop();
  vi.advanceTimersByTime(1000);
  expect(state.state.value).toBe("collapsed");
  const otherScope = effectScope();
  const other = otherScope.run(() =>
    usePanelState({ pinned: false, protectedFromClose: false, savePinned: vi.fn(), focusComposer }),
  )!;
  other.open();
  otherScope.stop();
  await Promise.resolve();
  expect(focusComposer).not.toHaveBeenCalled();
  vi.useRealTimers();
});
