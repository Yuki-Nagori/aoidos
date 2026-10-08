import { effectScope } from "vue";
import { expect, it, vi } from "vitest";
import { useMigration } from "../../../src-web/composables/useMigration";
import type { MigrationSnapshot } from "../../../src-web/api/store";
function snapshot(): MigrationSnapshot {
  return {
    migrationId: "flow",
    phase: "completed",
    from: 0,
    to: 1,
    current: 1,
    seq: { progress: 1, done: 1, failed: 0 },
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
it("all listeners register first, early events recover and scope disposal frees all", async () => {
  const pending = Array.from({ length: 3 }, () => deferred<() => void>());
  const releases = Array.from({ length: 3 }, () => vi.fn());
  const handlers: ((event: { seq: number; data: { migrationId: string } }) => void)[] = [];
  const listen = vi.fn(
    (_kind: string, callback: (event: { seq: number; data: { migrationId: string } }) => void) => {
      handlers.push(callback);
      return pending[handlers.length - 1]!.promise;
    },
  );
  const read = vi.fn().mockResolvedValue(snapshot());
  const scope = effectScope();
  const consumer = scope.run(() => useMigration({ listen, read }))!;
  await vi.waitFor(() => expect(listen).toHaveBeenCalledTimes(3));
  handlers[0]!({ seq: 1, data: { migrationId: "flow" } });
  pending[0]!.resolve(releases[0]!);
  pending[1]!.resolve(releases[1]!);
  await Promise.resolve();
  expect(read).not.toHaveBeenCalled();
  pending[2]!.resolve(releases[2]!);
  const connection = consumer.reconnect();
  expect(consumer.reconnect()).toBe(connection);
  await connection;
  expect(consumer.state.value.snapshot?.phase).toBe("completed");
  scope.stop();
  expect(releases.every((off) => off.mock.calls.length === 1)).toBe(true);
  handlers[2]!({ seq: 1, data: { migrationId: "flow" } });
  await consumer.reconnect();
});
it("partial failure cleans successful listeners and reconnect retries safely", async () => {
  const off = vi.fn();
  const read = vi.fn().mockResolvedValue(snapshot());
  const listen = vi.fn().mockResolvedValue(off).mockRejectedValueOnce(new Error("secret"));
  const scope = effectScope();
  const consumer = scope.run(() => useMigration({ listen, read }))!;
  await consumer.reconnect();
  expect(off).toHaveBeenCalledTimes(2);
  expect(consumer.connectionError.value?.code).toBe("app.event-failed");
  expect(read).not.toHaveBeenCalled();
  await consumer.reconnect();
  expect(consumer.state.value.snapshot?.phase).toBe("completed");
  scope.stop();
  expect(off).toHaveBeenCalledTimes(5);
});
it("late and never-started registrations are cleaned after disposal", async () => {
  const waits = Array.from({ length: 3 }, () => deferred<() => void>());
  const off = vi.fn();
  let count = 0;
  const transport = {
    read: vi.fn().mockResolvedValue(snapshot()),
    listen: vi.fn(() => waits[count++]!.promise),
  };
  const scope = effectScope();
  const consumer = scope.run(() => useMigration(transport))!;
  await vi.waitFor(() => expect(count).toBe(3));
  scope.stop();
  waits.forEach((wait) => wait.resolve(off));
  await Promise.resolve();
  await Promise.resolve();
  await consumer.reconnect();
  expect(off).toHaveBeenCalledTimes(3);
  const early = effectScope();
  const never = early.run(() => useMigration(transport))!;
  early.stop();
  await never.reconnect();
  expect(count).toBe(3);
});

it("disposal releases successful listeners while another registration remains pending", async () => {
  const wait = deferred<() => void>();
  const off = vi.fn();
  const late = vi.fn();
  const listen = vi.fn().mockResolvedValue(off).mockReturnValueOnce(wait.promise);
  const scope = effectScope();
  const consumer = scope.run(() =>
    useMigration({ listen, read: vi.fn().mockResolvedValue(snapshot()) }),
  )!;
  await vi.waitFor(() => expect(listen).toHaveBeenCalledTimes(3));
  await Promise.resolve();
  scope.stop();
  expect(off).toHaveBeenCalledTimes(2);
  wait.resolve(late);
  await consumer.reconnect();
  await vi.waitFor(() => expect(late).toHaveBeenCalledTimes(1));
  expect(off).toHaveBeenCalledTimes(2);
});
