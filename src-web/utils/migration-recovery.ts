import type { MigrationSnapshot } from "../api/store";
type Kind = "progress" | "done" | "failed";
export interface MigrationNotification {
  kind: Kind;
  seq: number;
  migrationId: string;
}
export interface MigrationState {
  snapshot?: MigrationSnapshot;
  recovering: boolean;
  needsRecovery: boolean;
  error?: { code: string; message: string };
}
const valid = (seq: number) => Number.isSafeInteger(seq) && seq >= 0;
/** 迁移流以当前快照为真相；事件投递错误与 SQL 成败分开消费。 */
export function createMigrationRecovery(
  read: () => Promise<MigrationSnapshot>,
  publish: (state: MigrationState) => void,
) {
  let connected = false,
    generation = 0;
  let pending: Promise<void> | undefined;
  let physical: Promise<MigrationSnapshot> | undefined;
  type Observed = { progress: number; done: number; failed: number; arrival: number };
  const observed = new Map<string, Observed>();
  let arrival = 0;
  let overflowArrival = 0;
  const retired: string[] = [];
  const state: MigrationState = { recovering: false, needsRecovery: false };
  const emit = () => publish({ ...state });
  const current = (token: number) => connected && generation === token;
  async function refresh(token: number) {
    try {
      let finalCatchup = false;
      for (let pass = 0; pass < 2 || finalCatchup; pass++) {
        if (physical) await physical.catch(() => undefined);
        if (!current(token)) return;
        let requestedArrival = arrival;
        const request = Promise.resolve().then(() => {
          requestedArrival = arrival;
          return read();
        });
        physical = request;
        let candidate: MigrationSnapshot;
        try {
          candidate = await request;
        } finally {
          physical = undefined;
        }
        if (!current(token)) return;
        const previous = state.snapshot;
        if (
          ![candidate.seq.progress, candidate.seq.done, candidate.seq.failed].every(valid) ||
          !valid(candidate.to) ||
          !["idle", "running", "completed", "failed"].includes(candidate.phase) ||
          (candidate.from !== undefined && !valid(candidate.from)) ||
          (candidate.current !== undefined && !valid(candidate.current)) ||
          (candidate.phase === "idle" &&
            (candidate.migrationId !== undefined ||
              candidate.from !== candidate.to ||
              candidate.current !== candidate.to)) ||
          (candidate.phase !== "idle" && !candidate.migrationId) ||
          (candidate.migrationId && retired.includes(candidate.migrationId))
        )
          throw new Error("invalid migration identity");
        if (
          previous?.migrationId === candidate.migrationId &&
          previous &&
          (Object.keys(previous.seq) as Kind[]).some(
            (name) => candidate.seq[name] < previous.seq[name],
          )
        )
          throw new Error("migration baseline regressed");
        if (
          previous?.migrationId === candidate.migrationId &&
          previous &&
          (previous.phase === "completed" || previous.phase === "failed") &&
          candidate.phase !== previous.phase
        )
          throw new Error("migration terminal regressed");
        if (
          candidate.phase === "completed" &&
          (candidate.from === undefined || candidate.current !== candidate.to)
        )
          throw new Error("incomplete migration terminal");
        if (candidate.phase === "failed" && !candidate.error)
          throw new Error("missing migration failure");
        if (previous?.migrationId && previous.migrationId !== candidate.migrationId) {
          retired.push(previous.migrationId);
          observed.delete(previous.migrationId);
          if (retired.length > 16) retired.shift();
        }
        state.snapshot = candidate;
        let needs = overflowArrival > requestedArrival;
        let lateTerminal = false;
        for (const [id, demand] of observed) {
          if (id === candidate.migrationId) {
            needs ||= (["progress", "done", "failed"] as const).some(
              (kind) => demand[kind] > candidate.seq[kind],
            );
          } else if (demand.arrival > requestedArrival) {
            needs = true;
            lateTerminal ||= demand.done > 0 || demand.failed > 0;
          } else {
            observed.delete(id);
          }
        }
        // 第二遍发出后新流的终态才到达时允许一次末次补读，其余仍保留恢复提示。
        finalCatchup = pass === 1 && lateTerminal;
        state.needsRecovery = needs;
        state.error = undefined;
        emit();
        if (!state.needsRecovery) break;
      }
    } catch {
      if (current(token)) {
        state.error = { code: "app.event-failed", message: "迁移快照恢复失败，可主动重试" };
        state.needsRecovery = true;
      }
    } finally {
      if (current(token)) {
        pending = undefined;
        state.recovering = false;
        emit();
      }
    }
  }
  function recover(): Promise<void> {
    if (!connected) return Promise.resolve();
    if (pending) return pending;
    const token = generation;
    const request = Promise.resolve().then(() => refresh(token));
    pending = request;
    state.recovering = true;
    emit();
    return request;
  }
  function receive(event: MigrationNotification) {
    if (
      !valid(event.seq) ||
      event.seq === 0 ||
      !event.migrationId ||
      retired.includes(event.migrationId)
    )
      return;
    if (
      event.migrationId === state.snapshot?.migrationId &&
      event.seq <= state.snapshot.seq[event.kind]
    )
      return;
    let demand = observed.get(event.migrationId);
    if (demand && event.seq <= demand[event.kind]) return;
    arrival++;
    if (!demand) {
      if (observed.size === 32) {
        const remove = [...observed.keys()].find((id) => id !== state.snapshot?.migrationId)!;
        observed.delete(remove);
        overflowArrival = arrival;
      }
      demand = { progress: 0, done: 0, failed: 0, arrival };
      observed.set(event.migrationId, demand);
    }
    demand[event.kind] = Math.max(demand[event.kind], event.seq);
    demand.arrival = arrival;
    state.needsRecovery = true;
    if (connected) void recover();
  }
  function connect() {
    connected = true;
    return recover();
  }
  function disconnect() {
    connected = false;
    generation++;
    pending = undefined;
    state.recovering = false;
    emit();
  }
  return { receive, connect, disconnect, recover };
}
