import { effectScope, shallowRef } from "vue";
import { describe, expect, it, vi } from "vitest";
import type { SceneHint } from "../../../src-web/api/engine";
import { useSceneHint } from "../../../src-web/composables/useSceneHint";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((accept) => {
    resolve = accept;
  });
  return { promise, resolve };
}

function phase(sessionId: string, sceneId = "entry", historyRevision = 0) {
  return {
    sessionId,
    scene: { sceneId },
    historyRevision,
    needsRecovery: false,
  };
}

const hint: SceneHint = {
  hintId: "door-clue",
  text: "石门的边缘有新鲜的刮痕。",
  targetScope: "player",
};

describe("useSceneHint", () => {
  it("claims only on explicit requests and keeps each returned hint", async () => {
    const sessionId = shallowRef<string | undefined>("session-1");
    const current = shallowRef(phase("session-1"));
    const nextHint: SceneHint = { ...hint, hintId: "window-clue", text: "窗沿落着灰尘。" };
    const claim = vi.fn().mockResolvedValueOnce(hint).mockResolvedValueOnce(nextHint);
    const scope = effectScope();
    const state = scope.run(() => useSceneHint(sessionId, current, { claim }))!;

    expect(claim).not.toHaveBeenCalled();
    await state.request();
    await state.request();
    expect(claim).toHaveBeenCalledTimes(2);
    expect(state.hints.value).toEqual([hint, nextHint]);
    scope.stop();
  });

  it("discards a response after the active history changes", async () => {
    const response = deferred<SceneHint | null>();
    const currentResponse = deferred<SceneHint | null>();
    const sessionId = shallowRef<string | undefined>("session-1");
    const current = shallowRef(phase("session-1"));
    const claim = vi
      .fn()
      .mockReturnValueOnce(response.promise)
      .mockReturnValueOnce(currentResponse.promise);
    const scope = effectScope();
    const state = scope.run(() => useSceneHint(sessionId, current, { claim }))!;

    const request = state.request();
    current.value = phase("session-1", "entry", 1);
    const currentRequest = state.request();
    currentResponse.resolve(hint);
    await currentRequest;
    response.resolve(hint);
    await request;
    expect(state.hints.value).toEqual([hint]);
    expect(state.unavailable.value).toBe(false);
    scope.stop();
  });

  it("reports an empty result and does not request when recovery is required", async () => {
    const sessionId = shallowRef<string | undefined>("session-1");
    const current = shallowRef({ ...phase("session-1"), needsRecovery: true });
    const claim = vi.fn(async () => null);
    const scope = effectScope();
    const state = scope.run(() => useSceneHint(sessionId, current, { claim }))!;

    await state.request();
    expect(claim).not.toHaveBeenCalled();
    current.value = phase("session-1");
    await state.request();
    expect(claim).toHaveBeenCalledOnce();
    expect(state.unavailable.value).toBe(true);
    scope.stop();
  });
});
