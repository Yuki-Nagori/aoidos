import type { PhaseSnapshot } from "../../../src-web/api/engine";

/** 固定合法身份，不依赖平台 UUID API；覆盖跨流 / 跨会话恢复竞态。 */
export function snapshot(extra: Partial<PhaseSnapshot> = {}): PhaseSnapshot {
  return {
    sessionId: "00000000-0000-0000-0000-000000000001",
    stateEpoch: "00000000-0000-0000-0000-000000000002",
    phaseRevision: 0,
    historyRevision: 0,
    phase: "idle",
    needsRecovery: false,
    resumeRequired: false,
    seq: { phaseChanged: 0, sceneAdvanced: 0, operationDone: 0, operationFailed: 0 },
    ...extra,
  };
}
