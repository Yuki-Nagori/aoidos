import { onScopeDispose, readonly, shallowRef, toValue, watch, type MaybeRefOrGetter } from "vue";
import { getPhase, listenPhaseEvent, type PhaseEventMap, type PhaseState } from "../api/engine";
import type { TurnEnvelope } from "../api/llm";
import { createListenerGroup } from "../utils/listener-group";
import { phaseEventKeys } from "../utils/phase-consumer";
import { createPhaseRecovery, type PhaseRecoveryState } from "../utils/phase-recovery";

interface Transport {
  read: typeof getPhase;
  listen(
    name: keyof PhaseEventMap,
    handler: (event: TurnEnvelope<PhaseState>) => void,
  ): Promise<() => void>;
}
const defaults: Transport = { read: getPhase, listen: listenPhaseEvent };
/** scope 内先注册全部四类监听再读快照；切换会话与卸载废弃迟到响应。 */
export function useEnginePhase(
  sessionId: MaybeRefOrGetter<string | undefined>,
  transport: Transport = defaults,
) {
  const state = shallowRef<PhaseRecoveryState>({ recovering: false, needsRecovery: false });
  const connecting = shallowRef(false);
  const connectionError = shallowRef<{ code: string; message: string }>();
  const recovery = createPhaseRecovery(transport.read, (next) => {
    state.value = next;
  });
  let disposed = false,
    epoch = 0;
  let listeners: ReturnType<typeof createListenerGroup> | undefined;
  let pending: Promise<void> | undefined;
  function reconnect(): Promise<void> {
    if (disposed) return Promise.resolve();
    if (pending) return pending;
    const token = ++epoch;
    pending = Promise.resolve().then(async () => {
      if (disposed || token !== epoch) return;
      try {
        const group = createListenerGroup();
        listeners = group;
        for (const name of Object.keys(phaseEventKeys) as (keyof PhaseEventMap)[]) {
          await group.register(() =>
            transport.listen(name, (envelope) => {
              if (!disposed && token === epoch) recovery.receive({ name, envelope });
            }),
          );
          if (disposed || token !== epoch) return;
        }
        connecting.value = false;
        await recovery.connect();
      } catch {
        if (!disposed && token === epoch)
          connectionError.value = {
            code: "app.event-failed",
            message: "阶段监听注册失败，可重新连接",
          };
      } finally {
        if (!disposed && token === epoch) {
          pending = undefined;
          connecting.value = false;
        }
      }
    });
    recovery.disconnect();
    listeners?.close();
    listeners = undefined;
    connecting.value = true;
    connectionError.value = undefined;
    return pending;
  }
  watch(
    () => toValue(sessionId),
    (id) => recovery.select(id),
    { immediate: true, flush: "sync" },
  );
  onScopeDispose(() => {
    disposed = true;
    epoch++;
    connecting.value = false;
    recovery.disconnect();
    listeners?.close();
    listeners = undefined;
  });
  void reconnect();
  return {
    state: readonly(state),
    connecting: readonly(connecting),
    connectionError: readonly(connectionError),
    recover: recovery.recover,
    reconnect,
  };
}
