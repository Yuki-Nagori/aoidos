import { expect, it, vi } from "vitest";
import type { PhaseSnapshot } from "../../../src-web/api/engine";
import {
  createPhaseRecovery,
  type PhaseRecoveryState,
} from "../../../src-web/utils/phase-recovery";
import { snapshot } from "./phase-fixture";

function deferred() {
  let resolve!: (value: PhaseSnapshot) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<PhaseSnapshot>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}
it("buffers subscription events and coalesces a storm into at most two reads", async () => {
  const first = deferred(),
    second = deferred();
  const get = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
  let state: PhaseRecoveryState = { recovering: false, needsRecovery: false };
  const recovery = createPhaseRecovery(get, (next) => {
    state = next;
  });
  recovery.select(snapshot().sessionId);
  for (let seq = 1; seq <= 1000; seq++)
    recovery.receive({
      name: "engine:phase:changed",
      envelope: { seq, data: snapshot({ phaseRevision: seq, phase: "generating" }) },
    });
  const connected = recovery.connect();
  await vi.waitFor(() => expect(get).toHaveBeenCalledTimes(1));
  first.resolve(snapshot());
  await vi.waitFor(() => expect(get).toHaveBeenCalledTimes(2));
  second.resolve(
    snapshot({
      phaseRevision: 1000,
      phase: "generating",
      seq: { phaseChanged: 1000, sceneAdvanced: 0, operationDone: 0, operationFailed: 0 },
    }),
  );
  await connected;
  expect(state.needsRecovery).toBe(false);
  expect(state.snapshot?.phaseRevision).toBe(1000);
  for (let seq = 1001; seq <= 1100; seq++)
    recovery.receive({
      name: "engine:phase:changed",
      envelope: { seq, data: snapshot({ phaseRevision: seq, phase: "generating" }) },
    });
  expect(state.snapshot?.phaseRevision).toBe(1100);
  expect(get).toHaveBeenCalledTimes(2);
  recovery.disconnect();
});
it("ignores old session responses and retains newer state during a stale snapshot read", async () => {
  const old = deferred();
  const next = snapshot({ sessionId: snapshot().stateEpoch, stateEpoch: snapshot().sessionId });
  const get = vi.fn().mockReturnValueOnce(old.promise).mockResolvedValue(next);
  let state: PhaseRecoveryState = { recovering: false, needsRecovery: false };
  const recovery = createPhaseRecovery(get, (candidate) => {
    state = candidate;
  });
  recovery.select(snapshot().sessionId);
  const connected = recovery.connect();
  await vi.waitFor(() => expect(get).toHaveBeenCalledTimes(1));
  recovery.select(next.sessionId);
  await recovery.recover();
  expect(state.snapshot?.sessionId).toBe(next.sessionId);
  old.resolve(snapshot({ phaseRevision: 99 }));
  await connected;
  expect(state.snapshot?.sessionId).toBe(next.sessionId);
  get.mockRejectedValue(new Error("private diagnostic"));
  await recovery.recover();
  expect(state.needsRecovery).toBe(true);
  expect(state.error?.message).not.toContain("private");
  expect(state.snapshot?.sessionId).toBe(next.sessionId);
  recovery.disconnect();
});

it("confirms a reopened epoch, rejects retired responses and ignores old or malformed notifications", async () => {
  const old = snapshot(),
    next = snapshot({
      stateEpoch: old.sessionId,
      phaseRevision: 1,
      seq: { phaseChanged: 1, sceneAdvanced: 0, operationDone: 0, operationFailed: 0 },
    });
  const get = vi.fn().mockResolvedValueOnce(old).mockResolvedValueOnce(old).mockResolvedValue(next);
  let state: PhaseRecoveryState = { recovering: false, needsRecovery: false };
  const recovery = createPhaseRecovery(get, (candidate) => {
    state = candidate;
  });
  recovery.select(old.sessionId);
  await recovery.connect();
  const notification = { name: "engine:phase:changed" as const, envelope: { seq: 1, data: next } };
  recovery.receive(notification);
  recovery.receive(notification);
  await recovery.recover();
  expect(get).toHaveBeenCalledTimes(3);
  expect(state.snapshot?.stateEpoch).toBe(next.stateEpoch);
  for (const envelope of [
    { seq: 2, data: old },
    { seq: 0, data: next },
    { seq: NaN, data: next },
    { seq: 2, data: { ...next, phaseRevision: -1 } },
    { seq: 2, data: { ...next, sessionId: "elsewhere" } },
  ])
    recovery.receive({ name: "engine:phase:changed", envelope });
  expect(get).toHaveBeenCalledTimes(3);
  get.mockResolvedValueOnce(old);
  await recovery.recover();
  expect(state.snapshot?.stateEpoch).toBe(next.stateEpoch);
  expect(state.needsRecovery).toBe(false);
  get.mockResolvedValue({ ...next, phaseRevision: 0 });
  await recovery.recover();
  expect(state.needsRecovery).toBe(true);
  expect(state.snapshot?.phaseRevision).toBe(1);
  recovery.disconnect();
  const count = get.mock.calls.length;
  await recovery.recover();
  recovery.select(undefined);
  await recovery.connect();
  expect(get).toHaveBeenCalledTimes(count);
  recovery.disconnect();
});

it("keeps reads bounded across abandoned sessions and makes the final lost event recoverable on demand", async () => {
  const requests: ReturnType<typeof deferred>[] = [];
  const get = vi.fn(() => {
    const request = deferred();
    requests.push(request);
    return request.promise;
  });
  let state: PhaseRecoveryState = { recovering: false, needsRecovery: false };
  const recovery = createPhaseRecovery(get, (candidate) => {
    state = candidate;
  });
  await recovery.connect();
  for (let i = 1; i <= 33; i++) {
    recovery.select(`00000000-0000-0000-0000-${i.toString().padStart(12, "0")}`);
    await Promise.resolve();
    await Promise.resolve();
  }
  await recovery.recover();
  expect(get).toHaveBeenCalledTimes(32);
  expect(state.error?.code).toBe("app.event-failed");
  recovery.disconnect();
  requests.forEach((request) => request.resolve(snapshot()));
  await Promise.resolve();
  await Promise.resolve();

  const latest = snapshot({ phaseRevision: 2, phase: "advancing" });
  const read = vi.fn().mockResolvedValueOnce(snapshot()).mockResolvedValue(latest);
  const connected = createPhaseRecovery(read, (candidate) => {
    state = candidate;
  });
  connected.select(snapshot().sessionId);
  await connected.connect();
  expect(state.snapshot?.phase).toBe("idle");
  expect(read).toHaveBeenCalledTimes(1);
  await connected.recover();
  expect(state.snapshot?.phase).toBe("advancing");
  expect(read).toHaveBeenCalledTimes(2);
  connected.disconnect();
});

it("bounds unknown epoch hints and retirement history without accepting their late state", async () => {
  const epoch = (i: number) => `00000000-0000-0000-0000-${i.toString().padStart(12, "0")}`;
  let latest = snapshot();
  const get = vi.fn(async () => latest);
  let state: PhaseRecoveryState = { recovering: false, needsRecovery: false };
  const recovery = createPhaseRecovery(get, (candidate) => {
    state = candidate;
  });
  recovery.select(latest.sessionId);
  await recovery.connect();
  for (let i = 10; i <= 30; i++) {
    latest = snapshot({
      stateEpoch: epoch(i),
      phaseRevision: 1,
      seq: { phaseChanged: 1, sceneAdvanced: 0, operationDone: 0, operationFailed: 0 },
    });
    recovery.receive({ name: "engine:phase:changed", envelope: { seq: 1, data: latest } });
    await recovery.recover();
  }
  expect(state.snapshot?.stateEpoch).toBe(epoch(30));
  recovery.disconnect();
  for (let i = 40; i <= 60; i++)
    recovery.receive({
      name: "engine:phase:changed",
      envelope: { seq: 1, data: snapshot({ stateEpoch: epoch(i), phaseRevision: 1 }) },
    });
  // 权威快照仍为最后确认的打开实例；未知提示不能把它替换成任意 UUID。
  await recovery.connect();
  expect(state.snapshot?.stateEpoch).toBe(epoch(30));
  expect(state.needsRecovery).toBe(false);
  recovery.disconnect();
});

it("retains an epoch hint that arrives during the second bounded read for explicit recovery", async () => {
  const first = deferred(),
    second = deferred();
  const reopened = snapshot({
    stateEpoch: snapshot().sessionId,
    phaseRevision: 1,
    seq: { phaseChanged: 1, sceneAdvanced: 0, operationDone: 0, operationFailed: 0 },
  });
  const get = vi
    .fn()
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise)
    .mockResolvedValue(reopened);
  let state: PhaseRecoveryState = { recovering: false, needsRecovery: false };
  const recovery = createPhaseRecovery(get, (candidate) => {
    state = candidate;
  });
  recovery.select(snapshot().sessionId);
  const connected = recovery.connect();
  await vi.waitFor(() => expect(get).toHaveBeenCalledTimes(1));
  recovery.receive({
    name: "engine:phase:changed",
    envelope: { seq: 2, data: snapshot({ phaseRevision: 2 }) },
  });
  first.resolve(snapshot());
  await vi.waitFor(() => expect(get).toHaveBeenCalledTimes(2));
  recovery.receive({ name: "engine:phase:changed", envelope: { seq: 1, data: reopened } });
  second.resolve(
    snapshot({
      phaseRevision: 2,
      seq: { phaseChanged: 2, sceneAdvanced: 0, operationDone: 0, operationFailed: 0 },
    }),
  );
  await connected;
  expect(get).toHaveBeenCalledTimes(2);
  expect(state.needsRecovery).toBe(true);
  expect(state.snapshot?.stateEpoch).not.toBe(reopened.stateEpoch);
  await recovery.recover();
  expect(state.snapshot?.stateEpoch).toBe(reopened.stateEpoch);
  expect(state.needsRecovery).toBe(false);
  recovery.disconnect();
});

it("waits for an abandoned read of the same session to fail before starting its replacement", async () => {
  const previous = deferred();
  const get = vi
    .fn()
    .mockReturnValueOnce(previous.promise)
    .mockImplementation(async (sessionId: string) => snapshot({ sessionId }));
  let state: PhaseRecoveryState = { recovering: false, needsRecovery: false };
  const recovery = createPhaseRecovery(get, (candidate) => {
    state = candidate;
  });
  recovery.select(snapshot().sessionId);
  const old = recovery.connect();
  await vi.waitFor(() => expect(get).toHaveBeenCalledTimes(1));
  recovery.select(snapshot().stateEpoch);
  await recovery.recover();
  recovery.select(snapshot().sessionId);
  const replacement = recovery.recover();
  await Promise.resolve();
  await Promise.resolve();
  expect(get).toHaveBeenCalledTimes(2);
  previous.reject(new Error("abandoned request"));
  await old;
  await replacement;
  expect(get).toHaveBeenCalledTimes(3);
  expect(state.snapshot?.sessionId).toBe(snapshot().sessionId);
  expect(state.error).toBeUndefined();
  recovery.select(snapshot().sessionId);
  expect(get).toHaveBeenCalledTimes(3);
  recovery.disconnect();
});

it("drops queued work after disconnect and discards old-epoch events buffered before reopening", async () => {
  const read = vi.fn(async () => snapshot());
  const stopped = createPhaseRecovery(read, () => undefined);
  stopped.select(snapshot().sessionId);
  const pending = stopped.connect();
  stopped.disconnect();
  await pending;
  expect(read).not.toHaveBeenCalled();
  const request = deferred();
  const reopened = snapshot({
    stateEpoch: snapshot().sessionId,
    phaseRevision: 1,
    phase: "advancing",
    seq: { phaseChanged: 1, sceneAdvanced: 0, operationDone: 0, operationFailed: 0 },
  });
  const get = vi.fn().mockResolvedValueOnce(snapshot()).mockReturnValueOnce(request.promise);
  let state: PhaseRecoveryState = { recovering: false, needsRecovery: false };
  const recovery = createPhaseRecovery(get, (candidate) => {
    state = candidate;
  });
  recovery.select(snapshot().sessionId);
  await recovery.connect();
  const restored = recovery.recover();
  await vi.waitFor(() => expect(get).toHaveBeenCalledTimes(2));
  recovery.receive({
    name: "engine:phase:changed",
    envelope: { seq: 1, data: snapshot({ phaseRevision: 1, phase: "generating" }) },
  });
  recovery.receive({ name: "engine:phase:changed", envelope: { seq: 1, data: reopened } });
  request.resolve(reopened);
  await restored;
  expect(state.snapshot?.phase).toBe("advancing");
  expect(state.snapshot?.stateEpoch).toBe(reopened.stateEpoch);
  expect(state.needsRecovery).toBe(false);
  recovery.disconnect();
});

it("replays queued successors when a late event fills a gap after the two-read budget", async () => {
  const first = deferred(),
    second = deferred();
  const get = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
  let state: PhaseRecoveryState = { recovering: false, needsRecovery: false };
  const recovery = createPhaseRecovery(get, (next) => {
    state = next;
  });
  recovery.select(snapshot().sessionId);
  const connected = recovery.connect();
  await vi.waitFor(() => expect(get).toHaveBeenCalledTimes(1));
  const event = (seq: number) => ({
    name: "engine:phase:changed" as const,
    envelope: { seq, data: snapshot({ phaseRevision: seq }) },
  });
  recovery.receive(event(2));
  first.resolve(snapshot());
  await vi.waitFor(() => expect(get).toHaveBeenCalledTimes(2));
  recovery.receive(event(6));
  recovery.receive(event(4));
  second.resolve(
    snapshot({
      phaseRevision: 2,
      seq: { phaseChanged: 2, sceneAdvanced: 0, operationDone: 0, operationFailed: 0 },
    }),
  );
  await connected;
  expect(state.needsRecovery).toBe(true);
  recovery.receive(event(3));
  expect(state.snapshot?.seq.phaseChanged).toBe(4);
  expect(state.snapshot?.phaseRevision).toBe(4);
  expect(state.needsRecovery).toBe(true);
  recovery.receive(event(5));
  expect(state.snapshot?.seq.phaseChanged).toBe(6);
  expect(state.needsRecovery).toBe(false);
  expect(get).toHaveBeenCalledTimes(2);
  recovery.disconnect();
});
