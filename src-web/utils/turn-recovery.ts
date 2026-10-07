import { type TurnSnapshot } from "../api/llm";
import {
  consumeTurnEvent,
  consumeTurnSnapshot,
  coversSequences,
  validTurnEvent,
  type TurnNotification,
} from "./turn-consumer";

/** 网络恢复错误与回合失败分开，恢复失败不清掉已确认正文。 */
export interface TurnRecoveryView {
  snapshot?: TurnSnapshot;
  recovering: boolean;
  needsRecovery: boolean;
  recoveryError?: { code: string; message: string };
}

/** 只消费稳定错误字段，未知异常不展示原始字符串或 detail。 */
export function turnRecoveryError(error: unknown): { code: string; message: string } {
  if (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    "message" in error &&
    typeof error.code === "string" &&
    typeof error.message === "string"
  )
    return { code: error.code, message: error.message };
  return { code: "app.event-failed", message: "回合快照恢复失败" };
}

/** 注入快照读取和状态发布；此层不引用 Vue、Tauri 或持续计时器。 */
export function createTurnRecovery(ports: {
  getSnapshot(turnId: string): Promise<TurnSnapshot>;
  publish(view: TurnRecoveryView): void;
}): {
  select(turnId?: string): Promise<void>;
  connect(): Promise<void>;
  disconnect(): void;
  receive(event: TurnNotification): void;
  recover(): Promise<void>;
} {
  let turnId: string | undefined;
  let generation = 0;
  let connected = false;
  let pending: Promise<void> | undefined;
  const reads = new Map<string, Promise<TurnSnapshot>>();
  let cache: TurnNotification[] = [];
  let observed = { chunk: 0, done: 0, failed: 0 };
  let view: TurnRecoveryView = { recovering: false, needsRecovery: false };

  function publish(next: TurnRecoveryView): void {
    view = next;
    ports.publish(view);
  }
  function reset(): void {
    generation += 1;
    pending = undefined;
    cache = [];
    observed = view.snapshot ? { ...view.snapshot.seq } : { chunk: 0, done: 0, failed: 0 };
  }
  function enqueue(event: TurnNotification): void {
    if (cache.length === 32) cache = [];
    cache.push(event);
  }
  async function refresh(epoch: number, id: string): Promise<void> {
    // 普通补取一次；终态在补取发出后才到达时，再读一次终态后的快照。
    // 第三次仍落后则等待新事件或主动恢复，绝不变成后台轮询。
    let finalCatchup = false;
    for (let pass = 0; pass < 2 || finalCatchup; pass += 1) {
      if (epoch !== generation || !connected) return;
      const requestedTerminal = { done: observed.done, failed: observed.failed };
      try {
        const prior = reads.get(id);
        if (prior) {
          // invoke 无取消接口；同 UUID 等旧读完成，但不消费旧代次的数据或错误。
          await prior.then(
            () => undefined,
            () => undefined,
          );
          if (epoch !== generation || !connected) return;
        }
        if (reads.size === 32) {
          throw { code: "app.busy", message: "快照读取等待数已达上限" };
        }
        const response = ports.getSnapshot(id);
        reads.set(id, response);
        let candidate: TurnSnapshot;
        try {
          candidate = await response;
        } finally {
          reads.delete(id);
        }
        if (epoch !== generation || !connected) return;
        finalCatchup =
          pass === 1 &&
          (observed.done > requestedTerminal.done || observed.failed > requestedTerminal.failed);
        const accepted = candidate.turnId === id && consumeTurnSnapshot(view.snapshot, candidate);
        if (!accepted) {
          publish({
            ...view,
            needsRecovery: true,
            recoveryError: { code: "app.event-failed", message: "回合快照未追平已确认状态" },
          });
          continue;
        }
        let snapshot = accepted.snapshot;
        const remaining: TurnNotification[] = [];
        // 正文先按序合并，终态再检查 chunkSeq；独立事件计数不能相互排序。
        const chunks = cache
          .filter((event) => event.kind === "chunk")
          .sort((a, b) => a.envelope.seq - b.envelope.seq);
        const terminals = cache.filter((event) => event.kind !== "chunk");
        for (const event of [...chunks, ...terminals]) {
          if (event.kind !== "chunk" && observed.chunk > snapshot.seq.chunk) {
            remaining.push(event);
            continue;
          }
          const result = consumeTurnEvent(snapshot, event);
          if (result.needsRecovery) remaining.push(event);
          else snapshot = result.snapshot;
        }
        cache = remaining;
        const needsRecovery = cache.length > 0 || !coversSequences(snapshot.seq, observed);
        publish({ snapshot, recovering: true, needsRecovery });
        if (!needsRecovery) return;
      } catch (error) {
        if (epoch === generation && connected)
          publish({ ...view, needsRecovery: true, recoveryError: turnRecoveryError(error) });
        return;
      }
    }
  }
  function recover(): Promise<void> {
    if (!connected || turnId === undefined) return Promise.resolve();
    if (pending) return pending;
    const epoch = generation;
    const id = turnId;
    const request = Promise.resolve()
      .then(() => refresh(epoch, id))
      .finally(() => {
        if (epoch === generation) {
          pending = undefined;
          publish({ ...view, recovering: false });
        }
      });
    pending = request;
    publish({ ...view, recovering: true, needsRecovery: true, recoveryError: undefined });
    return request;
  }
  function receive(event: TurnNotification): void {
    if (turnId === undefined || event.envelope.data.turnId !== turnId) return;
    if (!validTurnEvent(event)) {
      publish({ ...view, needsRecovery: true });
      void recover();
      return;
    }
    if (view.snapshot && event.envelope.seq <= view.snapshot.seq[event.kind]) return;
    if (
      cache.some(
        (cached) => cached.kind === event.kind && cached.envelope.seq === event.envelope.seq,
      )
    )
      return;
    observed[event.kind] = Math.max(observed[event.kind], event.envelope.seq);
    if (event.kind !== "chunk")
      observed.chunk = Math.max(observed.chunk, event.envelope.data.chunkSeq);
    if (!connected || pending || view.snapshot === undefined || view.needsRecovery) {
      enqueue(event);
      void recover();
      return;
    }
    const result = consumeTurnEvent(view.snapshot, event);
    publish({ snapshot: result.snapshot, recovering: false, needsRecovery: result.needsRecovery });
    if (result.needsRecovery) {
      enqueue(event);
      void recover();
    }
  }
  return {
    select(id): Promise<void> {
      if (id === turnId) return Promise.resolve();
      turnId = id;
      view = { recovering: false, needsRecovery: id !== undefined };
      reset();
      publish(view);
      return recover();
    },
    connect(): Promise<void> {
      connected = true;
      return recover();
    },
    disconnect(): void {
      connected = false;
      reset();
      publish({ ...view, recovering: false, needsRecovery: turnId !== undefined });
    },
    receive,
    recover,
  };
}
