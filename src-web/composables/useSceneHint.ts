import type { MaybeRefOrGetter } from "vue";
import { onScopeDispose, readonly, shallowRef, toValue, watch } from "vue";
import type { SceneHint } from "../api/engine";
import { claimSceneHint } from "../api/engine";

interface HintPhase {
  sessionId: string;
  scene?: { sceneId: string };
  historyRevision: number;
  needsRecovery: boolean;
}
interface HintKey {
  sessionId: string;
  sceneId: string;
  historyRevision: number;
}
interface Transport {
  claim: typeof claimSceneHint;
}
const defaults: Transport = { claim: claimSceneHint };

/** 只在玩家明确请求时领取提示；场景或历史变化会丢弃旧响应，不主动轮询。 */
export function useSceneHint(
  sessionId: MaybeRefOrGetter<string | undefined>,
  phase: MaybeRefOrGetter<HintPhase | undefined>,
  transport: Transport = defaults,
) {
  const hints = shallowRef<SceneHint[]>([]);
  const loading = shallowRef(false);
  const unavailable = shallowRef(false);
  const failed = shallowRef(false);
  let generation = 0;
  let disposed = false;
  let pendingGeneration: number | undefined;

  function currentKey(): HintKey | undefined {
    const id = toValue(sessionId);
    const state = toValue(phase);
    if (!id || !state || state.sessionId !== id || !state.scene || state.needsRecovery)
      return undefined;
    return {
      sessionId: id,
      sceneId: state.scene.sceneId,
      historyRevision: state.historyRevision,
    };
  }

  function isCurrent(key: HintKey, token: number): boolean {
    const current = currentKey();
    return (
      !disposed &&
      token === generation &&
      current?.sessionId === key.sessionId &&
      current.sceneId === key.sceneId &&
      current.historyRevision === key.historyRevision
    );
  }

  async function request(): Promise<void> {
    const key = currentKey();
    if (!key || pendingGeneration === generation) return;
    const token = generation;
    pendingGeneration = token;
    loading.value = true;
    unavailable.value = false;
    failed.value = false;
    try {
      const result = await transport.claim(key.sessionId);
      if (!isCurrent(key, token)) return;
      if (result && !hints.value.some((hint) => hint.hintId === result.hintId)) {
        hints.value = [...hints.value, result];
      }
      unavailable.value = result === null;
    } catch {
      if (isCurrent(key, token)) failed.value = true;
    } finally {
      if (pendingGeneration === token) {
        pendingGeneration = undefined;
        if (isCurrent(key, token)) loading.value = false;
      }
    }
  }

  watch(
    () => {
      const key = currentKey();
      return [key?.sessionId ?? "", key?.sceneId ?? "", key?.historyRevision ?? -1].join("\u001f");
    },
    () => {
      generation += 1;
      pendingGeneration = undefined;
      hints.value = [];
      loading.value = false;
      unavailable.value = false;
      failed.value = false;
    },
    { immediate: true, flush: "sync" },
  );
  onScopeDispose(() => {
    disposed = true;
    generation += 1;
    hints.value = [];
  });

  return {
    hints: readonly(hints),
    loading: readonly(loading),
    unavailable: readonly(unavailable),
    failed: readonly(failed),
    request,
  };
}
