import type { MaybeRefOrGetter } from "vue";
import { onScopeDispose, readonly, ref, shallowRef, toValue, watch } from "vue";
import type { ScenePresenceSnapshot } from "../../../api/engine";
import { getScenePresence } from "../../../api/engine";

interface PresencePhase {
  sessionId: string;
  scene?: { sceneId: string };
  phaseRevision: number;
  needsRecovery: boolean;
}
interface RequestKey {
  sessionId: string;
  sceneId: string;
}
interface Transport {
  read: typeof getScenePresence;
}
const defaults: Transport = { read: getScenePresence };

/** 只消费 Rust 快照；阶段事件触发有界刷新，旧会话 / 场景响应直接丢弃。 */
export function useScenePresence(
  sessionId: MaybeRefOrGetter<string | undefined>,
  phase: MaybeRefOrGetter<PresencePhase | undefined>,
  transport: Transport = defaults,
) {
  const snapshot = shallowRef<ScenePresenceSnapshot>();
  const selectedCharacterId = ref("");
  const loading = ref(false);
  const unavailable = ref(false);
  let disposed = false;
  let generation = 0;
  let desired: RequestKey | undefined;
  let running = false;
  let refreshAgain = false;

  function currentKey(): RequestKey | undefined {
    const id = toValue(sessionId);
    const state = toValue(phase);
    if (!id || !state || state.sessionId !== id || !state.scene || state.needsRecovery)
      return undefined;
    return { sessionId: id, sceneId: state.scene.sceneId };
  }

  function matchesSnapshot(value: ScenePresenceSnapshot, key: RequestKey): boolean {
    return (
      value.sessionId === key.sessionId &&
      value.sceneId === key.sceneId &&
      value.characters.length <= 512 &&
      new Set(value.characters.map(({ id }) => id)).size === value.characters.length
    );
  }

  async function drain(): Promise<void> {
    if (running || disposed) {
      refreshAgain = true;
      return;
    }
    running = true;
    do {
      refreshAgain = false;
      const key = desired;
      const token = generation;
      if (!key) break;
      loading.value = true;
      try {
        const value = await transport.read(key.sessionId);
        if (disposed || token !== generation || currentKey()?.sessionId !== key.sessionId) continue;
        if (!matchesSnapshot(value, key)) {
          snapshot.value = undefined;
          selectedCharacterId.value = "";
          unavailable.value = true;
          continue;
        }
        if (
          snapshot.value?.sessionId !== value.sessionId ||
          snapshot.value.sceneId !== value.sceneId ||
          snapshot.value.presenceRevision !== value.presenceRevision
        ) {
          selectedCharacterId.value = "";
        }
        snapshot.value = value;
        unavailable.value = false;
      } catch {
        if (!disposed && token === generation) {
          snapshot.value = undefined;
          selectedCharacterId.value = "";
          unavailable.value = true;
        }
      } finally {
        if (!disposed && token === generation) loading.value = false;
      }
    } while (refreshAgain && !disposed);
    running = false;
    if (refreshAgain && !disposed) void drain();
  }

  function schedule(key: RequestKey | undefined): void {
    generation++;
    desired = key;
    selectedCharacterId.value = "";
    snapshot.value = undefined;
    unavailable.value = false;
    if (!key) {
      loading.value = false;
      return;
    }
    void drain();
  }

  watch(
    () => {
      const key = currentKey();
      const state = toValue(phase);
      return [
        key?.sessionId ?? "",
        key?.sceneId ?? "",
        state?.phaseRevision ?? -1,
        state?.needsRecovery ?? true,
      ].join("\u001f");
    },
    () => schedule(currentKey()),
    { immediate: true, flush: "sync" },
  );

  function select(characterId: string): void {
    selectedCharacterId.value =
      characterId && snapshot.value?.characters.some(({ id }) => id === characterId)
        ? characterId
        : "";
  }
  function refresh(): void {
    schedule(currentKey());
  }
  onScopeDispose(() => {
    disposed = true;
    generation++;
    desired = undefined;
    snapshot.value = undefined;
    selectedCharacterId.value = "";
  });

  return {
    snapshot: readonly(snapshot),
    selectedCharacterId: readonly(selectedCharacterId),
    loading: readonly(loading),
    unavailable: readonly(unavailable),
    select,
    refresh,
  };
}
