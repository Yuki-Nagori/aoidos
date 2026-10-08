import { expect, it, vi } from "vitest";
import { createListenerGroup } from "../../../src-web/utils/listener-group";

it("owns each registration separately and closes idempotently", async () => {
  const group = createListenerGroup();
  const off = vi.fn();
  await Promise.all([group.register(async () => off), group.register(async () => off)]);
  group.close();
  group.close();
  expect(off).toHaveBeenCalledTimes(2);
  await group.register(async () => off);
  expect(off).toHaveBeenCalledTimes(3);
});

it("one registration failure releases successes and any late success", async () => {
  const group = createListenerGroup();
  const off = vi.fn();
  await group.register(async () => off);
  let resolve!: (value: () => void) => void;
  const late = group.register(
    () =>
      new Promise<() => void>((yes) => {
        resolve = yes;
      }),
  );
  const error = new Error("registration failed");
  await expect(group.register(() => Promise.reject(error))).rejects.toBe(error);
  expect(off).toHaveBeenCalledTimes(1);
  resolve(off);
  await late;
  expect(off).toHaveBeenCalledTimes(2);
});
