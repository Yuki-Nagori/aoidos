import { describe, expect, it } from "vitest";
import { type TurnSnapshot } from "../../../src-web/api/llm";
import {
  consumeTurnEvent,
  consumeTurnSnapshot,
  coversSequences,
  validTurnEvent,
  type TurnNotification,
} from "../../../src-web/utils/turn-consumer";

function snapshot(extra: Partial<TurnSnapshot> = {}): TurnSnapshot {
  return { turnId: "a", text: "", seq: { chunk: 0, done: 0, failed: 0 }, ...extra };
}
function chunk(seq = 1, delta = "正文", turnId = "a"): TurnNotification {
  return { kind: "chunk", envelope: { seq, data: { turnId, delta } } };
}
function done(seq = 1, chunkSeq = 0): TurnNotification {
  return {
    kind: "done",
    envelope: { seq, data: { turnId: "a", chunkSeq, outcome: "completed", finishReason: "stop" } },
  };
}

describe("turn consumer", () => {
  it("pins UTF-8 capacities and sequence constraints before buffering", () => {
    expect(validTurnEvent(chunk())).toBe(true);
    for (const event of [
      chunk(0),
      chunk(-1),
      chunk(1.5),
      chunk(Number.MAX_SAFE_INTEGER + 1),
      chunk(1, ""),
      chunk(1, "x".repeat(8193)),
      chunk(1, "界".repeat(2731)),
      done(1, -1),
      done(1, 1.5),
    ])
      expect(validTurnEvent(event)).toBe(false);
    expect(validTurnEvent(chunk(1, "x".repeat(8192)))).toBe(true);
    expect(validTurnEvent(done())).toBe(true);
  });
  it("appends consecutive chunks once and preserves confirmed text across gaps", () => {
    const first = consumeTurnEvent(snapshot(), chunk()).snapshot;
    expect(first.text).toBe("正文");
    expect(consumeTurnEvent(first, chunk()).snapshot).toBe(first);
    expect(consumeTurnEvent(first, chunk()).needsRecovery).toBe(false);
    expect(consumeTurnEvent(first, chunk(1, "other", "b")).snapshot).toBe(first);
    expect(consumeTurnEvent(first, chunk(3)).needsRecovery).toBe(true);
    expect(consumeTurnEvent(first, chunk(0)).needsRecovery).toBe(true);
    expect(
      consumeTurnEvent(
        snapshot({ text: "x".repeat(256 * 1024), seq: { chunk: 1, done: 0, failed: 0 } }),
        chunk(2, "y"),
      ).needsRecovery,
    ).toBe(true);
    expect(
      consumeTurnEvent(snapshot({ outcome: "completed", finishReason: "stop" }), chunk())
        .needsRecovery,
    ).toBe(true);
  });
  it("recovers missing terminal text and distinguishes failed empty reasons and cancellation", () => {
    expect(consumeTurnEvent(snapshot(), done(1, 2)).needsRecovery).toBe(true);
    const complete = consumeTurnEvent(snapshot(), done()).snapshot;
    expect(complete.outcome).toBe("completed");
    expect(complete.finishReason).toBe("stop");
    for (const finishReason of ["stop", "guard", "length"] as const) {
      const event: TurnNotification = {
        kind: "failed",
        envelope: {
          seq: 1,
          data: {
            turnId: "a",
            code: "llm.empty-output",
            message: "空输出",
            chunkSeq: 0,
            finishReason,
          },
        },
      };
      expect(consumeTurnEvent(snapshot(), event).snapshot).toMatchObject({
        outcome: "failed",
        finishReason,
        error: { code: "llm.empty-output" },
      });
    }
    expect(
      consumeTurnEvent(snapshot(), {
        kind: "failed",
        envelope: {
          seq: 1,
          data: { turnId: "a", code: "llm.empty-output", message: "空输出", chunkSeq: 0 },
        },
      }).needsRecovery,
    ).toBe(true);
    expect(
      consumeTurnEvent(snapshot(), {
        kind: "failed",
        envelope: {
          seq: 1,
          data: { turnId: "a", code: "llm.network", message: "断开", chunkSeq: 0 },
        },
      }).snapshot.finishReason,
    ).toBeUndefined();
    const cancelled = consumeTurnEvent(snapshot(), {
      kind: "done",
      envelope: { seq: 1, data: { turnId: "a", outcome: "cancelled", chunkSeq: 0 } },
    }).snapshot;
    expect(cancelled.outcome).toBe("cancelled");
    expect(cancelled.finishReason).toBeUndefined();
  });
  it("rejects stale, rewritten, oversized or inconsistent snapshots without regressing any baseline", () => {
    const current = snapshot({
      text: "前文",
      seq: { chunk: 2, done: 1, failed: 0 },
      outcome: "completed",
      finishReason: "guard",
    });
    expect(
      consumeTurnSnapshot(current, { ...current, seq: { chunk: 2, done: 1, failed: 0 } })?.snapshot,
    ).toEqual(current);
    for (const candidate of [
      { ...current, turnId: "b" },
      { ...current, text: "改文" },
      { ...current, seq: { chunk: 1, done: 1, failed: 0 } },
      { ...current, outcome: undefined, finishReason: undefined },
      { ...current, finishReason: undefined },
      { ...current, error: { code: "x", message: "x" } },
      snapshot({ seq: { chunk: -1, done: 0, failed: 0 } }),
      snapshot({ seq: { chunk: 0, done: 0.5, failed: 0 } }),
      snapshot({ seq: { chunk: 0, done: 0, failed: Number.MAX_SAFE_INTEGER + 1 } }),
      snapshot({ text: "x".repeat(256 * 1024 + 1) }),
      snapshot({ text: "界".repeat(100000) }),
      snapshot({ finishReason: "length" }),
      snapshot({ error: { code: "x", message: "x" } }),
      snapshot({ outcome: "cancelled", finishReason: "stop" }),
      snapshot({ outcome: "cancelled", error: { code: "x", message: "x" } }),
      snapshot({ outcome: "failed" }),
    ])
      expect(consumeTurnSnapshot(current, candidate)).toBeUndefined();
    expect(consumeTurnSnapshot(undefined, snapshot({ outcome: "cancelled" }))).toBeDefined();
    expect(consumeTurnSnapshot(current, { ...current, finishReason: "length" })).toBeUndefined();
    const failed = snapshot({ outcome: "failed", error: { code: "llm.network", message: "断开" } });
    expect(
      consumeTurnSnapshot(failed, { ...failed, error: { code: "llm.auth", message: "断开" } }),
    ).toBeUndefined();
    expect(
      consumeTurnSnapshot(failed, { ...failed, error: { code: "llm.network", message: "不同" } }),
    ).toBeUndefined();
    expect(consumeTurnSnapshot(failed, failed)).toBeDefined();
    expect(
      coversSequences({ chunk: 2, done: 1, failed: 1 }, { chunk: 2, done: 1, failed: 1 }),
    ).toBe(true);
    expect(
      coversSequences({ chunk: 2, done: 0, failed: 1 }, { chunk: 2, done: 1, failed: 1 }),
    ).toBe(false);
    expect(
      coversSequences({ chunk: 2, done: 1, failed: 0 }, { chunk: 2, done: 1, failed: 1 }),
    ).toBe(false);
  });
});
