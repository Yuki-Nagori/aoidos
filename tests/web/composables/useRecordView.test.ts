import { effectScope, ref } from "vue";
import { expect, it, vi } from "vitest";
import { useRecordView } from "../../../src-web/composables/useRecordView";
import type { RecordNotification, RecordView } from "../../../src-web/api/records";
function view(sessionId = "a"): RecordView {
  return {
    sessionId,
    viewEpoch: "epoch",
    lastSeq: 0,
    lastRecordSeq: 0,
    items: [],
    needsRecovery: false,
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
it("registers before fetching, follows getters and releases listeners on reconnect/unmount", async () => {
  const registration = deferred<() => void>();
  let handler!: (event: RecordNotification) => void;
  const off = vi.fn();
  const transport = {
    view: vi.fn(async (id: string) => view(id)),
    page: vi.fn(async (id: string) => view(id)),
    body: vi.fn(async () => ({ text: "x" })),
    listen: vi.fn((callback: (event: RecordNotification) => void) => {
      handler = callback;
      return registration.promise;
    }),
  };
  const id = ref<string | undefined>("a");
  const scope = effectScope();
  const consumer = scope.run(() => useRecordView(() => id.value, transport))!;
  await vi.waitFor(() => expect(transport.listen).toHaveBeenCalledOnce());
  expect(transport.view).not.toHaveBeenCalled();
  handler({
    seq: 1,
    data: { viewEpoch: "epoch", sessionId: "a", recordSeq: 1, kind: "narration" },
  });
  registration.resolve(off);
  await consumer.reconnect();
  expect(consumer.state.value.view?.sessionId).toBe("a");
  expect(consumer.items.value).toEqual([]);
  id.value = "b";
  await consumer.recover();
  expect(consumer.state.value.view?.sessionId).toBe("b");
  transport.listen.mockResolvedValue(off);
  const reconnect = consumer.reconnect();
  expect(consumer.reconnect()).toBe(reconnect);
  await reconnect;
  expect(off).toHaveBeenCalledOnce();
  scope.stop();
  expect(off).toHaveBeenCalledTimes(2);
  await consumer.reconnect();
  handler({
    seq: 2,
    data: { viewEpoch: "epoch", sessionId: "b", recordSeq: 2, kind: "narration" },
  });
  expect(consumer.state.value.recovering).toBe(false);
});
it("cleans late registration and reports synchronous registration failures", async () => {
  const registration = deferred<() => void>();
  const off = vi.fn();
  const transport = {
    view: vi.fn(async (id: string) => view(id)),
    page: vi.fn(async (id: string) => view(id)),
    body: vi.fn(async () => ({ text: "x" })),
    listen: vi.fn(() => registration.promise),
  };
  const scope = effectScope();
  const consumer = scope.run(() => useRecordView("a", transport))!;
  await vi.waitFor(() => expect(transport.listen).toHaveBeenCalledOnce());
  scope.stop();
  registration.resolve(off);
  await Promise.resolve();
  await Promise.resolve();
  expect(off).toHaveBeenCalledOnce();
  await consumer.reconnect();
  const failed = effectScope();
  const failure = failed.run(() =>
    useRecordView("a", {
      ...transport,
      listen: vi.fn(() => {
        throw new Error("secret");
      }),
    }),
  )!;
  await failure.reconnect();
  expect(failure.connectionError.value?.code).toBe("app.event-failed");
  failed.stop();
  const stopped = effectScope();
  const early = stopped.run(() => useRecordView("a", transport))!;
  stopped.stop();
  await early.reconnect();
});

it("does not expose an obsolete registration failure after scope disposal", async () => {
  const registration = deferred<() => void>();
  const transport = {
    view: vi.fn(async (id: string) => view(id)),
    page: vi.fn(async (id: string) => view(id)),
    body: vi.fn(async () => ({ text: "x" })),
    listen: vi.fn(() => registration.promise),
  };
  const scope = effectScope();
  const consumer = scope.run(() => useRecordView("a", transport))!;
  await vi.waitFor(() => expect(transport.listen).toHaveBeenCalledOnce());
  scope.stop();
  registration.reject(new Error("late failure"));
  await Promise.resolve();
  await Promise.resolve();
  expect(consumer.connectionError.value).toBeUndefined();
});
