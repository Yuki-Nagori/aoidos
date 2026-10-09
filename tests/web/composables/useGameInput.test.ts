import { effectScope, ref } from "vue";
import { expect, it, vi } from "vitest";
import { useGameInput } from "../../../src-web/composables/useGameInput";

function deferred() {
  let resolve!: (value: { operationId: string; roundId: string }) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<{ operationId: string; roundId: string }>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const accepted = { operationId: "op", roundId: "round" };
it("submits the original draft once, preserving IME and newly edited text", async () => {
  const pending = deferred(),
    submit = vi.fn().mockReturnValue(pending.promise);
  const id = ref<string | undefined>();
  const scope = effectScope(),
    input = scope.run(() => useGameInput(id, submit))!;
  await input.send();
  id.value = "session";
  await input.send();
  input.draft.value = " \n ";
  await input.send();
  input.draft.value = "  探索地窖  ";
  input.composing.value = true;
  await input.send();
  expect(submit).not.toHaveBeenCalled();
  input.composing.value = false;
  const sending = input.send();
  await input.send();
  expect(submit).toHaveBeenCalledExactlyOnceWith("session", "  探索地窖  ");
  input.draft.value = "下一条";
  pending.resolve(accepted);
  await sending;
  expect(input.draft.value).toBe("下一条");
  submit.mockResolvedValue(accepted);
  await input.send();
  expect(input.draft.value).toBe("");
  scope.stop();
  input.draft.value = "卸载草稿";
  await input.send();
  expect(submit).toHaveBeenCalledTimes(2);
});
it("keeps failed drafts and sanitizes errors, discarding responses for another session", async () => {
  const pending = deferred(),
    submit = vi.fn().mockRejectedValue(new Error("secret"));
  const id = ref<string | undefined>("one");
  const scope = effectScope(),
    input = scope.run(() => useGameInput(() => id.value, submit))!;
  input.draft.value = "保留";
  await input.send();
  expect(input.error.value?.message).not.toContain("secret");
  expect(input.draft.value).toBe("保留");
  submit.mockReturnValue(pending.promise);
  const sending = input.send();
  expect(input.error.value).toBeUndefined();
  id.value = "two";
  pending.reject(new Error("old"));
  await sending;
  expect(input.error.value).toBeUndefined();
  const late = deferred();
  submit.mockReturnValue(late.promise);
  const sent = input.send();
  id.value = undefined;
  late.resolve(accepted);
  await sent;
  expect(input.draft.value).toBe("保留");
  scope.stop();
});
it.each(["resolve", "reject"] as const)("ignores %s after disposal", async (result) => {
  const pending = deferred(),
    scope = effectScope();
  const input = scope.run(() => useGameInput("session", vi.fn().mockReturnValue(pending.promise)))!;
  input.draft.value = "保留";
  const sending = input.send();
  scope.stop();
  if (result === "resolve") pending.resolve(accepted);
  else pending.reject(new Error("late"));
  await sending;
  expect(input.draft.value).toBe("保留");
  expect(input.error.value).toBeUndefined();
});
