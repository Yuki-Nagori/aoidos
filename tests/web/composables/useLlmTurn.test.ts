import { effectScope, ref } from "vue";
import { describe, expect, it, vi } from "vitest";
import { useLlmTurn, type TurnTransport } from "../../../src-web/composables/useLlmTurn";
import {
  type TurnSnapshot,
  type TurnEnvelope,
  type TurnChunk,
  type TurnDone,
  type TurnFailed,
} from "../../../src-web/api/llm";

vi.mock("../../../src-web/api/llm", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../src-web/api/llm")>()),
  getTurn: vi.fn(),
  listenTurnEvent: vi.fn(),
}));
import { getTurn, listenTurnEvent } from "../../../src-web/api/llm";

type Wire = TurnEnvelope<TurnChunk | TurnDone | TurnFailed>;
function snapshot(turnId = "a"): TurnSnapshot {
  return { turnId, text: "", seq: { chunk: 0, done: 0, failed: 0 } };
}
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
function harness() {
  const handlers: {
    name: string;
    callback: (payload: Wire) => void;
    unlisten: ReturnType<typeof vi.fn<() => void>>;
  }[] = [];
  const pending: ReturnType<typeof deferred<() => void>>[] = [];
  const listen: TurnTransport["listen"] = vi.fn((name, callback) => {
    const wait = deferred<() => void>();
    const unlisten = vi.fn();
    handlers.push({ name, callback: callback as (payload: Wire) => void, unlisten });
    pending.push(wait);
    return wait.promise;
  });
  const get = vi.fn().mockImplementation(async (id: string) => snapshot(id));
  const transport: TurnTransport = { getTurn: get, listen };
  const id = ref<string | undefined>("a");
  const scope = effectScope();
  const consumer = scope.run(() => useLlmTurn(id, transport))!;
  return { id, scope, consumer, handlers, pending, get, listen };
}

describe("useLlmTurn lifecycle", () => {
  it("waits for all listeners, buffers early events and changes reactive identity safely", async () => {
    const h = harness();
    await vi.waitFor(() => expect(h.pending).toHaveLength(3));
    expect(h.get).not.toHaveBeenCalled();
    expect(h.consumer.connecting.value).toBe(true);
    h.handlers[0]!.callback({ seq: 1, data: { turnId: "a", delta: "前" } });
    h.pending[0]!.resolve(h.handlers[0]!.unlisten);
    h.pending[1]!.resolve(h.handlers[1]!.unlisten);
    await Promise.resolve();
    expect(h.get).not.toHaveBeenCalled();
    h.handlers[1]!.callback({
      seq: 1,
      data: { turnId: "a", outcome: "completed", finishReason: "stop", chunkSeq: 1 },
    });
    h.pending[2]!.resolve(h.handlers[2]!.unlisten);
    await h.consumer.reconnect();
    expect(h.consumer.view.value.snapshot).toMatchObject({
      text: "前",
      outcome: "completed",
      finishReason: "stop",
    });
    expect(h.consumer.connecting.value).toBe(false);
    h.id.value = "b";
    await h.consumer.recover();
    expect(h.consumer.view.value.snapshot?.turnId).toBe("b");
    expect(h.consumer.view.value.snapshot?.finishReason).toBeUndefined();
    h.handlers[2]!.callback({
      seq: 1,
      data: {
        turnId: "b",
        code: "llm.empty-output",
        message: "空",
        finishReason: "length",
        chunkSeq: 0,
      },
    });
    expect(h.consumer.view.value.snapshot).toMatchObject({
      outcome: "failed",
      finishReason: "length",
    });
    h.scope.stop();
    for (const handler of h.handlers) expect(handler.unlisten).toHaveBeenCalledOnce();
    await h.consumer.reconnect();
    expect(h.listen).toHaveBeenCalledTimes(3);
  });
  it("reconnect releases old handlers and ignores callbacks from old registrations", async () => {
    const h = harness();
    await vi.waitFor(() => expect(h.pending).toHaveLength(3));
    h.pending.forEach((wait, i) => wait.resolve(h.handlers[i]!.unlisten));
    await h.consumer.reconnect();
    const request = h.consumer.reconnect();
    expect(h.consumer.reconnect()).toBe(request);
    await vi.waitFor(() => expect(h.pending).toHaveLength(6));
    for (const handler of h.handlers.slice(0, 3)) expect(handler.unlisten).toHaveBeenCalledOnce();
    h.handlers[0]!.callback({ seq: 1, data: { turnId: "a", delta: "旧" } });
    expect(h.consumer.view.value.snapshot?.text).toBe("");
    h.pending.slice(3).forEach((wait, i) => wait.resolve(h.handlers[i + 3]!.unlisten));
    await request;
    expect(h.get).toHaveBeenCalledTimes(2);
    h.scope.stop();
    h.handlers[3]!.callback({ seq: 1, data: { turnId: "a", delta: "卸载后" } });
    expect(h.consumer.view.value.snapshot?.text).toBe("");
  });
  it("failed registration releases successful subscriptions and can be retried explicitly", async () => {
    const h = harness();
    await vi.waitFor(() => expect(h.pending).toHaveLength(3));
    h.pending[0]!.resolve(h.handlers[0]!.unlisten);
    h.pending[1]!.reject({ code: "app.event-failed", message: "监听失败" });
    h.pending[2]!.resolve(h.handlers[2]!.unlisten);
    await h.consumer.reconnect();
    expect(h.consumer.connectionError.value).toEqual({
      code: "app.event-failed",
      message: "监听失败",
    });
    expect(h.get).not.toHaveBeenCalled();
    expect(h.handlers[0]!.unlisten).toHaveBeenCalledOnce();
    expect(h.handlers[2]!.unlisten).toHaveBeenCalledOnce();
    const retry = h.consumer.reconnect();
    await vi.waitFor(() => expect(h.pending).toHaveLength(6));
    h.pending.slice(3).forEach((wait, i) => wait.resolve(h.handlers[i + 3]!.unlisten));
    await retry;
    expect(h.consumer.connectionError.value).toBeUndefined();
    h.scope.stop();
  });
  it("unmount during pending registration releases late results without requesting a snapshot", async () => {
    const h = harness();
    await vi.waitFor(() => expect(h.pending).toHaveLength(3));
    h.scope.stop();
    h.pending.forEach((wait, i) => wait.resolve(h.handlers[i]!.unlisten));
    await h.consumer.reconnect();
    await vi.waitFor(() =>
      h.handlers.forEach((handler) => expect(handler.unlisten).toHaveBeenCalledOnce()),
    );
    expect(h.get).not.toHaveBeenCalled();
    expect(h.consumer.connecting.value).toBe(false);
  });
  it("the default API adapter works with a getter and normalizes synchronous listener errors", async () => {
    vi.mocked(getTurn).mockResolvedValue(snapshot());
    vi.mocked(listenTurnEvent).mockImplementation(() => {
      throw new Error("private raw");
    });
    const scope = effectScope();
    const consumer = scope.run(() => useLlmTurn(() => "a"))!;
    await consumer.reconnect();
    expect(consumer.connectionError.value?.code).toBe("app.event-failed");
    scope.stop();
  });
});

it("disposal before registration starts avoids creating platform listeners", async () => {
  const h = harness();
  h.scope.stop();
  await Promise.resolve();
  await Promise.resolve();
  expect(h.listen).not.toHaveBeenCalled();
  expect(h.get).not.toHaveBeenCalled();
});
