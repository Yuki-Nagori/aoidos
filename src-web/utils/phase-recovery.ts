import type { PhaseSequences, PhaseSnapshot } from "../api/engine";
import {
  consumePhaseEvent,
  consumePhaseSnapshot,
  validPhaseState,
  phaseEventKeys,
  type PhaseNotification,
} from "./phase-consumer";

export interface PhaseRecoveryState {
  snapshot?: PhaseSnapshot;
  recovering: boolean;
  needsRecovery: boolean;
  error?: { code: string; message: string };
}
/** 一次恢复最多两读；事件只合并缺口，不生成无限请求队列或定时轮询。 */
export function createPhaseRecovery(
  get: (sessionId: string) => Promise<PhaseSnapshot>,
  publish: (state: PhaseRecoveryState) => void,
) {
  let id: string | undefined;
  let generation = 0;
  let connected = false;
  let pending: Promise<void> | undefined;
  let cache: PhaseNotification[] = [];
  let overflowed = false;
  const reads = new Map<string, Promise<PhaseSnapshot>>();
  const retired: string[] = [];
  const observed = new Map<string, PhaseSequences>();
  let state: PhaseRecoveryState = { recovering: false, needsRecovery: false };
  const emit = () => publish({ ...state });
  const current = (token: number, session: string) =>
    token === generation && session === id && connected;
  const knownOld = (event: PhaseNotification) => retired.includes(event.envelope.data.stateEpoch);

  async function refresh(token: number, session: string) {
    try {
      for (let pass = 0; pass < 2; pass++) {
        await reads.get(session)?.catch(() => undefined);
        if (!current(token, session)) return;
        if (reads.size >= 32) throw new Error("capacity");
        const hints = new Set(cache.map((event) => event.envelope.data.stateEpoch));
        const request = Promise.resolve().then(() => get(session));
        reads.set(session, request);
        let candidate: PhaseSnapshot;
        try {
          candidate = await request;
        } finally {
          reads.delete(session);
        }
        if (!current(token, session)) return;
        const next = retired.includes(candidate.stateEpoch)
          ? undefined
          : consumePhaseSnapshot(state.snapshot, candidate);
        if (!next || next.sessionId !== session) {
          state.needsRecovery = true;
          if (pass === 0) continue;
          throw new Error("stale");
        }
        if (state.snapshot && next.stateEpoch !== state.snapshot.stateEpoch) {
          retired.push(state.snapshot.stateEpoch);
          observed.delete(state.snapshot.stateEpoch);
          if (retired.length > 16) retired.shift();
        }
        state.snapshot = next;
        state.needsRecovery = overflowed;
        overflowed = false;
        const replay = cache;
        cache = [];
        for (const event of replay) {
          if (knownOld(event)) continue;
          if (event.envelope.data.stateEpoch !== next.stateEpoch) {
            if (pass === 0 || !hints.has(event.envelope.data.stateEpoch)) {
              cache.push(event);
              state.needsRecovery = true;
            }
            continue;
          }
          const consumed = consumePhaseEvent(state.snapshot, event);
          state.snapshot = consumed.snapshot;
          state.needsRecovery ||= consumed.needsRecovery;
          if (consumed.needsRecovery) cache.push(event);
        }
        const seen = observed.get(next.stateEpoch);
        if (seen)
          state.needsRecovery ||= Object.values(phaseEventKeys).some(
            (kind) => seen[kind] > state.snapshot!.seq[kind],
          );
        state.error = undefined;
        emit();
        if (!state.needsRecovery) break;
      }
    } catch {
      if (current(token, session)) {
        state.error = { code: "app.event-failed", message: "阶段恢复失败，可主动重试" };
        state.needsRecovery = true;
      }
    } finally {
      if (current(token, session)) {
        pending = undefined;
        state.recovering = false;
        emit();
      }
    }
  }
  function recover(): Promise<void> {
    if (!id || !connected) return Promise.resolve();
    if (pending) return pending;
    const token = generation,
      session = id;
    pending = Promise.resolve().then(() => refresh(token, session));
    state.recovering = true;
    emit();
    return pending;
  }
  function select(sessionId: string | undefined) {
    if (id === sessionId) return;
    id = sessionId;
    generation++;
    pending = undefined;
    cache = [];
    overflowed = false;
    retired.length = 0;
    observed.clear();
    state = { recovering: false, needsRecovery: false };
    emit();
    if (connected) void recover();
  }
  function receive(event: PhaseNotification) {
    if (
      !id ||
      event.envelope.data.sessionId !== id ||
      knownOld(event) ||
      !validPhaseState(event.envelope.data) ||
      !Number.isSafeInteger(event.envelope.seq) ||
      event.envelope.seq <= 0
    )
      return;
    const epoch = event.envelope.data.stateEpoch;
    if (!observed.has(epoch)) {
      if (observed.size === 16) {
        const oldest = [...observed.keys()].find((value) => value !== state.snapshot?.stateEpoch)!;
        observed.delete(oldest);
      }
      observed.set(epoch, {
        phaseChanged: 0,
        sceneAdvanced: 0,
        operationDone: 0,
        operationFailed: 0,
      });
    }
    const seen = observed.get(epoch)!;
    const kind = phaseEventKeys[event.name];
    seen[kind] = Math.max(seen[kind], event.envelope.seq);
    if (!pending && connected && state.snapshot) {
      const consumed = consumePhaseEvent(state.snapshot, event);
      state.snapshot = consumed.snapshot;
      if (!consumed.needsRecovery) {
        // 迟到的连续事件可能补齐此前两读留下的缺口；只重放有界缓存，不增加请求。
        let progressed = true;
        while (progressed && cache.length) {
          progressed = false;
          const replay = cache;
          cache = [];
          for (const queued of replay) {
            const next = consumePhaseEvent(state.snapshot!, queued);
            state.snapshot = next.snapshot;
            if (next.needsRecovery) cache.push(queued);
            else progressed = true;
          }
        }
        const observedSeq = observed.get(state.snapshot!.stateEpoch)!;
        state.needsRecovery =
          overflowed ||
          cache.length > 0 ||
          Object.values(phaseEventKeys).some((key) => observedSeq[key] > state.snapshot!.seq[key]);
        if (!state.needsRecovery) state.error = undefined;
        emit();
        return;
      }
    }
    if (
      cache.some(
        (old) =>
          old.name === event.name &&
          old.envelope.seq === event.envelope.seq &&
          old.envelope.data.stateEpoch === event.envelope.data.stateEpoch,
      )
    )
      return;
    if (cache.length === 32) {
      cache = [];
      overflowed = true;
    }
    cache.push(event);
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
    cache = [];
    overflowed = false;
    observed.clear();
    state.recovering = false;
    emit();
  }
  return { select, receive, recover, connect, disconnect };
}
