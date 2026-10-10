import {
  computed,
  onScopeDispose,
  readonly,
  shallowRef,
  toValue,
  watch,
  type MaybeRefOrGetter,
} from "vue";
import { getRecordView, getRecordPage, getRecordBody, listenRecords } from "../api/records";
import {
  createRecordRecovery,
  recordItems,
  type RecordState,
  type RecordTransport,
} from "../utils/record-recovery";

import { createListenerGroup } from "../utils/listener-group";

type Transport = RecordTransport & { listen: typeof listenRecords };
const defaults: Transport = {
  view: getRecordView,
  page: getRecordPage,
  body: getRecordBody,
  listen: listenRecords,
};

/** setup / effectScope 内使用；窗口、历史页和正文缓存均按组件独立且有界。 */
export function useRecordView(
  sessionId: MaybeRefOrGetter<string | undefined>,
  transport: Transport = defaults,
) {
  const state = shallowRef<RecordState>({
    pages: [],
    historyStale: false,
    recovering: false,
    needsRecovery: false,
  });
  const connecting = shallowRef(false);
  const connectionError = shallowRef<{ code: string; message: string }>();
  const recovery = createRecordRecovery(transport, (next) => {
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
    const request = Promise.resolve().then(async () => {
      if (disposed || token !== epoch) return;
      try {
        const group = createListenerGroup();
        listeners = group;
        await group.register(() =>
          transport.listen((event) => {
            if (!disposed && token === epoch) recovery.receive(event);
          }),
        );
        if (disposed || token !== epoch) return;
        connecting.value = false;
        await recovery.connect();
      } catch {
        if (!disposed && token === epoch)
          connectionError.value = {
            code: "app.event-failed",
            message: "记录监听注册失败，可重新连接",
          };
      } finally {
        if (!disposed && token === epoch) {
          pending = undefined;
          connecting.value = false;
        }
      }
    });
    pending = request;
    recovery.disconnect();
    listeners?.close();
    listeners = undefined;
    connecting.value = true;
    connectionError.value = undefined;
    return request;
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
    items: computed(() => recordItems(state.value)),
    state: readonly(state),
    connecting: readonly(connecting),
    connectionError: readonly(connectionError),
    recover: recovery.recover,
    reconnect,
    loadPage: recovery.loadPage,
    loadBody: recovery.loadBody,
    clearBody: recovery.clearBody,
  };
}
