// 真实主 Webview 装载产品消费者，不复制恢复算法；夹具始终运行本地 Rust Provider。
import { createApp, h, ref, watch, type DeepReadonly } from "vue";
import { invoke } from "@tauri-apps/api/core";
import {
  getTurn,
  submitTurn,
  cancelTurn,
  listenTurnEvent,
  type TurnInput,
  type TurnSnapshot,
} from "../../../../src-web/api/llm";
import { useLlmTurn, type TurnTransport } from "../../../../src-web/composables/useLlmTurn";
import { type TurnNotification } from "../../../../src-web/utils/turn-consumer";

import { useRecordView } from "../../../../src-web/composables/useRecordView";
import { useMigration } from "../../../../src-web/composables/useMigration";
import { getUiPreferences, setUiPreferences } from "../../../../src-web/api/store";
import { getRecordPage } from "../../../../src-web/api/records";
import { useEnginePhase } from "../../../../src-web/composables/useEnginePhase";
import {
  submitInput,
  interruptRound,
  cancelRound,
  resume,
  regenerate,
  rewind,
  submitCheck,
  getPhase,
  listenPhaseEvent,
  type PhaseState,
} from "../../../../src-web/api/engine";

function check(value: boolean, code: string): void {
  if (!value) throw { code };
}

async function smoke(): Promise<void> {
  const turnId = ref<string>();
  const sessionId = ref<string>();
  const gameId = ref<string>();
  let phaseCallbacks = 0,
    phaseReleased = 0,
    phaseListeners = 0,
    phaseReads = 0;
  let dropPhase = false;
  const phaseNames = new Set<string>();
  let phase!: ReturnType<typeof useEnginePhase>;
  let records!: ReturnType<typeof useRecordView>;
  let migration!: ReturnType<typeof useMigration>;
  let consumer!: ReturnType<typeof useLlmTurn>;
  let requests = 0;
  let callbacks = 0;
  let released = 0;
  let dropAll = false;
  const transport: TurnTransport = {
    getTurn(id) {
      requests += 1;
      return getTurn(id);
    },
    listen(name, handler) {
      return listenTurnEvent(name, (payload) => {
        callbacks += 1;
        if (!dropAll) handler(payload);
      }).then((unlisten) => () => {
        released += 1;
        unlisten();
      });
    },
  };
  const app = createApp({
    setup() {
      consumer = useLlmTurn(turnId, transport);
      records = useRecordView(sessionId);
      migration = useMigration();
      phase = useEnginePhase(gameId, {
        read(id) {
          phaseReads++;
          return getPhase(id);
        },
        listen(name, handler) {
          return listenPhaseEvent(name, (envelope) => {
            phaseCallbacks++;
            phaseNames.add(name);
            if (!dropPhase) handler(envelope);
          }).then((unlisten) => {
            phaseListeners++;
            return () => {
              phaseReleased++;
              unlisten();
            };
          });
        },
      });
      return () => h("div", "Mythos consumer fixture");
    },
  });
  app.mount("#app");
  function phaseState(
    predicate: (state: DeepReadonly<PhaseState>) => boolean,
  ): Promise<DeepReadonly<PhaseState>> {
    const state = phase.state.value.snapshot;
    if (state && predicate(state)) return Promise.resolve(state);
    return new Promise((resolve) => {
      const stop = watch(
        () => phase.state.value.snapshot,
        (next) => {
          if (next && predicate(next)) {
            stop();
            resolve(next);
          }
        },
        { flush: "sync" },
      );
    });
  }
  const received: TurnNotification[] = [];
  const observers: (() => void)[] = [];
  let barrier: (() => void) | undefined;
  function completed(): Promise<TurnSnapshot> {
    const current = consumer.view.value.snapshot;
    if (current?.outcome) return Promise.resolve(current);
    return new Promise((resolve) => {
      const stop = watch(
        () => consumer.view.value.snapshot,
        (snapshot) => {
          if (snapshot?.outcome) {
            stop();
            resolve(snapshot);
          }
        },
        { flush: "sync" },
      );
    });
  }
  let unmounted = false;
  let passed = false;
  let diagnostic = "";
  try {
    await consumer.reconnect();
    observers.push(
      await listenTurnEvent("llm:turn:chunk", (envelope) => {
        received.push({ kind: "chunk", envelope });
        if (envelope.data.delta === "unmounted") barrier?.();
      }),
    );
    observers.push(
      await listenTurnEvent("llm:turn:done", (envelope) =>
        received.push({ kind: "done", envelope }),
      ),
    );
    observers.push(
      await listenTurnEvent("llm:turn:failed", (envelope) =>
        received.push({ kind: "failed", envelope }),
      ),
    );
    let rejected = false;
    try {
      await submitTurn(
        "ipc-fixture",
        { kind: "chat", messages: [{ role: "tool", content: "invalid" }] } as unknown as TurnInput,
        "debug-fixture-v1",
      );
    } catch (error) {
      rejected =
        typeof error === "object" &&
        error !== null &&
        "code" in error &&
        error.code === "app.bad-request";
    }
    check(rejected, "invalid-input-not-rejected");
    const first = await submitTurn(
      "ipc-fixture",
      { kind: "completion", prompt: "本地夹具" },
      "debug-fixture-v1",
    );
    turnId.value = first.turnId;
    const snapshot = await completed();
    const cancelled = await cancelTurn(first.turnId);
    const raw = await getTurn(first.turnId);
    check(
      snapshot.text === "本地夹具正文。" &&
        snapshot.outcome === "completed" &&
        snapshot.finishReason === "stop" &&
        raw.text === snapshot.text &&
        cancelled.outcome === "completed",
      "consumer-snapshot-mismatch",
    );
    check(
      received.filter((event) => event.kind === "chunk").length === 1 &&
        received.filter((event) => event.kind === "done").length === 1 &&
        snapshot.seq.chunk === 1 &&
        snapshot.seq.done === 1 &&
        snapshot.seq.failed === 0,
      "event-sequence-mismatch",
    );
    await consumer.reconnect();
    check(
      released === 3 && consumer.view.value.snapshot?.text === snapshot.text,
      "reconnect-mismatch",
    );
    dropAll = true;
    const second = await submitTurn(
      "ipc-fixture",
      { kind: "completion", prompt: "丢失最后所有事件" },
      "debug-fixture-v1",
    );
    turnId.value = second.turnId;
    await consumer.recover();
    const before = requests;
    await invoke("wait_smoke", { turnId: second.turnId });
    check(requests === before, "unexpected-polling");
    await consumer.recover();
    check(
      requests === before + 1 &&
        consumer.view.value.snapshot?.outcome === "completed" &&
        consumer.view.value.snapshot.text === "本地夹具正文。",
      "lost-events-recovery-mismatch",
    );
    await migration.reconnect();
    check(
      migration.state.value.snapshot?.phase === "completed" &&
        migration.state.value.snapshot.current === 1,
      "migration-terminal-mismatch",
    );
    await setUiPreferences(true, "auto");
    const preferences = await getUiPreferences();
    check(preferences.panelPinned && preferences.diceMode === "auto", "preferences-mismatch");
    const persisted = await invoke<{ sessionId: string; turnId: string }>("record_smoke");
    sessionId.value = persisted.sessionId;
    await records.reconnect();
    await invoke("wait_smoke", { turnId: persisted.turnId });
    await records.recover();
    const page = await getRecordPage(persisted.sessionId);
    const item = page.items[0];
    const recordedTurn = await getTurn(persisted.turnId);
    check(
      item?.turnId === persisted.turnId &&
        item.outcome === "completed" &&
        item.body !== undefined &&
        "text" in item.body &&
        item.body.text === recordedTurn.text,
      "record-snapshot-bytes-mismatch",
    );
    check(
      records.state.value.view?.items[0]?.recordSeq === item?.recordSeq,
      "record-consumer-mismatch",
    );
    // 登记本地可信场景后调用产品命令，等待真实事件；测试不增加持续快照轮询。
    await setUiPreferences(true, "manual");
    gameId.value = await invoke<string>("game_smoke");
    await phase.reconnect();
    const initialPhase = await getPhase(gameId.value);
    const game = await submitInput(gameId.value, "搜索房间");
    const waiting = await phaseState((state) => state.check?.status === "waiting");
    check(waiting.inFlight?.roundId === game.roundId, "game-round-discovery");
    let debugBusy = false;
    try {
      await submitTurn(
        "ipc-fixture",
        { kind: "completion", prompt: "共享门禁" },
        "debug-fixture-v1",
      );
    } catch (error) {
      debugBusy =
        typeof error === "object" && error !== null && "code" in error && error.code === "app.busy";
    }
    check(debugBusy, "game-debug-gate-not-shared");
    let invalidSteer = false;
    try {
      await interruptRound(gameId.value, game.roundId, "等待时插话");
    } catch (error) {
      invalidSteer =
        typeof error === "object" &&
        error !== null &&
        "code" in error &&
        error.code === "engine.invalid-phase";
    }
    check(invalidSteer, "waiting-interrupt-not-rejected");
    const plan = waiting.check!.planId;
    await submitCheck(gameId.value, game.roundId, plan);
    await submitCheck(gameId.value, game.roundId, plan);
    await phaseState(
      (state) =>
        state.lastOperation?.operationId === game.operationId &&
        state.lastOperation.outcome === "completed",
    );
    check(
      (await cancelRound(gameId.value, game.roundId)).outcome === "completed",
      "game-cached-terminal",
    );
    const regenerated = await regenerate(gameId.value, game.roundId);
    await phaseState(
      (state) =>
        state.lastOperation?.operationId === regenerated.operationId &&
        state.lastOperation.outcome === "completed",
    );
    const gamePage = await getRecordPage(gameId.value, undefined, 50);
    check(
      gamePage.items.filter((item) => item.kind === "dice").length === 1,
      "regenerate-rerolled-dice",
    );
    const checkSeq = gamePage.items.find((item) => item.kind === "check")!.recordSeq;
    await rewind(gameId.value, checkSeq);
    await invoke("game_restart_smoke", { sessionId: gameId.value });
    await phase.reconnect();
    const reopened = await getPhase(gameId.value);
    check(
      reopened.stateEpoch !== initialPhase.stateEpoch && reopened.resumeRequired,
      "game-restart-checkpoint",
    );
    const resumed = await resume(gameId.value);
    await phaseState(
      (state) =>
        state.lastOperation?.operationId === resumed.operationId &&
        state.lastOperation.outcome === "completed",
    );
    check(
      (await getRecordPage(gameId.value, undefined, 50)).items.filter(
        (item) => item.kind === "dice",
      ).length === 1,
      "resume-rerolled-dice",
    );
    await setUiPreferences(true, "auto");
    gameId.value = await invoke<string>("game_smoke", { scenario: "failure" });
    await phase.recover();
    const failedGame = await submitInput(gameId.value, "搜索失败夹具");
    await phaseState(
      (state) =>
        state.lastOperation?.operationId === failedGame.operationId &&
        state.lastOperation.outcome === "failed",
    );
    gameId.value = await invoke<string>("game_smoke", { scenario: "exit" });
    await phase.recover();
    const exitGame = await submitInput(gameId.value, "搜索出口夹具");
    await phaseState(
      (state) =>
        state.lastOperation?.operationId === exitGame.operationId &&
        state.lastOperation.outcome === "completed",
    );
    check(phase.state.value.snapshot?.scene === undefined, "exit-scene-not-cleared");
    check(phaseNames.size === 4, "phase-event-stream-missing");
    gameId.value = await invoke<string>("game_smoke", { scenario: "exit" });
    await phase.recover();
    const baselineReads = phaseReads;
    dropPhase = true;
    const lostGame = await submitInput(gameId.value, "丢弃所有阶段事件");
    // 夹具只读等待 Rust 已确认终态；产品消费者本身不轮询。
    await invoke("game_wait_smoke", { sessionId: gameId.value, operationId: lostGame.operationId });
    check(phaseReads === baselineReads, "phase-added-polling");
    dropPhase = false;
    await phase.recover();
    check(
      phase.state.value.snapshot?.lastOperation?.operationId === lostGame.operationId &&
        phase.state.value.snapshot.lastOperation.outcome === "completed",
      "phase-active-recovery-failed",
    );
    let stageBarrier!: () => void;
    const stageSeen = new Promise<void>((resolve) => {
      stageBarrier = resolve;
    });
    observers.push(await listenPhaseEvent("engine:phase:changed", () => stageBarrier()));
    const beforePhaseUnmount = phaseCallbacks;
    const beforeUnmount = callbacks;
    app.unmount();
    unmounted = true;
    check(phaseReleased === phaseListeners, "phase-listener-release-mismatch");
    check(released === 6, "listener-release-mismatch");
    const seen = new Promise<void>((resolve) => {
      barrier = resolve;
    });
    await invoke("emit_smoke", { turnId: second.turnId, sessionId: gameId.value });
    await seen;
    await stageSeen;
    check(phaseCallbacks === beforePhaseUnmount, "unmounted-phase-listener-called");
    check(callbacks === beforeUnmount, "unmounted-listener-called");
    passed = true;
  } catch (error) {
    diagnostic =
      typeof error === "object" && error !== null && "code" in error
        ? String(error.code)
        : "ipc-error";
  } finally {
    if (!unmounted) app.unmount();
    for (const unlisten of observers) unlisten();
  }
  await invoke("report_smoke", { passed, diagnostic });
}
void smoke();
