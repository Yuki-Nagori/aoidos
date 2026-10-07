import {
  onScopeDispose,
  readonly,
  shallowRef,
  toValue,
  watch,
  type DeepReadonly,
  type ShallowRef,
  type MaybeRefOrGetter,
} from "vue";
import { getTurn, listenTurnEvent } from "../api/llm";
import {
  createTurnRecovery,
  turnRecoveryError,
  type TurnRecoveryView,
} from "../utils/turn-recovery";

/** 平台端口只注入真实调用的替代点；消费规则不依赖平台对象。 */
export interface TurnTransport {
  getTurn: typeof getTurn;
  listen: typeof listenTurnEvent;
}
const defaultTransport: TurnTransport = { getTurn, listen: listenTurnEvent };

/**
 * 在 setup 或活动 effectScope 中调用，订阅不依赖挂载后的 DOM。
 * 先完成三类订阅再取快照，回合身份变化拒绝旧响应；卸载释放全部监听。
 * 重建窗口后传入已知 turnId 即可恢复；最后事件全部丢失时由 recover / reconnect 主动触发。
 */
export function useLlmTurn(
  turnId: MaybeRefOrGetter<string | undefined>,
  transport: TurnTransport = defaultTransport,
): {
  view: DeepReadonly<ShallowRef<TurnRecoveryView>>;
  connecting: DeepReadonly<ShallowRef<boolean>>;
  connectionError: DeepReadonly<ShallowRef<{ code: string; message: string } | undefined>>;
  recover(): Promise<void>;
  reconnect(): Promise<void>;
} {
  const view = shallowRef<TurnRecoveryView>({ recovering: false, needsRecovery: false });
  const connecting = shallowRef(false);
  const connectionError = shallowRef<{ code: string; message: string }>();
  const recovery = createTurnRecovery({
    getSnapshot: (id) => transport.getTurn(id),
    publish: (next) => {
      view.value = next;
    },
  });
  let disposed = false;
  let epoch = 0;
  let listeners: (() => void)[] = [];
  let registration: Promise<void> | undefined;
  function release(): void {
    for (const unlisten of listeners) unlisten();
    listeners = [];
  }
  function reconnect(): Promise<void> {
    if (disposed) return Promise.resolve();
    if (registration) return registration;
    const current = ++epoch;
    const operation = Promise.resolve()
      .then(async () => {
        if (disposed || current !== epoch) return;
        const forward = (event: import("../utils/turn-consumer").TurnNotification): void => {
          if (!disposed && current === epoch) recovery.receive(event);
        };
        const results = await Promise.allSettled([
          Promise.resolve().then(() =>
            transport.listen("llm:turn:chunk", (envelope) => forward({ kind: "chunk", envelope })),
          ),
          Promise.resolve().then(() =>
            transport.listen("llm:turn:done", (envelope) => forward({ kind: "done", envelope })),
          ),
          Promise.resolve().then(() =>
            transport.listen("llm:turn:failed", (envelope) =>
              forward({ kind: "failed", envelope }),
            ),
          ),
        ]);
        const successful = results
          .filter((result) => result.status === "fulfilled")
          .map((result) => result.value);
        if (disposed || current !== epoch) {
          for (const unlisten of successful) unlisten();
          return;
        }
        connecting.value = false;
        const failure = results.find((result) => result.status === "rejected");
        if (failure) {
          for (const unlisten of successful) unlisten();
          connectionError.value = turnRecoveryError(failure.reason);
          return;
        }
        listeners = successful;
        await recovery.connect();
      })
      .finally(() => {
        registration = undefined;
      });
    registration = operation;
    recovery.disconnect();
    release();
    connecting.value = true;
    connectionError.value = undefined;
    return operation;
  }
  onScopeDispose(() => {
    disposed = true;
    connecting.value = false;
    epoch += 1;
    recovery.disconnect();
    release();
  });
  watch(
    () => toValue(turnId),
    (id) => {
      void recovery.select(id);
    },
    { immediate: true, flush: "sync" },
  );
  void reconnect();
  return {
    view: readonly(view),
    connecting: readonly(connecting),
    connectionError: readonly(connectionError),
    recover: recovery.recover,
    reconnect,
  };
}
