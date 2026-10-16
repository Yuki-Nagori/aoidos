import { effectScope, shallowRef } from "vue";
import { describe, expect, it, vi } from "vitest";
import type { ScenePresenceSnapshot } from "../../../src-web/api/engine";
import { useScenePresence } from "../../../src-web/composables/useScenePresence";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((accept, fail) => {
    resolve = accept;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function snapshot(
  sessionId: string,
  sceneId = "entry",
  presenceRevision = "sha256:revision-1",
): ScenePresenceSnapshot {
  return {
    sessionId,
    sceneId,
    presenceRevision,
    characters: [{ id: "keeper", kind: "character", scope: "global", canonicalName: "Mira" }],
  };
}

function phase(
  sessionId: string,
  phaseRevision = 1,
  historyRevision = 0,
  needsRecovery = false,
  sceneId = "entry",
) {
  return { sessionId, scene: { sceneId }, phaseRevision, historyRevision, needsRecovery };
}

describe("useScenePresence", () => {
  it("loads the active roster once per phase revision and clears stale recipient selection", async () => {
    const sessionId = shallowRef<string | undefined>("session-1");
    const current = shallowRef(phase("session-1"));
    const read = vi.fn(async () => snapshot("session-1"));
    const scope = effectScope();
    const state = scope.run(() => useScenePresence(sessionId, current, { read }))!;

    await vi.waitFor(() => expect(state.snapshot.value?.characters[0]?.canonicalName).toBe("Mira"));
    state.select("keeper");
    expect(state.selectedCharacterId.value).toBe("keeper");

    current.value = phase("session-1", 1, 1);
    expect(read).toHaveBeenCalledOnce();

    current.value = phase("session-1", 2, 1);
    expect(state.selectedCharacterId.value).toBe("");
    await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2));
    expect(state.snapshot.value?.presenceRevision).toBe("sha256:revision-1");
    scope.stop();
  });

  it("discards a late response from an old session", async () => {
    const first = deferred<ScenePresenceSnapshot>();
    const sessionId = shallowRef<string | undefined>("session-1");
    const current = shallowRef(phase("session-1"));
    const read = vi.fn((id: string) =>
      id === "session-1" ? first.promise : Promise.resolve(snapshot(id, "entry", "sha256:two")),
    );
    const scope = effectScope();
    const state = scope.run(() => useScenePresence(sessionId, current, { read }))!;

    sessionId.value = "session-2";
    current.value = phase("session-2");
    first.resolve(snapshot("session-1"));
    await vi.waitFor(() => expect(state.snapshot.value?.sessionId).toBe("session-2"));
    expect(state.snapshot.value?.sessionId).toBe("session-2");
    scope.stop();
  });

  it("discards a late response from the previous scene", async () => {
    const first = deferred<ScenePresenceSnapshot>();
    const sessionId = shallowRef<string | undefined>("session-1");
    const current = shallowRef(phase("session-1"));
    const read = vi
      .fn()
      .mockReturnValueOnce(first.promise)
      .mockResolvedValue(snapshot("session-1", "next-scene", "sha256:next"));
    const scope = effectScope();
    const state = scope.run(() => useScenePresence(sessionId, current, { read }))!;

    current.value = phase("session-1", 2, 1, false, "next-scene");
    first.resolve(snapshot("session-1"));
    await vi.waitFor(() => expect(state.snapshot.value?.sceneId).toBe("next-scene"));
    expect(state.snapshot.value?.sceneId).toBe("next-scene");
    scope.stop();
  });

  it("coalesces phase invalidations behind one pending read", async () => {
    const first = deferred<ScenePresenceSnapshot>();
    const sessionId = shallowRef<string | undefined>("session-1");
    const current = shallowRef(phase("session-1"));
    const read = vi
      .fn()
      .mockReturnValueOnce(first.promise)
      .mockResolvedValue(snapshot("session-1", "entry", "sha256:latest"));
    const scope = effectScope();
    const state = scope.run(() => useScenePresence(sessionId, current, { read }))!;

    current.value = phase("session-1", 2, 1);
    current.value = phase("session-1", 3, 2);
    expect(read).toHaveBeenCalledTimes(1);
    first.resolve(snapshot("session-1"));
    await vi.waitFor(() => expect(state.snapshot.value?.presenceRevision).toBe("sha256:latest"));
    expect(read).toHaveBeenCalledTimes(2);
    scope.stop();
  });

  it("does not expose a roster while the session needs recovery and supports explicit retry", async () => {
    const sessionId = shallowRef<string | undefined>("session-1");
    const current = shallowRef(phase("session-1", 1, 0, true));
    const read = vi.fn(async () => snapshot("session-1"));
    const scope = effectScope();
    const state = scope.run(() => useScenePresence(sessionId, current, { read }))!;
    await Promise.resolve();
    expect(read).not.toHaveBeenCalled();
    expect(state.snapshot.value).toBeUndefined();

    current.value = phase("session-1");
    await vi.waitFor(() => expect(state.snapshot.value?.sessionId).toBe("session-1"));
    expect(read).toHaveBeenCalledOnce();
    scope.stop();
  });

  it("fails closed on snapshot errors and reloads only after an explicit retry", async () => {
    const sessionId = shallowRef<string | undefined>("session-1");
    const current = shallowRef(phase("session-1"));
    const read = vi
      .fn()
      .mockRejectedValueOnce(new Error("private storage detail"))
      .mockResolvedValue(snapshot("session-1"));
    const scope = effectScope();
    const state = scope.run(() => useScenePresence(sessionId, current, { read }))!;

    await vi.waitFor(() => expect(state.unavailable.value).toBe(true));
    expect(state.snapshot.value).toBeUndefined();
    state.refresh();
    await vi.waitFor(() => expect(state.snapshot.value?.sessionId).toBe("session-1"));
    expect(read).toHaveBeenCalledTimes(2);
    scope.stop();
  });
});
