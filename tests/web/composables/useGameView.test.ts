import { effectScope, ref } from "vue";
import { expect, it, vi } from "vitest";
import { getPhase, listenPhaseEvent } from "../../../src-web/api/engine";
import { getTurn, type TurnSnapshot } from "../../../src-web/api/llm";
import type { RecordItem, RecordView } from "../../../src-web/api/records";
import { useGameView } from "../../../src-web/composables/useGameView";
import { snapshot } from "../utils/phase-fixture";

const uuid = (n: number) => `00000000-0000-0000-0000-${String(n).padStart(12, "0")}`;
function view(sessionId = snapshot().sessionId, items: RecordItem[] = []): RecordView {
  return {
    sessionId,
    viewEpoch: "epoch",
    items,
    lastSeq: 0,
    lastRecordSeq: items[0]?.recordSeq ?? 0,
    needsRecovery: false,
  };
}
function item(recordSeq: number, kind: string, turnId?: string): RecordItem {
  return {
    recordSeq,
    kind,
    createdAt: "2026-10-09",
    turnId,
    body:
      (kind === "narration" || kind === "characterSpeech") && turnId
        ? { turnId, text: "正文", outcome: "completed", finishReason: "stop" }
        : undefined,
  };
}
function ports() {
  const off = vi.fn();
  return {
    phase: {
      read: vi.fn(getPhase).mockImplementation(async (sessionId) => snapshot({ sessionId })),
      listen: vi.fn(listenPhaseEvent).mockResolvedValue(off),
    },
    records: {
      view: vi.fn(async (sessionId: string) => view(sessionId)),
      page: vi.fn(async (sessionId: string) => view(sessionId)),
      body: vi.fn(async () => ({ text: "" })),
      listen: vi.fn().mockResolvedValue(off),
    },
    turn: {
      getTurn: vi.fn(getTurn).mockImplementation(async (turnId): Promise<TurnSnapshot> => ({
        turnId,
        text: "正文",
        outcome: "completed",
        finishReason: "stop",
        seq: { chunk: 1, done: 1, failed: 0 },
      })),
      listen: vi.fn(async () => off),
    },
    off,
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => {
    resolve = yes;
  });
  return { promise, resolve };
}
it("recovers a completely missed round from the newest public record, including terminal reason", async () => {
  const api = ports(),
    scope = effectScope(),
    game = scope.run(() => useGameView(snapshot().sessionId, api))!;
  await game.reconnect();
  api.records.view.mockResolvedValue(
    view(undefined, [item(2, "narration", uuid(10)), item(1, "playerSpeech")]),
  );
  await game.recover();
  expect(game.turnId.value).toBe(uuid(10));
  expect(game.latestPublicRecord.value?.body).toMatchObject({
    text: "正文",
    outcome: "completed",
    finishReason: "stop",
  });
  // 第二轮的全部阶段、正文及终态事件均不投递；只有主动恢复发现新回合。
  api.records.view.mockResolvedValue(
    view(undefined, [
      item(5, "system"),
      {
        ...item(4, "characterSpeech", uuid(11)),
        body: {
          turnId: uuid(11),
          text: "截断正文",
          outcome: "failed",
          finishReason: "length",
          error: { code: "llm.empty-output", message: "失败" },
        },
      },
      item(3, "playerSpeech"),
      item(2, "narration", uuid(10)),
    ]),
  );
  api.turn.getTurn.mockResolvedValue({
    turnId: uuid(11),
    text: "截断正文",
    outcome: "failed",
    finishReason: "length",
    error: { code: "llm.empty-output", message: "失败" },
    seq: { chunk: 1, done: 0, failed: 1 },
  });
  const recovering = game.recover();
  expect(game.recover()).toBe(recovering);
  await recovering;
  expect(game.turnId.value).toBe(uuid(11));
  expect(game.latestPublicRecord.value?.body).toMatchObject({
    text: "截断正文",
    outcome: "failed",
    finishReason: "length",
  });
  const reads = api.turn.getTurn.mock.calls.length;
  await Promise.resolve();
  expect(api.turn.getTurn).toHaveBeenCalledTimes(reads);
  scope.stop();
  expect(api.off).toHaveBeenCalledTimes(11);
  await game.recover();
});
it("keeps a known public terminal until a new operation enters its private proposal stage", async () => {
  const api = ports(),
    scope = effectScope();
  api.phase.read.mockResolvedValue(
    snapshot({
      phaseRevision: 1,
      phase: "generating",
      inFlight: { operationId: uuid(20), roundId: uuid(21), turnId: uuid(10) },
    }),
  );
  api.turn.getTurn.mockResolvedValue({
    turnId: uuid(10),
    text: "旧失败正文",
    outcome: "failed",
    finishReason: "guard",
    error: { code: "llm.empty-output", message: "旧错误" },
    seq: { chunk: 1, done: 0, failed: 1 },
  });
  const game = scope.run(() => useGameView(snapshot().sessionId, api))!;
  await game.reconnect();
  api.phase.read.mockResolvedValue(snapshot({ phaseRevision: 2 }));
  await game.recover();
  expect(game.turn.view.value.snapshot?.finishReason).toBe("guard");
  api.phase.read.mockResolvedValue(
    snapshot({
      phaseRevision: 3,
      phase: "awaitingCheck",
      inFlight: { operationId: uuid(30), roundId: uuid(31) },
    }),
  );
  api.records.view.mockResolvedValue(
    view(undefined, [item(3, "playerSpeech"), item(2, "narration", uuid(10))]),
  );
  await game.recover();
  expect(game.turnId.value).toBeUndefined();
  expect(game.turn.view.value.snapshot).toBeUndefined();
  // 同一私有操作的更新也不把旧失败重新选回来。
  api.phase.read.mockResolvedValue(
    snapshot({
      phaseRevision: 4,
      phase: "awaitingCheck",
      inFlight: { operationId: uuid(30), roundId: uuid(31) },
    }),
  );
  await game.recover();
  expect(game.turnId.value).toBeUndefined();
  scope.stop();
});
it("prefers public active turns over stale records and retains them when an old window is read", async () => {
  const api = ports(),
    scope = effectScope();
  api.records.view.mockResolvedValue(view(undefined, [item(2, "narration", uuid(10))]));
  api.phase.read.mockResolvedValue(
    snapshot({
      phaseRevision: 1,
      phase: "generating",
      inFlight: { operationId: uuid(20), roundId: uuid(21), turnId: uuid(11) },
    }),
  );
  const game = scope.run(() => useGameView(snapshot().sessionId, api))!;
  await game.reconnect();
  expect(game.turnId.value).toBe(uuid(11));
  await game.recover();
  api.phase.read.mockResolvedValue(snapshot({ phaseRevision: 2 }));
  await game.recover();
  expect(game.turnId.value).toBe(uuid(11));
  scope.stop();
});
it("discovers record in-flight references, but clears an old turn behind a newer player input", async () => {
  const api = ports(),
    scope = effectScope(),
    game = scope.run(() => useGameView(snapshot().sessionId, api))!;
  await game.reconnect();
  api.records.view.mockResolvedValue({ ...view(), inFlight: { turnId: uuid(10), recordSeq: 2 } });
  await game.recover();
  expect(game.turnId.value).toBe(uuid(10));
  api.records.view.mockResolvedValue(
    view(undefined, [item(3, "playerSpeech"), item(2, "narration", uuid(10))]),
  );
  await game.recover();
  expect(game.turnId.value).toBeUndefined();
  api.records.view.mockResolvedValue(view(undefined, [item(4, "playerSpeech")]));
  await game.recover();
  expect(game.turnId.value).toBeUndefined();
  scope.stop();
});
it.each(["session", "dispose"])(
  "discards a pending identity discovery after %s changes",
  async (change) => {
    const api = ports(),
      scope = effectScope(),
      id = ref<string | undefined>(snapshot().sessionId);
    const game = scope.run(() => useGameView(id, api))!;
    await game.reconnect();
    const waiting = deferred<RecordView>();
    api.records.view.mockReturnValueOnce(waiting.promise);
    const recovering = game.recover();
    await vi.waitFor(() => expect(api.records.view).toHaveBeenCalledTimes(2));
    if (change === "session") id.value = uuid(50);
    else scope.stop();
    waiting.resolve(view(undefined, [item(2, "narration", uuid(10))]));
    await recovering;
    expect(game.turnId.value).toBeUndefined();
    if (change === "session") {
      api.records.view.mockImplementation(async (sessionId) =>
        view(sessionId, [item(2, "narration", uuid(11))]),
      );
      await game.recover();
      expect(game.turnId.value).toBe(uuid(11));
      id.value = undefined;
      await game.reconnect();
      expect(game.turnId.value).toBeUndefined();
    }
    scope.stop();
  },
);
it("uses production defaults and registers before any selected turn read", async () => {
  const scope = effectScope(),
    game = scope.run(() => useGameView(undefined))!;
  await game.reconnect();
  expect(game.turnId.value).toBeUndefined();
  scope.stop();
});

it("does not revive an old terminal after a private operation fails before records catch up", async () => {
  const api = ports(),
    scope = effectScope();
  api.records.view.mockResolvedValue(
    view(undefined, [item(2, "narration", uuid(10)), item(1, "playerSpeech")]),
  );
  const game = scope.run(() => useGameView(snapshot().sessionId, api))!;
  await game.reconnect();
  expect(game.turnId.value).toBe(uuid(10));
  api.phase.read.mockResolvedValue(
    snapshot({
      phaseRevision: 1,
      phase: "generating",
      inFlight: { operationId: uuid(20), roundId: uuid(21) },
    }),
  );
  await game.recover();
  expect(game.turnId.value).toBeUndefined();
  api.phase.read.mockResolvedValue(snapshot({ phaseRevision: 2 }));
  await game.recover();
  expect(game.turn.view.value.snapshot).toBeUndefined();
  scope.stop();
});
it("accepts early private and public phase identities before the first record window arrives", async () => {
  const api = ports(),
    scope = effectScope(),
    pending = deferred<RecordView>();
  api.phase.read.mockResolvedValue(
    snapshot({
      phaseRevision: 1,
      phase: "generating",
      inFlight: { operationId: uuid(20), roundId: uuid(21) },
    }),
  );
  api.records.view.mockReturnValue(pending.promise);
  const game = scope.run(() => useGameView(snapshot().sessionId, api))!;
  await vi.waitFor(() => expect(game.phase.state.value.snapshot?.phaseRevision).toBe(1));
  expect(game.turnId.value).toBeUndefined();
  api.phase.read.mockResolvedValue(
    snapshot({
      phaseRevision: 2,
      phase: "generating",
      inFlight: { operationId: uuid(20), roundId: uuid(21), turnId: uuid(10) },
    }),
  );
  await game.phase.recover();
  expect(game.turnId.value).toBe(uuid(10));
  pending.resolve(view());
  await game.recover();
  scope.stop();
});
it("does not raise its old-turn floor to narration already committed ahead of a private phase read", async () => {
  const api = ports(),
    scope = effectScope();
  api.records.view.mockResolvedValue(
    view(undefined, [item(2, "narration", uuid(10)), item(1, "playerSpeech")]),
  );
  const game = scope.run(() => useGameView(snapshot().sessionId, api))!;
  await game.reconnect();
  api.phase.read.mockResolvedValue(
    snapshot({
      phaseRevision: 1,
      phase: "generating",
      inFlight: { operationId: uuid(20), roundId: uuid(21) },
    }),
  );
  api.records.view.mockResolvedValue(
    view(undefined, [
      item(4, "narration", uuid(11)),
      item(3, "playerSpeech"),
      item(2, "narration", uuid(10)),
    ]),
  );
  await game.recover();
  expect(game.turnId.value).toBeUndefined();
  api.phase.read.mockResolvedValue(snapshot({ phaseRevision: 2 }));
  await game.recover();
  expect(game.turnId.value).toBe(uuid(11));
  expect(game.latestPublicRecord.value?.recordSeq).toBe(4);
  expect(api.turn.getTurn).not.toHaveBeenCalled();
  scope.stop();
});
it("reselects a lower surviving record after history revision changes without reusing departed branch state", async () => {
  const api = ports(),
    scope = effectScope();
  api.records.view.mockResolvedValue(
    view(undefined, [item(8, "narration", uuid(11)), item(7, "playerSpeech")]),
  );
  const game = scope.run(() => useGameView(snapshot().sessionId, api))!;
  await game.reconnect();
  expect(game.turnId.value).toBe(uuid(11));
  api.phase.read.mockResolvedValue(snapshot({ phaseRevision: 1, historyRevision: 1 }));
  // 两份同 viewEpoch 的读取跨过回退：第一份旧读已发出，第二份才是确认后的分支。
  api.records.view
    .mockResolvedValueOnce(
      view(undefined, [item(8, "narration", uuid(11)), item(7, "playerSpeech")]),
    )
    .mockResolvedValue({
      ...view(undefined, [item(2, "narration", uuid(10)), item(1, "playerSpeech")]),
      lastRecordSeq: 9,
    });
  await game.recover();
  expect(game.turnId.value).toBe(uuid(10));
  expect(game.latestPublicRecord.value?.recordSeq).toBe(2);
  expect(api.turn.getTurn).not.toHaveBeenCalled();
  scope.stop();
});
it("discards the old ring identity on process epoch changes and displays committed bodies or paged references", async () => {
  const api = ports(),
    scope = effectScope();
  api.phase.read.mockResolvedValue(
    snapshot({
      phaseRevision: 1,
      phase: "generating",
      inFlight: { operationId: uuid(20), roundId: uuid(21), turnId: uuid(10) },
    }),
  );
  const game = scope.run(() => useGameView(snapshot().sessionId, api))!;
  await game.reconnect();
  const reads = api.turn.getTurn.mock.calls.length;
  api.turn.getTurn.mockRejectedValue({ code: "app.not-found", message: "不在新进程的 ring" });
  api.phase.read.mockResolvedValue(snapshot({ stateEpoch: uuid(80) }));
  api.records.view.mockResolvedValue({
    ...view(undefined, [
      { ...item(2, "narration", uuid(10)), body: undefined, bodyRef: "opaque" },
      item(1, "playerSpeech"),
    ]),
    viewEpoch: "new-view",
  });
  await game.recover();
  expect(game.turnId.value).toBe(uuid(10));
  expect(game.latestPublicRecord.value?.bodyRef).toBe("opaque");
  expect(game.turn.view.value.snapshot).toBeUndefined();
  expect(game.turn.view.value.recoveryError).toBeUndefined();
  expect(api.turn.getTurn).toHaveBeenCalledTimes(reads);
  scope.stop();
});
it("resets a lower record floor when only the record view epoch changes", async () => {
  const api = ports(),
    scope = effectScope();
  api.records.view.mockResolvedValue(view(undefined, [item(8, "narration", uuid(11))]));
  const game = scope.run(() => useGameView(snapshot().sessionId, api))!;
  await game.reconnect();
  api.records.view.mockResolvedValue({
    ...view(undefined, [item(2, "narration", uuid(10))]),
    viewEpoch: "new-view",
    lastRecordSeq: 9,
  });
  await game.recover();
  expect(game.turnId.value).toBe(uuid(10));
  scope.stop();
});
it("allows a live public turn whose committed record arrived before phase completion", async () => {
  const api = ports(),
    scope = effectScope();
  api.records.view.mockResolvedValue(view(undefined, [item(2, "narration", uuid(10))]));
  const game = scope.run(() => useGameView(snapshot().sessionId, api))!;
  await game.reconnect();
  api.phase.read.mockResolvedValue(
    snapshot({
      phaseRevision: 1,
      phase: "generating",
      inFlight: { operationId: uuid(20), roundId: uuid(21), turnId: uuid(11) },
    }),
  );
  api.records.view.mockResolvedValue(
    view(undefined, [item(4, "narration", uuid(11)), item(3, "playerSpeech")]),
  );
  await game.recover();
  expect(game.latestPublicRecord.value?.recordSeq).toBe(4);
  api.phase.read.mockResolvedValue(snapshot({ phaseRevision: 2 }));
  await game.recover();
  expect(game.turnId.value).toBe(uuid(11));
  expect(game.latestPublicRecord.value?.recordSeq).toBe(4);
  scope.stop();
});
it.each(["session", "dispose"])(
  "abandons a branch confirmation read after %s changes",
  async (change) => {
    const api = ports(),
      scope = effectScope(),
      id = ref<string | undefined>(snapshot().sessionId);
    api.records.view.mockResolvedValue(view(undefined, [item(8, "narration", uuid(11))]));
    const game = scope.run(() => useGameView(id, api))!;
    await game.reconnect();
    api.phase.read.mockResolvedValue(snapshot({ phaseRevision: 1, historyRevision: 1 }));
    const pending = deferred<RecordView>();
    api.records.view
      .mockResolvedValueOnce(view(undefined, [item(8, "narration", uuid(11))]))
      .mockReturnValueOnce(pending.promise);
    const recovering = game.recover();
    await vi.waitFor(() => expect(api.records.view).toHaveBeenCalledTimes(3));
    if (change === "session") id.value = undefined;
    else scope.stop();
    pending.resolve({ ...view(undefined, [item(2, "narration", uuid(10))]), lastRecordSeq: 9 });
    await recovering;
    expect(game.turnId.value).toBeUndefined();
    expect(game.latestPublicRecord.value).toBeUndefined();
    scope.stop();
  },
);
it("does not absorb a future committed round into a known active turn's floor", async () => {
  const api = ports(),
    scope = effectScope();
  api.phase.read.mockResolvedValue(
    snapshot({
      phaseRevision: 1,
      phase: "generating",
      inFlight: { operationId: uuid(20), roundId: uuid(21), turnId: uuid(10) },
    }),
  );
  api.records.view.mockResolvedValue(
    view(undefined, [item(2, "narration", uuid(10)), item(1, "playerSpeech")]),
  );
  const game = scope.run(() => useGameView(snapshot().sessionId, api))!;
  await game.reconnect();
  // 跨流快照到达不同步：记录已看到下一回合，阶段仍停在此前公开回合。
  api.records.view.mockResolvedValue(
    view(undefined, [item(4, "narration", uuid(11)), item(3, "playerSpeech")]),
  );
  await game.recover();
  api.phase.read.mockResolvedValue(snapshot({ phaseRevision: 2 }));
  await game.recover();
  expect(game.turnId.value).toBe(uuid(11));
  expect(game.latestPublicRecord.value?.recordSeq).toBe(4);
  scope.stop();
});
it("keeps a departed branch hidden when confirmation fails and reveals only a successful retry", async () => {
  const api = ports(),
    scope = effectScope();
  api.records.view.mockResolvedValue(view(undefined, [item(8, "narration", uuid(11))]));
  const game = scope.run(() => useGameView(snapshot().sessionId, api))!;
  await game.reconnect();
  api.phase.read.mockResolvedValue(snapshot({ phaseRevision: 1, historyRevision: 1 }));
  api.records.view
    .mockResolvedValueOnce(view(undefined, [item(8, "narration", uuid(11))]))
    .mockRejectedValueOnce(new Error("confirmation unavailable"));
  await game.recover();
  expect(game.historyPending.value).toBe(true);
  expect(game.turnId.value).toBeUndefined();
  expect(game.latestPublicRecord.value).toBeUndefined();
  expect(game.records.state.value.error).toBeDefined();
  expect(game.records.items.value[0]?.recordSeq).toBe(8);
  const reads = api.records.view.mock.calls.length;
  await Promise.resolve();
  expect(api.records.view).toHaveBeenCalledTimes(reads);
  api.records.view.mockResolvedValue({
    ...view(undefined, [item(2, "narration", uuid(10))]),
    lastRecordSeq: 9,
  });
  await game.recover();
  expect(game.historyPending.value).toBe(false);
  expect(game.turnId.value).toBe(uuid(10));
  expect(game.latestPublicRecord.value?.recordSeq).toBe(2);
  scope.stop();
});
