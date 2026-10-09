import { effectScope, ref, readonly } from "vue";
import { expect, it, vi } from "vitest";
import type { PhaseSnapshot } from "../../../src-web/api/engine";
import { useGameControls } from "../../../src-web/composables/useGameControls";
import { snapshot } from "../utils/phase-fixture";

function deferred() {
  let resolve!: (value: { operationId: string; roundId: string }) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<{ operationId: string; roundId: string }>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function transport() {
  return {
    resume: vi.fn().mockResolvedValue({}),
    cancelRound: vi.fn().mockResolvedValue({}),
    submitCheck: vi.fn().mockResolvedValue({}),
  };
}
const waiting = snapshot({
  inFlight: { operationId: "op", roundId: "round" },
  check: {
    planId: "plan",
    status: "waiting",
    mode: "manual",
    ruleId: "rule",
    actorId: "actor",
    expression: "2d6",
    modifierTotal: 0,
  },
});
it("accepts readonly consumers and sends only controls with a confirmed round and plan", async () => {
  const state = ref<PhaseSnapshot>(),
    api = transport(),
    scope = effectScope();
  const controls = scope.run(() => useGameControls(readonly(state), api))!;
  await controls.control("resume");
  state.value = snapshot();
  await controls.control("cancel");
  await controls.control("check");
  state.value = snapshot({ inFlight: { operationId: "op" } });
  await controls.control("cancel");
  await controls.control("check");
  state.value = snapshot({ inFlight: { operationId: "op", roundId: "round" } });
  await controls.control("check");
  expect(api.submitCheck).not.toHaveBeenCalled();
  state.value = waiting;
  await controls.control("check");
  await controls.control("cancel");
  await controls.control("resume");
  expect(api.submitCheck).toHaveBeenCalledExactlyOnceWith(waiting.sessionId, "round", "plan");
  expect(api.cancelRound).toHaveBeenCalledExactlyOnceWith(waiting.sessionId, "round");
  expect(api.resume).toHaveBeenCalledExactlyOnceWith(waiting.sessionId);
  scope.stop();
  await controls.control("resume");
  expect(api.resume).toHaveBeenCalledTimes(1);
});
it("serializes controls and shows sanitized errors for the current revision", async () => {
  const pending = deferred(),
    api = transport(),
    scope = effectScope();
  api.resume.mockReturnValue(pending.promise);
  const controls = scope.run(() => useGameControls(waiting, api))!;
  const sending = controls.control("resume");
  await controls.control("check");
  expect(api.submitCheck).not.toHaveBeenCalled();
  pending.reject(new Error("secret"));
  await sending;
  expect(controls.error.value?.message).not.toContain("secret");
  expect(controls.busy.value).toBe(false);
  api.resume.mockResolvedValue({});
  await controls.control("resume");
  expect(controls.error.value).toBeUndefined();
  scope.stop();
});
it.each(["session", "epoch", "revision", "missing", "disposed"])(
  "discards a late error after %s changes",
  async (change) => {
    const pending = deferred(),
      api = transport(),
      scope = effectScope(),
      state = ref<PhaseSnapshot | undefined>(waiting);
    api.resume.mockReturnValue(pending.promise);
    const controls = scope.run(() => useGameControls(state, api))!;
    const sending = controls.control("resume");
    if (change === "session") state.value = snapshot({ sessionId: "new" });
    else if (change === "epoch") state.value = snapshot({ stateEpoch: "new" });
    else if (change === "revision") state.value = snapshot({ phaseRevision: 1 });
    else if (change === "missing") state.value = undefined;
    else scope.stop();
    pending.reject(new Error("stale"));
    await sending;
    expect(controls.error.value).toBeUndefined();
    scope.stop();
  },
);
it("does not mutate busy state when a successful request arrives after disposal", async () => {
  const pending = deferred(),
    api = transport(),
    scope = effectScope();
  api.resume.mockReturnValue(pending.promise);
  const controls = scope.run(() => useGameControls(waiting, api))!;
  const sending = controls.control("resume");
  scope.stop();
  pending.resolve({ operationId: "op", roundId: "round" });
  await sending;
  expect(controls.busy.value).toBe(true);
});
