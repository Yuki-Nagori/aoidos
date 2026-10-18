import { mount } from "@vue/test-utils";
import { computed, ref, toValue } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenedSession } from "../../../../../src-web/api/engine";
import type { useUiPreferences } from "../../../../../src-web/features/game/composables/useUiPreferences";
import ActionComposer from "../../../../../src-web/features/game/components/ActionComposer.vue";
import GameWorkspace from "../../../../../src-web/features/game/components/GameWorkspace.vue";
import { i18n } from "../../../../../src-web/i18n/index";

const { useWorkspaceMock } = vi.hoisted(() => ({ useWorkspaceMock: vi.fn() }));
vi.mock("../../../../../src-web/features/game/composables/useGameWorkspace", () => ({
  useGameWorkspace: useWorkspaceMock,
}));

let capturedPorts: Parameters<typeof useWorkspaceMock>[0];
let confirmation: ReturnType<typeof ref<{ kind: "regenerate" } | undefined>>;

beforeEach(() => {
  useWorkspaceMock.mockReset();
});

describe("GameWorkspace", () => {
  it("装配开局后的舞台、输入、对话记录和历史修订通知", async () => {
    const session = { sessionId: "session-1", title: "雾钟地窖" } as OpenedSession;
    const current = ref({
      historyRevision: 8,
      phase: "idle",
      scene: { path: [] },
      needsRecovery: false,
      lastOperation: { roundId: "round-1" },
    });
    confirmation = ref(undefined);
    const prefs = {
      panelPinned: ref(false),
      diceMode: ref<"manual" | "auto">("manual"),
      ready: ref(true),
      saving: ref(false),
      saveError: ref(undefined),
      loadError: ref(undefined),
      setPanelPinned: vi.fn(),
      setDiceMode: vi.fn(),
      load: vi.fn(),
    } as unknown as ReturnType<typeof useUiPreferences>;
    const panel = {
      state: ref("pinned"),
      isOpen: ref(true),
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
    const records = { state: ref({ body: undefined, recovering: false }), loadBody: vi.fn() };
    const fixture = {
      session: computed(() => session),
      t: (key: string) => i18n.global.t(key),
      phase: { connecting: ref(false) },
      records,
      current,
      presence: {
        snapshot: ref({ characters: [] }),
        loading: ref(false),
        unavailable: ref(false),
        selectedCharacterId: ref(undefined),
        select: vi.fn(),
        refresh: vi.fn(),
      },
      sceneHint: {
        loading: ref(false),
        hints: ref([]),
        unavailable: ref(false),
        failed: ref(false),
        request: vi.fn(),
      },
      input: { draft: ref(""), composing: ref(false), tooLarge: ref(false) },
      controls: { busy: ref(false) },
      preferences: prefs,
      loadingOlder: ref(false),
      unreadRecords: ref(false),
      panelProtected: ref(false),
      confirmation,
      rewindSelectorOpen: ref(false),
      slashMenuDismissed: ref(false),
      panel,
      orderedRecords: [],
      composerPreview: {},
      composerSubmitDisabled: false,
      composerSubmitLabel: "行动",
      scenePath: undefined,
      olderCursor: undefined,
      game: { historyPending: ref(false), recover: vi.fn() },
      generation: undefined,
      preview: undefined,
      hasTextSelection: () => false,
      updateFollowState: vi.fn(),
      onReadingPointerDown: vi.fn(),
      jumpToLatest: vi.fn(),
      onComposerKeydown: vi.fn(),
      interruptDraft: vi.fn(),
      submitComposer: vi.fn(),
      confirmOperation: vi.fn(),
      chooseSlashCommand: vi.fn(),
      closePanel: vi.fn(),
      loadOlder: vi.fn(),
      phaseName: (phase: string) => phase,
      reconnect: vi.fn(),
    };
    useWorkspaceMock.mockImplementation((ports) => {
      capturedPorts = ports;
      ports.emitHistoryRevision(current.value.historyRevision);
      return fixture;
    });
    const wrapper = mount(GameWorkspace, {
      props: { session, productBusy: false, preferences: prefs },
      global: { plugins: [i18n], stubs: { RecordFeed: true, ActionComposer: true } },
    });
    await wrapper.vm.$nextTick();

    expect(wrapper.find(".game-stage").exists()).toBe(true);
    expect(wrapper.text()).toContain(session.title);
    expect(wrapper.findComponent(ActionComposer).exists()).toBe(true);
    expect(capturedPorts.recordsFeed.value).toBeInstanceOf(HTMLDivElement);
    expect(capturedPorts.confirmDialog.value?.tagName).toBe("DIALOG");
    expect(wrapper.emitted("historyRevision")).toEqual([[8]]);

    const regenerate = wrapper
      .findAll("button")
      .find((button) => button.text() === i18n.global.t("game.regenerate"));
    expect(regenerate).toBeDefined();
    await regenerate?.trigger("click");
    expect(toValue(confirmation)).toEqual({ kind: "regenerate" });
  });
});
