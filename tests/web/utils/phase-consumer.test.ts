import { expect, it } from "vitest";
import type { PhaseSnapshot } from "../../../src-web/api/engine";
import {
  consumePhaseEvent,
  consumePhaseSnapshot,
  validPhaseState,
} from "../../../src-web/utils/phase-consumer";
import { snapshot } from "./phase-fixture";
it("merges cross-stream state revisions and advances old-event baselines without rolling back", () => {
  const initial = snapshot();
  const advanced = snapshot({ phaseRevision: 3, historyRevision: 2, phase: "advancing" });
  const first = consumePhaseEvent(initial, {
    name: "engine:scene:advanced",
    envelope: { seq: 1, data: advanced },
  });
  expect(first.needsRecovery).toBe(false);
  expect(first.snapshot.phaseRevision).toBe(3);
  const old = snapshot({ phaseRevision: 1, phase: "generating" });
  const next = consumePhaseEvent(first.snapshot, {
    name: "engine:phase:changed",
    envelope: { seq: 1, data: old },
  });
  expect(next.snapshot.phase).toBe("advancing");
  expect(next.snapshot.historyRevision).toBe(2);
  expect(next.snapshot.seq).toEqual({
    phaseChanged: 1,
    sceneAdvanced: 1,
    operationDone: 0,
    operationFailed: 0,
  });
  expect(
    consumePhaseEvent(next.snapshot, {
      name: "engine:phase:changed",
      envelope: { seq: 1, data: old },
    }).needsRecovery,
  ).toBe(false);
  expect(
    consumePhaseEvent(next.snapshot, {
      name: "engine:phase:changed",
      envelope: { seq: 3, data: advanced },
    }).needsRecovery,
  ).toBe(true);
  expect(
    consumePhaseEvent(next.snapshot, {
      name: "engine:operation:done",
      envelope: { seq: 1, data: { ...advanced, phase: "idle" } },
    }).needsRecovery,
  ).toBe(true);
  expect(consumePhaseSnapshot(next.snapshot, initial)).toBeUndefined();
  expect(consumePhaseSnapshot(next.snapshot, { ...next.snapshot, phase: "idle" })).toBeUndefined();
  expect(
    consumePhaseSnapshot(next.snapshot, {
      ...next.snapshot,
      seq: { ...next.snapshot.seq, operationDone: 1 },
    }),
  ).toEqual({ ...next.snapshot, seq: { ...next.snapshot.seq, operationDone: 1 } });
});
it("requires snapshot recovery for epoch changes and discovers a public turn within the same phase", () => {
  const initial = snapshot({ phase: "generating", phaseRevision: 1 });
  const active = {
    operationId: initial.sessionId,
    roundId: initial.sessionId,
    turnId: initial.sessionId,
  };
  const changed = { ...initial, phaseRevision: 2, inFlight: active };
  const next = consumePhaseEvent(initial, {
    name: "engine:phase:changed",
    envelope: { seq: 1, data: changed },
  });
  expect(next.snapshot.inFlight).toEqual(active);
  expect(next.snapshot.phase).toBe("generating");
  expect(
    consumePhaseEvent(next.snapshot, {
      name: "engine:phase:changed",
      envelope: { seq: 2, data: { ...changed, stateEpoch: initial.sessionId } },
    }).needsRecovery,
  ).toBe(true);
  expect(
    consumePhaseSnapshot(next.snapshot, {
      ...changed,
      stateEpoch: initial.sessionId,
      seq: initial.seq,
    })?.stateEpoch,
  ).toBe(initial.sessionId);
  expect(
    consumePhaseEvent(next.snapshot, {
      name: "engine:phase:changed",
      envelope: { seq: 0, data: changed },
    }).needsRecovery,
  ).toBe(true);
  expect(
    consumePhaseEvent(next.snapshot, {
      name: "engine:phase:changed",
      envelope: { seq: 2, data: { ...changed, sessionId: initial.stateEpoch } },
    }).needsRecovery,
  ).toBe(false);
});
it("rejects invalid capacities, conflicting recovery states and incomplete confirmation sequences", () => {
  expect(validPhaseState(snapshot())).toBe(true);
  for (const extra of [
    { sessionId: "bad" },
    { phaseRevision: -1 },
    { historyRevision: Number.MAX_SAFE_INTEGER + 1 },
    { resumeRequired: true },
    { inFlight: { operationId: "bad" } },
    { scene: { sceneId: "room", path: [] } },
    { scene: { sceneId: "room", path: [{ kind: "scene", id: "room", title: "界".repeat(86) }] } },
    {
      check: {
        planId: "plan",
        status: "waiting",
        mode: "manual",
        ruleId: "r",
        actorId: "p",
        expression: "02d6",
        modifierTotal: 0,
      },
    },
    { lastOperation: { operationId: snapshot().sessionId, outcome: "failed" } },
  ] as Partial<PhaseSnapshot>[])
    expect(validPhaseState(snapshot(extra))).toBe(false);
  expect(
    consumePhaseSnapshot(
      undefined,
      snapshot({
        seq: { phaseChanged: -1, sceneAdvanced: 0, operationDone: 0, operationFailed: 0 },
      }),
    ),
  ).toBeUndefined();
  expect(
    consumePhaseSnapshot(snapshot(), snapshot({ sessionId: snapshot().stateEpoch })),
  ).toBeUndefined();
});

it("validates frozen check expressions, paused anchors and terminal error shapes against Rust limits", () => {
  const base = snapshot();
  const check = {
    planId: "plan",
    status: "waiting" as const,
    mode: "manual" as const,
    ruleId: "search",
    actorId: "player",
    expression: "2d6",
    modifierTotal: 0,
  };
  for (const expression of ["2d6", "2d6+3", "2d6-3", "20d1000+100", "1d2-100"]) {
    expect(validPhaseState(snapshot({ check: { ...check, expression } }))).toBe(true);
  }
  for (const expression of [
    "garbage",
    "0d6",
    "21d6",
    "2d1",
    "2d1001",
    "2d6+101",
    "2d6-101",
    "2d6+0",
  ]) {
    expect(validPhaseState(snapshot({ check: { ...check, expression } }))).toBe(false);
  }
  const checkpoint = { sourceRoundId: base.sessionId, throughSeq: 1, stage: "check" as const };
  expect(validPhaseState(snapshot({ resumeRequired: true, checkpoint }))).toBe(true);
  for (const extra of [
    { resumeRequired: true, checkpoint: { ...checkpoint, sourceRoundId: "bad" } },
    { resumeRequired: true, checkpoint: { ...checkpoint, throughSeq: 0 } },
    { resumeRequired: true, checkpoint: { ...checkpoint, throughSeq: 1.5 } },
    { resumeRequired: true, checkpoint, needsRecovery: true },
    { resumeRequired: true, checkpoint, inFlight: { operationId: base.sessionId } },
    { inFlight: { operationId: base.sessionId, roundId: "bad" } },
    { inFlight: { operationId: base.sessionId, turnId: "bad" } },
    { check: { ...check, planId: "/" } },
    { check: { ...check, ruleId: "/" } },
    { check: { ...check, actorId: "/" } },
    { check: { ...check, modifierTotal: 1.5 } },
    { check: { ...check, modifierTotal: -101 } },
    { check: { ...check, modifierTotal: 101 } },
    { lastOperation: { operationId: "bad", outcome: "completed" } },
    { lastOperation: { operationId: base.sessionId, roundId: "bad", outcome: "cancelled" } },
    {
      lastOperation: {
        operationId: base.sessionId,
        outcome: "failed",
        error: { code: "/", message: "故障" },
      },
    },
    {
      lastOperation: {
        operationId: base.sessionId,
        outcome: "failed",
        error: { code: "engine.invalid-phase", message: "界".repeat(171) },
      },
    },
  ] as Partial<PhaseSnapshot>[])
    expect(validPhaseState(snapshot(extra))).toBe(false);
  expect(
    validPhaseState(
      snapshot({
        lastOperation: {
          operationId: base.sessionId,
          roundId: base.sessionId,
          outcome: "completed",
        },
      }),
    ),
  ).toBe(true);
  expect(
    validPhaseState(
      snapshot({
        lastOperation: {
          operationId: base.sessionId,
          outcome: "failed",
          error: { code: "engine.invalid-phase", message: "故障" },
        },
      }),
    ),
  ).toBe(true);
  const current = snapshot({
    phaseRevision: 3,
    historyRevision: 2,
    seq: { phaseChanged: 2, sceneAdvanced: 0, operationDone: 0, operationFailed: 0 },
  });
  expect(
    consumePhaseEvent(current, {
      name: "engine:phase:changed",
      envelope: { seq: 3, data: snapshot({ phaseRevision: 4, historyRevision: 1 }) },
    }).needsRecovery,
  ).toBe(true);
  expect(
    consumePhaseSnapshot(
      current,
      snapshot({ phaseRevision: 4, historyRevision: 1, seq: current.seq }),
    ),
  ).toBeUndefined();
  expect(
    consumePhaseSnapshot(current, snapshot({ phaseRevision: 4, historyRevision: 2 })),
  ).toBeUndefined();
  expect(
    consumePhaseEvent(base, {
      name: "engine:phase:changed",
      envelope: { seq: 1, data: snapshot({ phaseRevision: -1 }) },
    }).needsRecovery,
  ).toBe(true);
});

it("merges same-revision Value events and typed snapshots regardless of nested key order", () => {
  const base = snapshot({
    phaseRevision: 1,
    scene: { sceneId: "room", path: [{ kind: "scene", id: "room", title: "房间" }] },
    lastOperation: {
      operationId: snapshot().sessionId,
      roundId: snapshot().sessionId,
      outcome: "failed",
      error: { code: "engine.invalid-phase", message: "拒绝" },
    },
  });
  const reordered = {
    ...base,
    scene: { path: [{ title: "房间", id: "room", kind: "scene" }], sceneId: "room" },
    lastOperation: {
      error: { message: "拒绝", code: "engine.invalid-phase" },
      outcome: "failed" as const,
      roundId: base.sessionId,
      operationId: base.sessionId,
    },
  };
  const event = consumePhaseEvent(base, {
    name: "engine:operation:failed",
    envelope: { seq: 1, data: reordered },
  });
  expect(event.needsRecovery).toBe(false);
  expect(event.snapshot.seq.operationFailed).toBe(1);
  expect(consumePhaseSnapshot(base, reordered)).toEqual(base);
  reordered.scene.path[0]!.title = "外部后续修改";
  expect(event.snapshot.scene?.path[0]?.title).toBe("房间");
  expect(base.scene?.path[0]?.title).toBe("房间");
});

it("copies a live manual plan and its paused recovery checkpoint without retaining transport objects", () => {
  const id = snapshot().sessionId;
  const candidate = snapshot({
    phase: "awaitingCheck",
    phaseRevision: 1,
    inFlight: { operationId: id, roundId: id },
    check: {
      planId: "plan",
      status: "waiting",
      mode: "manual",
      ruleId: "search",
      actorId: "player",
      expression: "2d6",
      modifierTotal: 0,
    },
    lastOperation: { operationId: id, roundId: id, outcome: "accepted" },
  });
  const accepted = consumePhaseSnapshot(undefined, candidate)!;
  candidate.check!.expression = "2d6+3";
  candidate.inFlight!.turnId = id;
  expect(accepted.check?.expression).toBe("2d6");
  expect(accepted.inFlight?.turnId).toBeUndefined();
  const paused = snapshot({
    phase: "awaitingCheck",
    phaseRevision: 2,
    resumeRequired: true,
    checkpoint: { sourceRoundId: id, throughSeq: 2, stage: "check" },
  });
  const recovered = consumePhaseSnapshot(accepted, paused)!;
  paused.checkpoint!.throughSeq = 3;
  expect(recovered.checkpoint?.throughSeq).toBe(2);
  expect(recovered.lastOperation).toBeUndefined();
});
