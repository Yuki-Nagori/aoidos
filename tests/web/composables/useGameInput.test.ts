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
  input.draft.value = "two 的草稿";
  const late = deferred();
  submit.mockReturnValue(late.promise);
  const sent = input.send();
  id.value = undefined;
  late.resolve(accepted);
  await sent;
  expect(input.draft.value).toBe("");
  id.value = "one";
  expect(input.draft.value).toBe("保留");
  input.draft.value = "one 的新草稿";
  id.value = "two";
  expect(input.draft.value).toBe("two 的草稿");
  scope.stop();
});
it("keeps only five bounded session drafts and blocks input beyond Rust's UTF-8 limit", async () => {
  const id = ref("session-0");
  const submit = vi.fn().mockResolvedValue(accepted);
  const scope = effectScope();
  const input = scope.run(() => useGameInput(id, submit))!;

  input.draft.value = "first";
  for (let index = 1; index < 6; index++) {
    id.value = `session-${index}`;
    input.draft.value = `draft-${index}`;
  }
  id.value = "session-0";
  expect(input.draft.value).toBe("");
  id.value = "session-5";
  expect(input.draft.value).toBe("draft-5");

  input.draft.value = "😀".repeat(8193);
  expect(input.tooLarge.value).toBe(true);
  await input.send();
  expect(submit).not.toHaveBeenCalled();
  scope.stop();
});
it("clears an accepted command only for the same session generation and unchanged draft", () => {
  const id = ref("one");
  const scope = effectScope();
  const input = scope.run(() => useGameInput(id, vi.fn()))!;
  input.draft.value = "/stop";
  const ticket = input.captureDraft();

  id.value = "two";
  input.draft.value = "/stop";
  input.clearDraftIfCurrent(ticket);
  expect(input.draft.value).toBe("/stop");

  id.value = "one";
  input.draft.value = "/stop";
  input.clearDraftIfCurrent(ticket);
  expect(input.draft.value).toBe("/stop");

  const current = input.captureDraft();
  input.draft.value = "new text";
  input.clearDraftIfCurrent(current);
  expect(input.draft.value).toBe("new text");
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

it("clears a matching accepted command ticket and removes its saved session draft", () => {
  const id = ref("one");
  const scope = effectScope();
  const input = scope.run(() => useGameInput(id, vi.fn()))!;
  input.draft.value = "/stop";
  input.clearDraftIfCurrent(input.captureDraft());
  expect(input.draft.value).toBe("");
  id.value = "two";
  id.value = "one";
  expect(input.draft.value).toBe("");
  input.draft.value = "/resume";
  const ticket = input.captureDraft();
  scope.stop();
  input.clearDraftIfCurrent(ticket);
  expect(input.draft.value).toBe("/resume");
});
