import { onScopeDispose, readonly, shallowRef } from "vue";
import { getMigration, listenMigration } from "../api/store";
import type { MigrationSnapshot } from "../api/store";
import { createMigrationRecovery, type MigrationState } from "../utils/migration-recovery";
import { createListenerGroup } from "../utils/listener-group";

const defaults = { read: getMigration, listen: listenMigration };
interface Transport {
  read(): Promise<MigrationSnapshot>;
  listen(
    kind: "progress" | "done" | "failed",
    handler: (event: { seq: number; data: { migrationId: string } }) => void,
  ): Promise<() => void>;
}
/** 活动 scope 内先注册三类监听，再读迁移快照；卸载和失败注册均释放监听。 */
export function useMigration(transport: Transport = defaults) {
  const state = shallowRef<MigrationState>({ recovering: false, needsRecovery: false });
  const connecting = shallowRef(false);
  const connectionError = shallowRef<{ code: string; message: string }>();
  const recovery = createMigrationRecovery(
    () => transport.read(),
    (next) => {
      state.value = next;
    },
  );
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
      const group = createListenerGroup();
      listeners = group;
      const registered = await Promise.allSettled(
        (["progress", "done", "failed"] as const).map((kind) =>
          group.register(() =>
            transport.listen(kind, (event) => {
              if (!disposed && token === epoch)
                recovery.receive({ kind, seq: event.seq, migrationId: event.data.migrationId });
            }),
          ),
        ),
      );
      if (
        disposed ||
        token !== epoch ||
        registered.some((result) => result.status === "rejected")
      ) {
        group.close();
        if (token === epoch) listeners = undefined;
        if (!disposed && token === epoch)
          connectionError.value = {
            code: "app.event-failed",
            message: "迁移监听注册失败，可重新连接",
          };
      } else {
        connecting.value = false;
        await recovery.connect();
      }
      if (!disposed && token === epoch) {
        pending = undefined;
        connecting.value = false;
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
