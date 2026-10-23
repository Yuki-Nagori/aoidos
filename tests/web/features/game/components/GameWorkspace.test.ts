import { mount } from "@vue/test-utils";
import { computed, nextTick, ref } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RecordItem } from "../../../../../src-web/api/records";
import type { OpenedSession } from "../../../../../src-web/api/engine";
import type { useUiPreferences } from "../../../../../src-web/features/game/composables/useUiPreferences";
import ActionComposer from "../../../../../src-web/features/game/components/ActionComposer.vue";
import GameWorkspace from "../../../../../src-web/features/game/components/GameWorkspace.vue";
import RecordFeed from "../../../../../src-web/features/game/components/RecordFeed.vue";
import { i18n } from "../../../../../src-web/i18n/index";

const { useWorkspaceMock } = vi.hoisted(() => ({ useWorkspaceMock: vi.fn() }));
vi.mock("../../../../../src-web/features/game/composables/useGameWorkspace", () => ({
  useGameWorkspace: useWorkspaceMock,
}));

let capturedPorts: Parameters<typeof useWorkspaceMock>[0];
let confirmation: ReturnType<typeof ref<boolean>>;

beforeEach(() => {
  useWorkspaceMock.mockReset();
});

describe("GameWorkspace", () => {
  it("装配开局后的舞台、输入、对话记录和历史修订通知", async () => {
    vi.useFakeTimers();
    const session = {
      sessionId: "session-1",
      scriptId: "mistbell",
      title: "雾钟地窖",
    } as OpenedSession;
    const current = ref<{
      historyRevision: number;
      phase: string;
      scene?: { path: { kind: string; id: string; title: string }[] };
      needsRecovery: boolean;
      lastOperation: { roundId: string; outcome: string };
      check?: {
        planId: string;
        status: "waiting" | "rolling";
        mode: "manual" | "auto";
        ruleId: string;
        actorId: string;
        expression: string;
        modifierTotal: number;
      };
    }>({
      historyRevision: 8,
      phase: "idle",
      scene: { path: [] },
      needsRecovery: false,
      lastOperation: { roundId: "round-1", outcome: "completed" },
    });
    confirmation = ref(false);
    const orderedRecords = ref<RecordItem[]>([]);
    const prefs = {
      panelPinned: ref(false),
      diceMode: ref<"manual" | "auto">("manual"),
      diceSkin: ref<"auto">("auto"),
      ready: ref(true),
      saving: ref(false),
      saveError: ref(undefined),
      loadError: ref(undefined),
      setPanelPinned: vi.fn(),
      setDiceMode: vi.fn(),
      setDiceSkin: vi.fn(),
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
    const activeTurnId = ref<string>();
    const turnPreview = ref<{
      turnId: string;
      text: string;
      outcome?: "completed";
    }>();
    const fixture = {
      session: computed(() => session),
      t: (key: string, values?: Record<string, string>) => i18n.global.t(key, values ?? {}),
      phase: { connecting: ref(false) },
      connectionFailed: ref(true),
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
      orderedRecords,
      composerPreview: {},
      composerSubmitDisabled: false,
      composerSubmitLabel: "行动",
      scenePath: "第一幕 · 塔门",
      olderCursor: undefined,
      game: {
        historyPending: ref(false),
        latestPublicRecord: ref(undefined),
        recover: vi.fn(),
      },
      generation: undefined,
      preview: turnPreview,
      activeTurnId,
      hasTextSelection: () => false,
      updateFollowState: vi.fn(),
      onReadingPointerDown: vi.fn(),
      jumpToLatest: vi.fn(),
      onComposerKeydown: vi.fn(),
      interruptDraft: vi.fn(),
      submitComposer: vi.fn(),
      confirmOperation: vi.fn(),
      requestRegenerate: vi.fn(),
      selectRewindTarget: vi.fn(),
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
      props: {
        session,
        scriptTitle: "Mistbell Cellar",
        suggestedAction: "Look around the cellar.",
        productBusy: false,
        errorMessage: "No price is registered for this model.",
        preferences: prefs,
      },
      global: {
        plugins: [i18n],
        stubs: {
          RecordFeed: {
            props: ["items", "preview", "onRequestRewind", "rewindSelectorOpen"],
            emits: ["rewind"],
            template:
              '<div class="record-feed-probe" :data-preview="preview?.text ?? \'\'" :data-narration-count="items.filter((item) => item.kind === \'narration\').length" />',
          },
          CheckDiceTray: {
            emits: ["presentationSettled"],
            template: '<button class="check-dice-probe" @click="$emit(\'presentationSettled\')" />',
          },
          ActionComposer: true,
        },
      },
    });
    await wrapper.vm.$nextTick();

    expect(wrapper.find(".game-stage").exists()).toBe(true);
    expect(wrapper.find(".game-topbar .phase-pill").exists()).toBe(true);
    expect(wrapper.find(".stage-area .stage-art").exists()).toBe(true);
    expect(wrapper.get(".game-topbar .title").text()).toBe("Mistbell Cellar");
    expect(wrapper.get('[role="alert"]').text()).toContain(
      "No price is registered for this model.",
    );
    expect(wrapper.get('[role="alert"]').text()).toContain(i18n.global.t("common.reconnect"));
    await wrapper.get('[role="alert"] button').trigger("click");
    expect(fixture.reconnect).toHaveBeenCalledOnce();
    expect(wrapper.find(".work").exists()).toBe(false);
    const feed = wrapper.findComponent(RecordFeed);
    expect(feed.props("onRequestRewind")).toBeTypeOf("function");
    feed.props("onRequestRewind")?.();
    await nextTick();
    expect(fixture.rewindSelectorOpen.value).toBe(true);
    feed.vm.$emit("rewind", 1);
    expect(fixture.selectRewindTarget).toHaveBeenCalledWith(1);
    fixture.rewindSelectorOpen.value = false;
    expect(wrapper.find(".dialogue-panel-history > .record-feed").exists()).toBe(true);
    expect(wrapper.find(".dialogue-panel-composer").exists()).toBe(true);
    expect(wrapper.text()).not.toContain(i18n.global.t("game.diceMode"));
    panel.state.value = "hover";
    await nextTick();
    expect(wrapper.findComponent(ActionComposer).exists()).toBe(true);
    expect(wrapper.find(".work").exists()).toBe(false);
    current.value = { ...current.value, scene: undefined };
    await nextTick();
    expect(wrapper.get(".scene-empty-state h2").text()).toBe("Mistbell Cellar");
    current.value = { ...current.value, scene: { path: [] } };
    await nextTick();
    expect(wrapper.findComponent(ActionComposer).exists()).toBe(true);
    expect(wrapper.findComponent(ActionComposer).props("suggestedAction")).toBe(
      "Look around the cellar.",
    );
    orderedRecords.value = [
      {
        recordSeq: 1,
        createdAt: "2026-10-10T00:00:00Z",
        kind: "playerSpeech",
        body: { playerId: "player", text: "I look around." },
      },
    ];
    await nextTick();
    expect(wrapper.findComponent(ActionComposer).props("suggestedAction")).toBeUndefined();
    expect(capturedPorts.recordsFeed.value).toBeInstanceOf(HTMLDivElement);
    expect(capturedPorts.confirmDialog.value?.tagName).toBe("DIALOG");
    expect(wrapper.emitted("historyRevision")).toEqual([[8]]);

    current.value = {
      ...current.value,
      phase: "awaitingCheck",
      check: {
        planId: "plan-1",
        status: "rolling",
        mode: "manual",
        ruleId: "probe",
        actorId: "player",
        expression: "2d6+1",
        modifierTotal: 1,
      },
    };
    activeTurnId.value = "turn-1";
    turnPreview.value = {
      turnId: "turn-1",
      text: "The tower answers.",
      outcome: "completed",
    };
    orderedRecords.value = [
      {
        recordSeq: 2,
        createdAt: "2026-10-10T00:00:01Z",
        kind: "narration",
        turnId: "turn-1",
        body: {
          text: "The tower answers.",
          turnId: "turn-1",
          outcome: "completed",
          finishReason: "stop",
        },
      },
    ];
    await nextTick();
    expect(wrapper.find(".work").exists()).toBe(false);
    expect(wrapper.get(".record-feed-probe").attributes("data-preview")).toBe("");
    expect(wrapper.get(".record-feed-probe").attributes("data-narration-count")).toBe("0");
    await wrapper.get(".check-dice-probe").trigger("click");
    await nextTick();
    expect(wrapper.get(".record-feed-probe").attributes("data-preview")).toBe("");
    await vi.advanceTimersByTimeAsync(45);
    expect(wrapper.get(".record-feed-probe").attributes("data-preview")).toBe("T");
    await vi.advanceTimersByTimeAsync(45);
    expect(wrapper.get(".record-feed-probe").attributes("data-preview")).toBe("Th");
    await vi.advanceTimersByTimeAsync(1000);
    expect(wrapper.get(".record-feed-probe").attributes("data-preview")).toBe("The tower answers.");
    expect(wrapper.get(".record-feed-probe").attributes("data-narration-count")).toBe("1");
    current.value = { ...current.value, phase: "idle", check: undefined };
    await nextTick();

    expect(
      wrapper
        .findAll("button")
        .some((button) => button.text() === i18n.global.t("game.regenerate")),
    ).toBe(false);
    expect(wrapper.findComponent(ActionComposer).props("availableCommands")).toContain(
      "regenerate",
    );
    wrapper.unmount();
    vi.useRealTimers();
  });
});
