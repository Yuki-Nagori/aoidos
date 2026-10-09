import {
  onScopeDispose,
  readonly,
  shallowRef,
  toValue,
  watch,
  type DeepReadonly,
  type MaybeRefOrGetter,
} from "vue";
import type { RecordItem } from "../api/records";
import { useEnginePhase } from "./useEnginePhase";
import { useLlmTurn } from "./useLlmTurn";
import { useRecordView } from "./useRecordView";

interface Ports {
  phase?: Parameters<typeof useEnginePhase>[1];
  turn?: Parameters<typeof useLlmTurn>[1];
  records?: Parameters<typeof useRecordView>[1];
}
/** 从公开阶段或持久记录发现回合；整轮事件丢失时，主动恢复先确认身份再读正文。 */
export function useGameView(sessionId: MaybeRefOrGetter<string | undefined>, ports: Ports = {}) {
  const turnId = shallowRef<string>();
  const liveTurnId = shallowRef<string>();
  const latestPublicRecord = shallowRef<DeepReadonly<RecordItem>>();
  let epoch = 0,
    disposed = false,
    discovering = true,
    recordFloor = 0;
  let operationId: string | undefined;
  let historyRevision: number | undefined;
  let phaseEpoch: string | undefined;
  let viewEpoch: string | undefined;
  const branchPending = shallowRef(false);
  let pending: Promise<void> | undefined;
  watch(
    () => toValue(sessionId),
    () => {
      epoch++;
      pending = undefined;
      turnId.value = undefined;
      liveTurnId.value = undefined;
      latestPublicRecord.value = undefined;
      historyRevision = undefined;
      phaseEpoch = undefined;
      viewEpoch = undefined;
      branchPending.value = false;
      recordFloor = 0;
      operationId = undefined;
      discovering = true;
    },
    { immediate: true, flush: "sync" },
  );
  const phase = useEnginePhase(sessionId, ports.phase);
  const records = useRecordView(sessionId, ports.records);
  const turn = useLlmTurn(liveTurnId, ports.turn);
  function discover(): void {
    if (disposed || !discovering) return;
    const id = toValue(sessionId);
    const state = phase.state.value.snapshot;
    const view = records.state.value.view;
    const flight = state?.sessionId === id ? state?.inFlight : undefined;
    const active = flight?.turnId;
    const current = view?.sessionId === id ? view : undefined;
    const confirmed = state?.sessionId === id ? state : undefined;
    const historyChanged =
      confirmed && historyRevision !== undefined && historyRevision !== confirmed.historyRevision;
    const phaseChanged =
      confirmed && phaseEpoch !== undefined && phaseEpoch !== confirmed.stateEpoch;
    const viewChanged = current && viewEpoch !== undefined && viewEpoch !== current.viewEpoch;
    if (historyChanged || phaseChanged || viewChanged) {
      turnId.value = undefined;
      liveTurnId.value = undefined;
      latestPublicRecord.value = undefined;
      recordFloor = 0;
      operationId = undefined;
    }
    if (confirmed) {
      historyRevision = confirmed.historyRevision;
      phaseEpoch = confirmed.stateEpoch;
    }
    if (current) viewEpoch = current.viewEpoch;
    if (historyChanged) {
      // 记录 API 没有 historyRevision；等旧读结束后再读一次，不能把撤离分支当当前窗口。
      branchPending.value = true;
      void refresh(false);
    }
    if (branchPending.value) return;
    const player = current?.items.find((item) => item.kind === "playerSpeech");
    const latest = current?.items.find(
      (item) => (item.kind === "narration" || item.kind === "characterSpeech") && item.turnId,
    );
    if (flight && !active) {
      if (operationId !== flight.operationId) {
        turnId.value = undefined;
        liveTurnId.value = undefined;
        latestPublicRecord.value = undefined;
        // 新窗口可能已经封口；只用旧公开边界与新输入，而非未来正文的位置。
        recordFloor = Math.max(recordFloor, player?.recordSeq ?? 0);
      }
      operationId = flight.operationId;
      return;
    }
    if (active) {
      operationId = flight.operationId;
      recordFloor = Math.max(
        recordFloor,
        player?.recordSeq ?? 0,
        turnId.value !== active && latest?.turnId !== active ? (latest?.recordSeq ?? 0) : 0,
      );
      turnId.value = active;
      liveTurnId.value = active;
      latestPublicRecord.value = latest?.turnId === active ? latest : undefined;
      return;
    }
    if (current?.inFlight) {
      turnId.value = current.inFlight.turnId;
      liveTurnId.value = current.inFlight.turnId;
      recordFloor = Math.max(recordFloor, player?.recordSeq ?? 0);
      latestPublicRecord.value = undefined;
      return;
    }
    if (player && (!latest || player.recordSeq > latest.recordSeq)) {
      turnId.value = undefined;
      liveTurnId.value = undefined;
      latestPublicRecord.value = undefined;
      recordFloor = Math.max(recordFloor, player.recordSeq);
      return;
    }
    if (latest && (latest.turnId === turnId.value || latest.recordSeq > recordFloor)) {
      turnId.value = latest.turnId;
      latestPublicRecord.value = latest;
      recordFloor = latest.recordSeq;
      // 进程重开后只有持久记录；不向新的内存 ring 查询已提交的历史 UUID。
      if (liveTurnId.value !== latest.turnId) liveTurnId.value = undefined;
    }
  }
  watch([phase.state, records.state], discover, { immediate: true, flush: "sync" });
  function refresh(reconnect: boolean): Promise<void> {
    if (disposed) return Promise.resolve();
    if (pending) return pending;
    const token = epoch;
    discovering = false;
    const request = Promise.resolve()
      .then(async () => {
        await Promise.all(
          reconnect
            ? [phase.reconnect(), records.reconnect()]
            : [phase.recover(), records.recover()],
        );
        if (disposed || token !== epoch) return;
        discovering = true;
        discover();
        if (branchPending.value) {
          await records.recover();
          if (disposed || token !== epoch) return;
          const confirmation = records.state.value;
          if (
            confirmation.error ||
            confirmation.needsRecovery ||
            confirmation.view?.sessionId !== toValue(sessionId)
          )
            return;
          branchPending.value = false;
          discover();
        }
        if (reconnect) await turn.reconnect();
        else await turn.recover();
      })
      .finally(() => {
        if (!disposed && token === epoch) {
          discovering = true;
          pending = undefined;
        }
      });
    pending = request;
    return request;
  }
  onScopeDispose(() => {
    disposed = true;
    epoch++;
  });
  return {
    phase,
    turn,
    records,
    turnId: readonly(turnId),
    latestPublicRecord: readonly(latestPublicRecord),
    historyPending: readonly(branchPending),
    recover: () => refresh(false),
    reconnect: () => refresh(true),
  };
}
