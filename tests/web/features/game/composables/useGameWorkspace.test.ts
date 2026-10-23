import { mount } from "@vue/test-utils";
import { defineComponent, nextTick, ref } from "vue";
import type { Ref } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenedSession } from "../../../../../src-web/api/engine";
import { useGameControls } from "../../../../../src-web/features/game/composables/useGameControls";
import { useGameInput } from "../../../../../src-web/features/game/composables/useGameInput";
import { useGameView } from "../../../../../src-web/features/game/composables/useGameView";
import { usePanelState } from "../../../../../src-web/features/game/composables/usePanelState";
import { useSceneHint } from "../../../../../src-web/features/game/composables/useSceneHint";
import { useScenePresence } from "../../../../../src-web/features/game/composables/useScenePresence";
import type { useUiPreferences } from "../../../../../src-web/features/game/composables/useUiPreferences";
import { useGameWorkspace } from "../../../../../src-web/features/game/composables/useGameWorkspace";
import { i18n } from "../../../../../src-web/i18n/index";

vi.mock("../../../../../src-web/features/game/composables/useGameView", () => ({
  useGameView: vi.fn(),
}));
vi.mock("../../../../../src-web/features/game/composables/useGameInput", () => ({
  useGameInput: vi.fn(),
}));
vi.mock("../../../../../src-web/features/game/composables/useGameControls", () => ({
  useGameControls: vi.fn(),
}));
vi.mock("../../../../../src-web/features/game/composables/useScenePresence", () => ({
  useScenePresence: vi.fn(),
}));
vi.mock("../../../../../src-web/features/game/composables/useSceneHint", () => ({
  useSceneHint: vi.fn(),
}));
vi.mock("../../../../../src-web/features/game/composables/usePanelState", () => ({
  usePanelState: vi.fn(),
}));

let phaseState: Ref<{ snapshot: { historyRevision: number; phase: string }; error: undefined }>;
let turnView: Ref<{
  snapshot:
    | {
        turnId: string;
        text: string;
        seq: { chunk: number; done: number; failed: number };
        outcome?: "completed";
      }
    | undefined;
  recoveryError: undefined;
}>;
let panelOpen: Ref<boolean>;
const panelOptions = vi.mocked(usePanelState);
let workspace: ReturnType<typeof useGameWorkspace>;
const composerFocus = vi.fn<() => void>();
let ports: Parameters<typeof useGameWorkspace>[0];
let draft: Ref<string>;
let send: ReturnType<typeof vi.fn>;

describe("useGameWorkspace", () => {
  beforeEach(() => {
    phaseState = ref({ snapshot: { historyRevision: 4, phase: "idle" }, error: undefined });
    const phase = {
      state: phaseState,
      connectionError: ref(undefined),
    };
    turnView = ref({ snapshot: undefined, recoveryError: undefined });
    const records = {
      items: ref([]),
      state: ref({ pages: [], view: undefined, body: undefined, error: undefined }),
      connectionError: ref(undefined),
      clearBody: vi.fn(),
      loadPage: vi.fn(),
    };
    composerFocus.mockReset();
    const composer = ref<{ focus(): void }>({ focus: composerFocus });
    const feed = document.createElement("div");
    Object.defineProperties(feed, {
      scrollHeight: { value: 100, configurable: true },
      clientHeight: { value: 40, configurable: true },
    });
    feed.scrollTo = vi.fn();
    const dialog = document.createElement("dialog");
    dialog.showModal = vi.fn(() => {
      dialog.open = true;
    });
    dialog.close = vi.fn(() => {
      dialog.open = false;
    });
    const recordsFeed = ref(feed);
    const confirmDialog = ref(dialog);
    panelOpen = ref(false);
    const panel = {
      state: ref("closed"),
      isOpen: panelOpen,
      enterTrigger: vi.fn(),
      leaveTrigger: vi.fn(),
      enterPanel: vi.fn(),
      leavePanel: vi.fn(),
      open: vi.fn(),
      interact: vi.fn(),
      escape: vi.fn(),
      pin: vi.fn(),
      unpin: vi.fn(),
      close: vi.fn(),
    };
    panelOptions.mockReturnValue(panel as never);
    vi.mocked(useGameView).mockReturnValue({
      phase,
      turn: {
        view: turnView,
        connectionError: ref(undefined),
      },
      records,
      latestPublicRecord: ref(undefined),
      reconnect: vi.fn(),
    } as never);
    draft = ref("");
    send = vi.fn();
    vi.mocked(useGameInput).mockReturnValue({
      error: ref(undefined),
      composing: ref(false),
      draft,
      busy: ref(false),
      tooLarge: ref(false),
      captureDraft: vi.fn(),
      clearDraftIfCurrent: vi.fn(),
      send,
    } as never);
    vi.mocked(useGameControls).mockReturnValue({
      busy: ref(false),
      error: ref(undefined),
      control: vi.fn().mockResolvedValue(true),
    } as never);
    vi.mocked(useScenePresence).mockReturnValue({
      selectedCharacterId: ref(undefined),
      snapshot: ref(undefined),
      refresh: vi.fn(),
    } as never);
    vi.mocked(useSceneHint).mockReturnValue({
      loading: ref(false),
      hints: ref([]),
      unavailable: ref(false),
      failed: ref(false),
      request: vi.fn(),
    } as never);
    ports = {
      session: { sessionId: "session-1" } as OpenedSession,
      productBusy: ref(false),
      preferences: { panelPinned: false, setPanelPinned: vi.fn() } as unknown as ReturnType<
        typeof useUiPreferences
      >,
      composer,
      recordsFeed,
      confirmDialog,
      emitError: vi.fn(),
      emitHistoryRevision: vi.fn(),
    };
    const Host = defineComponent({
      setup() {
        workspace = useGameWorkspace(ports);
        return () => null;
      },
    });
    mount(Host, { global: { plugins: [i18n] } });
    expect((ports.composer.value as { focus: () => void }).focus).toBe(composerFocus);
  });

  it("forwards history revisions and uses the supplied DOM references", async () => {
    expect(ports.emitHistoryRevision).toHaveBeenCalledWith(4);
    phaseState.value.snapshot = { historyRevision: 5, phase: "idle" };
    await nextTick();
    expect(ports.emitHistoryRevision).toHaveBeenLastCalledWith(5);

    panelOptions.mock.calls[0]?.[0].focusComposer();
    workspace.jumpToLatest();
    expect(composerFocus).toHaveBeenCalledOnce();
    expect(ports.recordsFeed.value?.scrollTo).toHaveBeenCalledOnce();

    workspace.confirmation.value = { kind: "regenerate" };
    await nextTick();
    await nextTick();
    expect(ports.confirmDialog.value?.showModal).toHaveBeenCalledOnce();
  });

  it("blocks Enter submission while product initialization is pending", () => {
    draft.value = "Look around.";
    (ports.productBusy as Ref<boolean>).value = true;
    const event = new KeyboardEvent("keydown", { key: "Enter" });
    const preventDefault = vi.spyOn(event, "preventDefault");

    workspace.onComposerKeydown(event);

    expect(preventDefault).toHaveBeenCalledOnce();
    expect(send).not.toHaveBeenCalled();
  });

  it("starts following the newest narration after the player submits an action", async () => {
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      callback(0);
      return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
    panelOpen.value = true;
    const feed = ports.recordsFeed.value!;
    feed.scrollTop = 0;
    workspace.updateFollowState();
    draft.value = "Look around.";

    await workspace.submitComposer();
    turnView.value.snapshot = {
      turnId: "turn-1",
      text: "The room answers.",
      seq: { chunk: 1, done: 0, failed: 0 },
    };
    await nextTick();
    await nextTick();

    expect(send).toHaveBeenCalledOnce();
    expect(feed.scrollTop).toBe(feed.scrollHeight);
    vi.unstubAllGlobals();
  });
});
