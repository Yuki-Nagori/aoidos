import { expect, it, vi } from "vitest";
import type { MigrationSnapshot } from "../../../src-web/api/store";
import {
  createMigrationRecovery,
  type MigrationState,
} from "../../../src-web/utils/migration-recovery";
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function snapshot(
  id = "a",
  phase: MigrationSnapshot["phase"] = "running",
  seq = 0,
): MigrationSnapshot {
  return {
    migrationId: id,
    phase,
    from: 0,
    to: 2,
    current: phase === "completed" ? 2 : 1,
    seq: { progress: seq, done: phase === "completed" ? 1 : 0, failed: phase === "failed" ? 1 : 0 },
    ...(phase === "failed" ? { error: { code: "store.migration", message: "迁移失败" } } : {}),
  };
}
function harness(read = vi.fn<() => Promise<MigrationSnapshot>>().mockResolvedValue(snapshot())) {
  let state: MigrationState = { recovering: false, needsRecovery: false };
  const recovery = createMigrationRecovery(read, (next) => {
    state = next;
  });
  return {
    recovery,
    read,
    get state() {
      return state;
    },
  };
}
it("subscribes before reading and coalesces overflow into two snapshots without polling", async () => {
  const first = deferred<MigrationSnapshot>(),
    last = deferred<MigrationSnapshot>();
  const read = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(last.promise);
  const h = harness(read);
  h.recovery.receive({ kind: "progress", seq: 1, migrationId: "a" });
  const ready = h.recovery.connect();
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  for (let seq = 2; seq <= 100; seq++)
    h.recovery.receive({ kind: "progress", seq, migrationId: "a" });
  h.recovery.receive({ kind: "done", seq: 1, migrationId: "a" });
  expect(h.recovery.recover()).toBe(ready);
  first.resolve(snapshot("a", "running", 1));
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  last.resolve(snapshot("a", "completed", 100));
  await ready;
  expect(h.state.snapshot?.phase).toBe("completed");
  expect(h.state.needsRecovery).toBe(false);
  h.recovery.receive({ kind: "done", seq: 1, migrationId: "a" });
  await Promise.resolve();
  expect(read).toHaveBeenCalledTimes(2);
  h.recovery.disconnect();
  await h.recovery.recover();
  expect(read).toHaveBeenCalledTimes(2);
});
it("rejects stale flows, terminal regressions and malformed snapshot baselines", async () => {
  const h = harness();
  await h.recovery.connect();
  h.read.mockResolvedValue(snapshot("b", "completed", 2));
  await h.recovery.recover();
  expect(h.state.snapshot?.migrationId).toBe("b");
  h.read.mockResolvedValue(snapshot("a", "completed", 3));
  await h.recovery.recover();
  expect(h.state.error?.code).toBe("app.event-failed");
  expect(h.state.snapshot?.migrationId).toBe("b");
  h.read.mockResolvedValue(snapshot("b", "running", 2));
  await h.recovery.recover();
  expect(h.state.snapshot?.phase).toBe("completed");
  h.read.mockResolvedValue(snapshot("b", "completed", 1));
  await h.recovery.recover();
  expect(h.state.snapshot?.seq.progress).toBe(2);
  for (const value of [
    { ...snapshot("c"), migrationId: undefined },
    { ...snapshot("c"), to: -1 },
    { ...snapshot("c"), seq: { progress: -1, done: 0, failed: 0 } },
    { ...snapshot("c", "completed"), from: undefined },
    { ...snapshot("c", "failed"), error: undefined },
  ]) {
    h.read.mockResolvedValue(value);
    await h.recovery.recover();
    expect(h.state.error).toBeDefined();
  }
  h.recovery.receive({ kind: "done", seq: 1, migrationId: "a" });
  h.recovery.receive({ kind: "done", seq: 0, migrationId: "c" });
  h.recovery.receive({ kind: "done", seq: 1, migrationId: "" });
});
it("ignores late reads after disconnect and waits physical reads across reconnect", async () => {
  const first = deferred<MigrationSnapshot>();
  const read = vi
    .fn()
    .mockReturnValueOnce(first.promise)
    .mockResolvedValue(snapshot("new", "failed"));
  const h = harness(read);
  const old = h.recovery.connect();
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  h.recovery.disconnect();
  const fresh = h.recovery.connect();
  await Promise.resolve();
  expect(read).toHaveBeenCalledTimes(1);
  first.resolve(snapshot("old", "completed"));
  await old;
  await fresh;
  expect(h.state.snapshot?.migrationId).toBe("new");
  expect(h.state.snapshot?.error?.code).toBe("store.migration");
  h.read.mockRejectedValue(new Error("secret"));
  await h.recovery.recover();
  expect(h.state.error?.message).not.toContain("secret");
});
it("an authoritative read supersedes an earlier unknown-flow hint", async () => {
  const h = harness();
  h.recovery.receive({ kind: "done", seq: 1, migrationId: "new" });
  await h.recovery.connect();
  expect(h.read).toHaveBeenCalledTimes(1);
  expect(h.state.needsRecovery).toBe(false);
});

it("retains the highest sequence through duplicate and old-event floods", async () => {
  const first = deferred<MigrationSnapshot>(),
    last = deferred<MigrationSnapshot>();
  const read = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(last.promise);
  const h = harness(read);
  const ready = h.recovery.connect();
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  h.recovery.receive({ kind: "progress", seq: 100, migrationId: "a" });
  for (let seq = 1; seq <= 32; seq++)
    h.recovery.receive({ kind: "progress", seq, migrationId: "a" });
  first.resolve(snapshot("a", "running", 1));
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  last.resolve(snapshot("a", "running", 32));
  await ready;
  expect(h.state.needsRecovery).toBe(true);
  expect(read).toHaveBeenCalledTimes(2);
});

it("recovers a new terminal flow arriving during the second snapshot without polling", async () => {
  const first = deferred<MigrationSnapshot>(),
    second = deferred<MigrationSnapshot>(),
    last = deferred<MigrationSnapshot>();
  const read = vi
    .fn()
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise)
    .mockReturnValueOnce(last.promise);
  const h = harness(read);
  const ready = h.recovery.connect();
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  h.recovery.receive({ kind: "done", seq: 1, migrationId: "b" });
  first.resolve(snapshot("a"));
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  h.recovery.receive({ kind: "done", seq: 1, migrationId: "c" });
  second.resolve(snapshot("b", "completed"));
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(3));
  last.resolve(snapshot("c", "completed"));
  await ready;
  expect(h.state.snapshot?.migrationId).toBe("c");
  expect(h.state.needsRecovery).toBe(false);
});

it("rejects idle shape errors and terminal changes even with nonregressing sequences", async () => {
  const h = harness();
  const idle: MigrationSnapshot = {
    phase: "idle",
    from: 0,
    to: 0,
    current: 0,
    seq: { progress: 0, done: 0, failed: 0 },
  };
  h.read.mockResolvedValue(idle);
  await h.recovery.connect();
  await h.recovery.recover();
  for (const bad of [
    { ...idle, migrationId: "bad" },
    { ...idle, from: 1 },
    { ...idle, current: 1 },
  ]) {
    h.read.mockResolvedValue(bad);
    await h.recovery.recover();
    expect(h.state.error).toBeDefined();
  }
  h.read.mockResolvedValue(snapshot("a", "completed", 2));
  await h.recovery.recover();
  const running = snapshot("a", "running", 2);
  running.seq.done = 1;
  h.read.mockResolvedValue(running);
  await h.recovery.recover();
  expect(h.state.snapshot?.phase).toBe("completed");
  h.read.mockResolvedValue(snapshot("b", "failed", 2));
  await h.recovery.recover();
  const completed = snapshot("b", "completed", 2);
  completed.seq.failed = 1;
  h.read.mockResolvedValue(completed);
  await h.recovery.recover();
  expect(h.state.snapshot?.phase).toBe("failed");
});

it("caps flow hints while preserving the active flow high water mark", async () => {
  const h = harness();
  await h.recovery.connect();
  const first = deferred<MigrationSnapshot>(),
    second = deferred<MigrationSnapshot>();
  h.read
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise)
    .mockResolvedValue(snapshot("a", "running", 100));
  const ready = h.recovery.recover();
  await vi.waitFor(() => expect(h.read).toHaveBeenCalledTimes(2));
  h.recovery.receive({ kind: "progress", seq: 100, migrationId: "a" });
  for (let index = 0; index < 40; index++)
    h.recovery.receive({ kind: "progress", seq: 1, migrationId: `other-${index}` });
  first.resolve(snapshot("a", "running", 1));
  await vi.waitFor(() => expect(h.read).toHaveBeenCalledTimes(3));
  h.recovery.receive({ kind: "failed", seq: 1, migrationId: "late" });
  second.resolve(snapshot("a", "running", 100));
  await ready;
  expect(h.state.snapshot?.migrationId).toBe("a");
  expect(h.state.needsRecovery).toBe(false);
});
it("retains only sixteen old flow identities, ignores duplicate pending hints and stale failures", async () => {
  const h = harness();
  await h.recovery.connect();
  for (let n = 0; n < 18; n++) {
    h.read.mockResolvedValue(snapshot(`flow-${n}`));
    await h.recovery.recover();
  }
  const failing = deferred<MigrationSnapshot>();
  h.read.mockReturnValueOnce(failing.promise);
  const old = h.recovery.recover();
  await Promise.resolve();
  h.recovery.receive({ kind: "progress", seq: 2, migrationId: "flow-17" });
  h.recovery.receive({ kind: "progress", seq: 2, migrationId: "flow-17" });
  h.recovery.disconnect();
  failing.reject(new Error("old"));
  await old;
  expect(h.state.error).toBeUndefined();
  const waiting = deferred<MigrationSnapshot>();
  h.read.mockReturnValueOnce(waiting.promise);
  const pending = h.recovery.connect();
  await vi.waitFor(() => expect(h.state.recovering).toBe(true));
  h.recovery.disconnect();
  const newer = h.recovery.connect();
  h.recovery.disconnect();
  waiting.reject(new Error("previous"));
  await pending;
  await newer;
  expect(h.state.recovering).toBe(false);
});
