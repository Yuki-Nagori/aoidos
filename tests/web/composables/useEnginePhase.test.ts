import { effectScope, ref } from "vue";
import { expect, it, vi } from "vitest";
import { listenPhaseEvent, type PhaseState } from "../../../src-web/api/engine";
import type { TurnEnvelope } from "../../../src-web/api/llm";
import { useEnginePhase } from "../../../src-web/composables/useEnginePhase";
import { snapshot } from "../utils/phase-fixture";

function deferred() {
  let resolve!: (off: () => void) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<() => void>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
it("waits for all four listeners, retains early events and frees every registration", async () => {
  const registration = deferred(),
    off = vi.fn();
  const listen = vi.fn(listenPhaseEvent).mockResolvedValue(off);
  let handler!: (event: TurnEnvelope<PhaseState>) => void;
  listen.mockImplementationOnce((_, receive) => {
    handler = receive as typeof handler;
    return registration.promise;
  });
  const read = vi.fn(async (sessionId: string) => snapshot({ sessionId }));
  const id = ref<string | undefined>(snapshot().sessionId);
  const scope = effectScope();
  const consumer = scope.run(() => useEnginePhase(() => id.value, { read, listen }))!;
  await vi.waitFor(() => expect(listen).toHaveBeenCalledTimes(1));
  expect(read).not.toHaveBeenCalled();
  handler({ seq: 1, data: snapshot({ phaseRevision: 1, phase: "generating" }) });
  registration.resolve(off);
  await consumer.reconnect();
  expect(listen).toHaveBeenCalledTimes(4);
  expect(consumer.state.value.snapshot?.phase).toBe("generating");
  id.value = snapshot().stateEpoch;
  await consumer.recover();
  expect(consumer.state.value.snapshot?.sessionId).toBe(id.value);
  const reconnect = consumer.reconnect();
  expect(consumer.reconnect()).toBe(reconnect);
  await reconnect;
  expect(off).toHaveBeenCalledTimes(4);
  scope.stop();
  expect(off).toHaveBeenCalledTimes(8);
  await consumer.reconnect();
  handler({ seq: 2, data: snapshot({ phaseRevision: 2 }) });
  expect(consumer.state.value.recovering).toBe(false);
});
it("does not start the other listeners after disposal and cleans late registrations", async () => {
  const registration = deferred(),
    off = vi.fn();
  const listen = vi.fn(listenPhaseEvent).mockReturnValue(registration.promise);
  const read = vi.fn(async () => snapshot());
  const scope = effectScope();
  const consumer = scope.run(() => useEnginePhase(snapshot().sessionId, { read, listen }))!;
  await vi.waitFor(() => expect(listen).toHaveBeenCalledTimes(1));
  scope.stop();
  registration.resolve(off);
  await consumer.reconnect();
  await vi.waitFor(() => expect(off).toHaveBeenCalledTimes(1));
  expect(listen).toHaveBeenCalledTimes(1);
  expect(read).not.toHaveBeenCalled();
  const early = effectScope();
  const stopped = early.run(() => useEnginePhase(undefined, { read, listen }))!;
  early.stop();
  await stopped.reconnect();
});
it("releases prior listeners on registration failure and ignores a late rejected registration", async () => {
  const off = vi.fn(),
    read = vi.fn(async () => snapshot());
  const listen = vi
    .fn(listenPhaseEvent)
    .mockResolvedValueOnce(off)
    .mockRejectedValue(new Error("private transport diagnostic"));
  const scope = effectScope();
  const consumer = scope.run(() => useEnginePhase(snapshot().sessionId, { read, listen }))!;
  await consumer.reconnect();
  expect(off).toHaveBeenCalledTimes(1);
  expect(consumer.connectionError.value?.code).toBe("app.event-failed");
  expect(consumer.connectionError.value?.message).not.toContain("private");
  expect(read).not.toHaveBeenCalled();
  scope.stop();
  const registration = deferred();
  const late = effectScope();
  const stopped = late.run(() =>
    useEnginePhase(snapshot().sessionId, {
      read,
      listen: vi.fn(listenPhaseEvent).mockReturnValue(registration.promise),
    }),
  )!;
  await Promise.resolve();
  late.stop();
  registration.reject(new Error("late"));
  await stopped.reconnect();
  expect(stopped.connectionError.value).toBeUndefined();
});
