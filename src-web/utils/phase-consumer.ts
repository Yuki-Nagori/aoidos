import type { PhaseEventMap, PhaseSequences, PhaseSnapshot, PhaseState } from "../api/engine";
import type { TurnEnvelope } from "../api/llm";

/** 消费者只使用四类事件共有的完整状态，专属字段仍由 API 的事件映射定型。 */
export interface PhaseNotification {
  name: keyof PhaseEventMap;
  envelope: TurnEnvelope<PhaseState>;
}
const encoder = new TextEncoder();
const safe = (value: number) => Number.isSafeInteger(value) && value >= 0;
const uuid = (value: string) =>
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);
const id = (value: string) => /^[a-zA-Z0-9_-]{1,64}$/.test(value);
const error = (value: { code: string; message: string }) =>
  /^[a-zA-Z0-9_-]{1,64}\.[a-zA-Z0-9_-]{1,64}$/.test(value.code) &&
  encoder.encode(value.message).length <= 512;
function expression(value: string): boolean {
  const match = /^(\d+)d(\d+)([+-]\d+)?$/.exec(value);
  if (!match) return false;
  const count = Number(match[1]),
    sides = Number(match[2]),
    modifier = Number(match[3] ?? 0);
  return (
    count >= 1 &&
    count <= 20 &&
    sides >= 2 &&
    sides <= 1000 &&
    modifier >= -100 &&
    modifier <= 100 &&
    `${count}d${sides}${modifier > 0 ? `+${modifier}` : modifier < 0 ? modifier : ""}` === value
  );
}
/** 四种事件各自维护确认位置；场景和操作事件不能替代阶段事件的缺口检测。 */
export const phaseEventKeys: Record<keyof PhaseEventMap, keyof PhaseSequences> = {
  "engine:phase:changed": "phaseChanged",
  "engine:scene:advanced": "sceneAdvanced",
  "engine:operation:done": "operationDone",
  "engine:operation:failed": "operationFailed",
};
/** 复制已声明字段及嵌套对象的确定顺序，避免 Value 事件与类型化快照的键顺序差异。 */
function phaseState(data: PhaseState): PhaseState {
  return {
    sessionId: data.sessionId,
    stateEpoch: data.stateEpoch,
    phaseRevision: data.phaseRevision,
    historyRevision: data.historyRevision,
    phase: data.phase,
    scene: data.scene && {
      sceneId: data.scene.sceneId,
      path: data.scene.path.map((node) => ({ kind: node.kind, id: node.id, title: node.title })),
    },
    inFlight: data.inFlight && {
      operationId: data.inFlight.operationId,
      roundId: data.inFlight.roundId,
      turnId: data.inFlight.turnId,
    },
    check: data.check && {
      planId: data.check.planId,
      status: data.check.status,
      mode: data.check.mode,
      ruleId: data.check.ruleId,
      actorId: data.check.actorId,
      expression: data.check.expression,
      modifierTotal: data.check.modifierTotal,
    },
    needsRecovery: data.needsRecovery,
    resumeRequired: data.resumeRequired,
    checkpoint: data.checkpoint && {
      sourceRoundId: data.checkpoint.sourceRoundId,
      throughSeq: data.checkpoint.throughSeq,
      stage: data.checkpoint.stage,
    },
    lastOperation: data.lastOperation && {
      operationId: data.lastOperation.operationId,
      roundId: data.lastOperation.roundId,
      outcome: data.lastOperation.outcome,
      error: data.lastOperation.error && {
        code: data.lastOperation.error.code,
        message: data.lastOperation.error.message,
      },
    },
  };
}
/** 容量、确认身份与互斥关系对齐 Rust 发布边界；不从事件推断骰值或正文。 */
export function validPhaseState(data: PhaseState): boolean {
  return (
    uuid(data.sessionId) &&
    uuid(data.stateEpoch) &&
    safe(data.phaseRevision) &&
    safe(data.historyRevision) &&
    ["idle", "generating", "awaitingCheck", "settling", "advancing"].includes(data.phase) &&
    encoder.encode(JSON.stringify(data)).length <= 64 * 1024 &&
    !(data.inFlight && data.resumeRequired) &&
    data.resumeRequired === (data.checkpoint !== undefined) &&
    !(data.needsRecovery && data.checkpoint) &&
    (!data.checkpoint ||
      (uuid(data.checkpoint.sourceRoundId) &&
        safe(data.checkpoint.throughSeq) &&
        data.checkpoint.throughSeq > 0 &&
        ["check", "narration", "settle", "advance"].includes(data.checkpoint.stage))) &&
    (!data.inFlight ||
      (uuid(data.inFlight.operationId) &&
        (data.inFlight.roundId === undefined || uuid(data.inFlight.roundId)) &&
        (data.inFlight.turnId === undefined || uuid(data.inFlight.turnId)))) &&
    (!data.check ||
      (id(data.check.planId) &&
        id(data.check.ruleId) &&
        id(data.check.actorId) &&
        ["waiting", "rolling"].includes(data.check.status) &&
        ["manual", "auto"].includes(data.check.mode) &&
        expression(data.check.expression) &&
        Number.isInteger(data.check.modifierTotal) &&
        data.check.modifierTotal >= -100 &&
        data.check.modifierTotal <= 100)) &&
    (!data.lastOperation ||
      (uuid(data.lastOperation.operationId) &&
        (data.lastOperation.roundId === undefined || uuid(data.lastOperation.roundId)) &&
        ["accepted", "completed", "cancelled", "failed"].includes(data.lastOperation.outcome) &&
        (data.lastOperation.outcome === "failed") === (data.lastOperation.error !== undefined) &&
        (!data.lastOperation.error || error(data.lastOperation.error)))) &&
    (!data.scene ||
      (id(data.scene.sceneId) &&
        data.scene.path.length > 0 &&
        data.scene.path.length <= 16 &&
        data.scene.path.at(-1)!.id === data.scene.sceneId &&
        new Set(data.scene.path.map((node) => node.id)).size === data.scene.path.length &&
        data.scene.path.every(
          (node) =>
            id(node.id) &&
            id(node.kind) &&
            node.title.trim().length > 0 &&
            encoder.encode(node.title).length <= 256,
        )))
  );
}
/** 快照覆盖四条基线及跨流修订；旧快照不能把新回合 / 因果分支降回去。 */
export function consumePhaseSnapshot(
  current: PhaseSnapshot | undefined,
  candidate: PhaseSnapshot,
): PhaseSnapshot | undefined {
  if (
    !validPhaseState(candidate) ||
    !Object.values(phaseEventKeys).every((key) => safe(candidate.seq[key]))
  )
    return;
  if (current) {
    if (candidate.sessionId !== current.sessionId) return;
    if (
      candidate.stateEpoch === current.stateEpoch &&
      (candidate.phaseRevision < current.phaseRevision ||
        candidate.historyRevision < current.historyRevision ||
        !Object.values(phaseEventKeys).every((key) => candidate.seq[key] >= current.seq[key]) ||
        (candidate.phaseRevision === current.phaseRevision &&
          JSON.stringify(phaseState(candidate)) !== JSON.stringify(phaseState(current))))
    )
      return;
  }
  return { ...phaseState(candidate), seq: { ...candidate.seq } };
}
/** 缺口先恢复；旧修订事件仅确认自身序号，不覆盖较新状态或最近操作。 */
export function consumePhaseEvent(snapshot: PhaseSnapshot, event: PhaseNotification) {
  const { seq, data } = event.envelope;
  const unchanged = (needsRecovery: boolean) => ({ snapshot, needsRecovery });
  if (data.sessionId !== snapshot.sessionId) return unchanged(false);
  if (!validPhaseState(data) || !safe(seq) || seq === 0) return unchanged(true);
  if (data.stateEpoch !== snapshot.stateEpoch) return unchanged(true);
  const key = phaseEventKeys[event.name];
  if (seq <= snapshot.seq[key]) return unchanged(false);
  if (seq !== snapshot.seq[key] + 1) return unchanged(true);
  if (
    data.phaseRevision >= snapshot.phaseRevision &&
    data.historyRevision < snapshot.historyRevision
  )
    return unchanged(true);
  if (
    data.phaseRevision === snapshot.phaseRevision &&
    JSON.stringify(phaseState(data)) !== JSON.stringify(phaseState(snapshot))
  )
    return unchanged(true);
  return {
    snapshot: {
      ...phaseState(data.phaseRevision > snapshot.phaseRevision ? data : snapshot),
      seq: { ...snapshot.seq, [key]: seq },
    },
    needsRecovery: false,
  };
}
