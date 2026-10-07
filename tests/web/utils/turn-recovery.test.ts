import { describe, expect, it, vi } from "vitest";
import { type TurnSnapshot } from "../../../src-web/api/llm";
import {
  createTurnRecovery,
  turnRecoveryError,
  type TurnRecoveryView,
} from "../../../src-web/utils/turn-recovery";
import { type TurnNotification } from "../../../src-web/utils/turn-consumer";

function deferred<T>(): {
  promise: Promise<T>;
  resolve(value: T): void;
  reject(error: unknown): void;
} {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function snapshot(text = "", chunk = 0, extra: Partial<TurnSnapshot> = {}): TurnSnapshot {
  return { turnId: "a", text, seq: { chunk, done: 0, failed: 0 }, ...extra };
}
function chunk(seq: number, delta = "字", turnId = "a"): TurnNotification {
  return { kind: "chunk", envelope: { seq, data: { turnId, delta } } };
}
function done(chunkSeq: number): TurnNotification {
  return {
    kind: "done",
    envelope: {
      seq: 1,
      data: { turnId: "a", outcome: "completed", finishReason: "stop", chunkSeq },
    },
  };
}
function harness(
  getSnapshot = vi.fn<(id: string) => Promise<TurnSnapshot>>().mockResolvedValue(snapshot()),
) {
  let view: TurnRecoveryView = { recovering: false, needsRecovery: false };
  const consumer = createTurnRecovery({
    getSnapshot,
    publish: (next) => {
      view = next;
    },
  });
  return {
    consumer,
    getSnapshot,
    get view() {
      return view;
    },
  };
}

describe("turn recovery", () => {
  it("buffers subscription events before snapshot and never exposes missing terminal text", async () => {
    const pending = deferred<TurnSnapshot>();
    const h = harness(vi.fn().mockReturnValue(pending.promise));
    await h.consumer.select("a");
    h.consumer.receive(chunk(2, "后"));
    h.consumer.receive(done(2));
    h.consumer.receive(chunk(1, "前"));
    const ready = h.consumer.connect();
    await Promise.resolve();
    expect(h.view.snapshot).toBeUndefined();
    pending.resolve(snapshot());
    await ready;
    expect(h.view.snapshot).toMatchObject({
      text: "前后",
      outcome: "completed",
      finishReason: "stop",
      seq: { chunk: 2, done: 1, failed: 0 },
    });
    expect(h.getSnapshot).toHaveBeenCalledTimes(1);
    h.consumer.receive(chunk(2, "重复"));
    expect(h.view.snapshot?.text).toBe("前后");
  });
  it("coalesces hundreds of events, bounds overflow and does not fetch once per event", async () => {
    const first = deferred<TurnSnapshot>();
    const next = deferred<TurnSnapshot>();
    const get = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(next.promise);
    const h = harness(get);
    await h.consumer.select("a");
    const ready = h.consumer.connect();
    await Promise.resolve();
    for (let seq = 1; seq <= 200; seq++) h.consumer.receive(chunk(seq));
    h.consumer.receive(done(200));
    expect(get).toHaveBeenCalledTimes(1);
    first.resolve(snapshot());
    await vi.waitFor(() => expect(get).toHaveBeenCalledTimes(2));
    expect(h.view.snapshot?.outcome).toBeUndefined();
    next.resolve(
      snapshot("字".repeat(200), 200, {
        outcome: "completed",
        finishReason: "stop",
        seq: { chunk: 200, done: 1, failed: 0 },
      }),
    );
    await ready;
    expect(h.view.snapshot?.text.length).toBe(200);
    expect(h.view.needsRecovery).toBe(false);
    expect(get).toHaveBeenCalledTimes(2);
  });
  it("a covering snapshot avoids redundant followup after cache overflow", async () => {
    const first = deferred<TurnSnapshot>();
    const h = harness(vi.fn().mockReturnValue(first.promise));
    await h.consumer.select("a");
    const ready = h.consumer.connect();
    await Promise.resolve();
    for (let i = 1; i <= 70; i++) h.consumer.receive(chunk(i));
    first.resolve(snapshot("字".repeat(70), 70));
    await ready;
    expect(h.getSnapshot).toHaveBeenCalledTimes(1);
    expect(h.view.needsRecovery).toBe(false);
  });
  it("last lost events require explicit recovery and never start polling", async () => {
    const get = vi
      .fn()
      .mockResolvedValueOnce(snapshot())
      .mockResolvedValue(
        snapshot("终文", 1, {
          outcome: "completed",
          finishReason: "length",
          seq: { chunk: 1, done: 1, failed: 0 },
        }),
      );
    const h = harness(get);
    await h.consumer.select("a");
    await h.consumer.connect();
    await Promise.resolve();
    await Promise.resolve();
    expect(h.view.snapshot?.outcome).toBeUndefined();
    expect(get).toHaveBeenCalledTimes(1);
    await h.consumer.recover();
    expect(h.view.snapshot).toMatchObject({
      text: "终文",
      outcome: "completed",
      finishReason: "length",
    });
    h.consumer.disconnect();
    await h.consumer.connect();
    expect(get).toHaveBeenCalledTimes(3);
  });
  it("switches identity and ignores old successes, errors and responses after disconnect", async () => {
    const old = deferred<TurnSnapshot>();
    const get = vi
      .fn()
      .mockReturnValueOnce(old.promise)
      .mockResolvedValue(snapshot("新", 1, { turnId: "b" }));
    const h = harness(get);
    await h.consumer.select("a");
    const first = h.consumer.connect();
    await Promise.resolve();
    await h.consumer.select("b");
    old.resolve(snapshot("旧", 1));
    await first;
    expect(h.view.snapshot?.turnId).toBe("b");
    expect(h.view.snapshot?.text).toBe("新");
    expect(h.view.recoveryError).toBeUndefined();
    expect(h.view.needsRecovery).toBe(false);
    h.consumer.receive(chunk(2, "旧尾", "a"));
    expect(h.view.snapshot?.text).toBe("新");
    const late = deferred<TurnSnapshot>();
    get.mockReturnValueOnce(late.promise);
    const pending = h.consumer.recover();
    await Promise.resolve();
    h.consumer.disconnect();
    late.reject({ code: "llm.auth", message: "旧错" });
    await pending;
    expect(h.view.recoveryError).toBeUndefined();
    expect(h.view.recovering).toBe(false);
    await h.consumer.select();
    expect(h.view.snapshot).toBeUndefined();
    await h.consumer.recover();
    h.consumer.receive(chunk(1));
    await h.consumer.select();
  });
  it("rejects stale snapshots without rolling baseline or lingering previous finishReason", async () => {
    const good = snapshot("", 0, {
      outcome: "failed",
      finishReason: "guard",
      error: { code: "llm.empty-output", message: "空" },
      seq: { chunk: 0, done: 0, failed: 1 },
    });
    const get = vi.fn().mockResolvedValueOnce(good).mockResolvedValue(snapshot("旧", 1));
    const h = harness(get);
    await h.consumer.select("a");
    await h.consumer.connect();
    await h.consumer.recover();
    expect(h.view.snapshot).toEqual(good);
    expect(get).toHaveBeenCalledTimes(3);
    expect(h.view.needsRecovery).toBe(true);
    await h.consumer.select("b");
    expect(h.view.snapshot).toBeUndefined();
    get.mockResolvedValue(
      snapshot("", 0, { turnId: "b", outcome: "cancelled", seq: { chunk: 0, done: 1, failed: 0 } }),
    );
    await h.consumer.recover();
    expect(h.view.snapshot?.finishReason).toBeUndefined();
    expect(h.view.snapshot?.error).toBeUndefined();
  });
  it("preserves text on errors and coalesces manual requests in flight", async () => {
    const pending = deferred<TurnSnapshot>();
    const get = vi
      .fn()
      .mockResolvedValueOnce(snapshot("前", 1))
      .mockReturnValueOnce(pending.promise);
    const h = harness(get);
    await h.consumer.select("a");
    await h.consumer.connect();
    const first = h.consumer.recover();
    expect(h.consumer.recover()).toBe(first);
    await Promise.resolve();
    pending.reject({ code: "store.io", message: "失败", detail: "不展示" });
    await first;
    expect(h.view.snapshot?.text).toBe("前");
    expect(h.view.recoveryError).toEqual({ code: "store.io", message: "失败" });
    get.mockResolvedValue(snapshot("前后", 2));
    h.consumer.receive(chunk(2, "后"));
    await h.consumer.recover();
    expect(h.view.snapshot?.text).toBe("前后");
    expect(h.view.recoveryError).toBeUndefined();
  });
  it("recovers gaps, invalid events and applies live failures consistently", async () => {
    const get = vi.fn().mockResolvedValue(snapshot());
    const h = harness(get);
    await h.consumer.select("a");
    await h.consumer.connect();
    h.consumer.receive(chunk(1, "前"));
    expect(h.view.snapshot?.text).toBe("前");
    get.mockResolvedValue(snapshot("前后", 3));
    h.consumer.receive(chunk(3));
    await h.consumer.recover();
    expect(h.view.snapshot?.seq.chunk).toBe(3);
    h.consumer.receive(chunk(0));
    await h.consumer.recover();
    expect(h.view.snapshot?.text).toBe("前后");
    h.consumer.receive({
      kind: "failed",
      envelope: {
        seq: 1,
        data: { turnId: "a", code: "llm.aborted", message: "中断", chunkSeq: 3 },
      },
    });
    expect(h.view.snapshot?.outcome).toBe("failed");
  });
  it("rejects mismatched snapshots and behind servers stop after one followup", async () => {
    const get = vi.fn().mockResolvedValue(snapshot("", 0, { turnId: "b" }));
    const h = harness(get);
    await h.consumer.select("a");
    await h.consumer.connect();
    expect(get).toHaveBeenCalledTimes(2);
    expect(h.view.snapshot).toBeUndefined();
    get.mockResolvedValue(snapshot());
    await h.consumer.recover();
    h.consumer.receive(done(4));
    await h.consumer.recover();
    expect(h.view.needsRecovery).toBe(true);
    expect(h.view.snapshot?.outcome).toBeUndefined();
  });
  it("deduplicates buffered events and cancels not-yet-started recovery on disconnect", async () => {
    const gate = deferred<TurnSnapshot>();
    const get = vi.fn().mockReturnValue(gate.promise);
    const h = harness(get);
    await h.consumer.select("a");
    const request = h.consumer.connect();
    h.consumer.disconnect();
    await request;
    expect(get).not.toHaveBeenCalled();
    const next = h.consumer.connect();
    await Promise.resolve();
    h.consumer.receive(chunk(1));
    h.consumer.receive(chunk(1));
    h.consumer.receive(done(1));
    h.consumer.receive(done(1));
    gate.resolve(snapshot());
    await next;
    expect(h.view.snapshot?.text).toBe("字");
    expect(get).toHaveBeenCalledTimes(1);
    h.consumer.disconnect();
  });
  it("normalizes unknown recovery errors without raw strings", () => {
    for (const error of [
      null,
      "private",
      {},
      { code: 1, message: "x" },
      { code: "x" },
      { code: "x", message: 1 },
    ])
      expect(turnRecoveryError(error)).toEqual({
        code: "app.event-failed",
        message: "回合快照恢复失败",
      });
  });
});

it("publishes identity only after resetting its generation and preserves reentrant events", async () => {
  let switched = false;
  let latest: TurnRecoveryView;
  const consumer = createTurnRecovery({
    getSnapshot: async (turnId) => snapshot("", 0, { turnId }),
    publish(view) {
      latest = view;
      if (switched && !view.snapshot && !view.recovering) {
        switched = false;
        consumer.receive(chunk(1, "新", "b"));
      }
    },
  });
  await consumer.select("a");
  await consumer.connect();
  switched = true;
  await consumer.select("b");
  expect(latest!.snapshot?.text).toBe("新");
});

it("reentrant disconnect during state publication still returns a Promise and sends no request", async () => {
  let interrupt = false;
  const getSnapshot = vi.fn().mockResolvedValue(snapshot());
  const consumer = createTurnRecovery({
    getSnapshot,
    publish(view) {
      if (interrupt && view.recovering) {
        interrupt = false;
        consumer.disconnect();
      }
    },
  });
  await consumer.select("a");
  interrupt = true;
  const request = consumer.connect();
  expect(request).toBeInstanceOf(Promise);
  await request;
  expect(getSnapshot).not.toHaveBeenCalled();
});

it("reads once after a terminal arriving during the ordinary followup and never displays incomplete completion", async () => {
  for (const kind of ["done", "failed"] as const) {
    const first = deferred<TurnSnapshot>();
    const second = deferred<TurnSnapshot>();
    const final = deferred<TurnSnapshot>();
    const get = vi
      .fn()
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
      .mockReturnValueOnce(final.promise);
    const h = harness(get);
    await h.consumer.select("a");
    const ready = h.consumer.connect();
    await Promise.resolve();
    h.consumer.receive(chunk(3, "后"));
    first.resolve(snapshot());
    await vi.waitFor(() => expect(get).toHaveBeenCalledTimes(2));
    if (kind === "done") h.consumer.receive(done(3));
    else
      h.consumer.receive({
        kind: "failed",
        envelope: {
          seq: 1,
          data: { turnId: "a", code: "llm.aborted", message: "断开", chunkSeq: 3 },
        },
      });
    second.resolve(snapshot("前", 1));
    await vi.waitFor(() => expect(get).toHaveBeenCalledTimes(3));
    expect(h.view.snapshot?.outcome).toBeUndefined();
    final.resolve(
      snapshot(
        "前中后",
        3,
        kind === "done"
          ? { outcome: "completed", finishReason: "stop", seq: { chunk: 3, done: 1, failed: 0 } }
          : {
              outcome: "failed",
              error: { code: "llm.aborted", message: "断开" },
              seq: { chunk: 3, done: 0, failed: 1 },
            },
      ),
    );
    await ready;
    expect(h.view.snapshot?.text).toBe("前中后");
    expect(h.view.needsRecovery).toBe(false);
    expect(get).toHaveBeenCalledTimes(3);
  }
});

it("reconnect keeps one physical read per UUID and obtains a fresh response after old success or error", async () => {
  for (const fails of [false, true]) {
    const old = deferred<TurnSnapshot>();
    const get = vi.fn().mockReturnValueOnce(old.promise).mockResolvedValue(snapshot("新", 1));
    const h = harness(get);
    await h.consumer.select("a");
    const first = h.consumer.connect();
    await Promise.resolve();
    h.consumer.disconnect();
    const second = h.consumer.connect();
    await Promise.resolve();
    expect(get).toHaveBeenCalledTimes(1);
    if (fails) old.reject({ code: "store.io", message: "旧错" });
    else old.resolve(snapshot("旧", 1));
    await Promise.all([first, second]);
    expect(get).toHaveBeenCalledTimes(2);
    expect(h.view.snapshot?.text).toBe("新");
    expect(h.view.recoveryError).toBeUndefined();
  }
});

it("switching away while waiting an older same-ID read does not send an obsolete request", async () => {
  const old = deferred<TurnSnapshot>();
  const get = vi
    .fn()
    .mockReturnValueOnce(old.promise)
    .mockResolvedValue(snapshot("新", 1, { turnId: "b" }));
  const h = harness(get);
  await h.consumer.select("a");
  const first = h.consumer.connect();
  await Promise.resolve();
  h.consumer.disconnect();
  const waiting = h.consumer.connect();
  await Promise.resolve();
  await h.consumer.select("b");
  old.resolve(snapshot("旧", 1));
  await Promise.all([first, waiting]);
  expect(get).toHaveBeenCalledTimes(2);
  expect(h.view.snapshot?.turnId).toBe("b");
});

it("bounds outstanding identities and releases failed physical reads for explicit retry", async () => {
  const pending = Array.from({ length: 32 }, () => deferred<TurnSnapshot>());
  const get = vi.fn().mockImplementation((id: string) => pending[Number(id)]!.promise);
  const h = harness(get);
  const requests: Promise<void>[] = [];
  await h.consumer.connect();
  for (let i = 0; i < 32; i++) {
    requests.push(h.consumer.select(String(i)));
    await Promise.resolve();
  }
  await h.consumer.select("overflow");
  expect(get).toHaveBeenCalledTimes(32);
  expect(h.view.recoveryError?.code).toBe("app.busy");
  pending.forEach((item, i) => item.resolve(snapshot("", 0, { turnId: String(i) })));
  await Promise.all(requests);
  get.mockRejectedValueOnce("error");
  await h.consumer.recover();
  expect(h.view.recoveryError?.code).toBe("app.event-failed");
  get.mockResolvedValue(snapshot("", 0, { turnId: "overflow" }));
  await h.consumer.recover();
  expect(h.view.needsRecovery).toBe(false);
});
