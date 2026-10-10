import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type { TurnEnvelope, TurnOutcome } from "./llm";
import type { Locale } from "./locale";

type Phase = "idle" | "generating" | "awaitingCheck" | "settling" | "advancing";
interface ScenePosition {
  sceneId: string;
  path: { kind: string; id: string; title: string }[];
}
/** 对应 Rust PhaseState；正文由回合 / 记录 API 提供。 */
export interface PhaseState {
  sessionId: string;
  stateEpoch: string;
  phaseRevision: number;
  historyRevision: number;
  phase: Phase;
  scene?: ScenePosition;
  inFlight?: { operationId: string; roundId?: string; turnId?: string };
  check?: {
    planId: string;
    status: "waiting" | "rolling";
    mode: "manual" | "auto";
    ruleId: string;
    actorId: string;
    expression: string;
    modifierTotal: number;
  };
  needsRecovery: boolean;
  resumeRequired: boolean;
  checkpoint?: {
    sourceRoundId: string;
    throughSeq: number;
    stage: "check" | "narration" | "settle" | "advance";
  };
  lastOperation?: {
    operationId: string;
    roundId?: string;
    outcome: "accepted" | TurnOutcome;
    error?: { code: string; message: string };
  };
}
export interface PhaseSequences {
  phaseChanged: number;
  sceneAdvanced: number;
  operationDone: number;
  operationFailed: number;
}
export interface PhaseSnapshot extends PhaseState {
  seq: PhaseSequences;
}
export interface AcceptedRound {
  operationId: string;
  roundId: string;
}
type AcceptedOperation = { operationId: string };
/** 四种事件均包含完整阶段状态；各自序号与跨事件修订分别消费。 */
export type PhaseEventMap = {
  "engine:phase:changed": PhaseState;
  "engine:scene:advanced": PhaseState & { previousSceneId?: string };
  "engine:operation:done": PhaseState & {
    operationId: string;
    outcome: "completed" | "cancelled";
  };
  "engine:operation:failed": PhaseState & {
    operationId: string;
    code: string;
    message: string;
  };
};

/** 仅透传原始输入；语法、场景、门禁和规则由 Rust 确认。 */
export function submitInput(sessionId: string, text: string): Promise<AcceptedRound> {
  return invoke("engine_submit_input", { sessionId, text });
}
/** 取消当前子调用后由引擎保留父 lease，接纳插话。 */
export function interruptRound(
  sessionId: string,
  roundId: string,
  text: string,
): Promise<AcceptedOperation> {
  return invoke("engine_interrupt", { sessionId, roundId, text });
}
export function cancelRound(
  sessionId: string,
  roundId: string,
): Promise<{ roundId: string; outcome: TurnOutcome }> {
  return invoke("engine_cancel_round", { sessionId, roundId });
}
/** 显式续行冻结新配置，并复用检查点已有步骤。 */
export function resume(sessionId: string): Promise<AcceptedRound> {
  return invoke("engine_resume", { sessionId });
}
export function regenerate(sessionId: string, roundId: string): Promise<AcceptedRound> {
  return invoke("engine_regenerate", { sessionId, roundId });
}
export function rewind(sessionId: string, targetSeq: number): Promise<AcceptedOperation> {
  return invoke("engine_rewind", { sessionId, targetSeq });
}
/** 重复确认同一计划不会重复采样；不接收骰值或随机 seed。 */
export function submitCheck(
  sessionId: string,
  roundId: string,
  planId: string,
): Promise<{ roundId: string; planId: string; accepted: boolean }> {
  return invoke("engine_submit_check", { sessionId, roundId, planId });
}
export function getPhase(sessionId: string): Promise<PhaseSnapshot> {
  return invoke("engine_get_phase", { sessionId });
}
/** 监听主窗口；生命周期与恢复调度由消费方持有。 */
export function listenPhaseEvent<K extends keyof PhaseEventMap>(
  name: K,
  handler: (event: TurnEnvelope<PhaseEventMap[K]>) => void,
): Promise<UnlistenFn> {
  return listen<TurnEnvelope<PhaseEventMap[K]>>(name, (event) => handler(event.payload), {
    target: { kind: "AnyLabel", label: "main" },
  });
}

/** 对应 Rust 随应用内嵌的剧本摘要；许可正文与资源同时发布。 */
export interface ScriptInfo {
  scriptId: string;
  title: string;
  displayNames: Record<Locale, string>;
  attributions: string;
}
export interface OpenedSession {
  sessionId: string;
  profileId: string;
  scriptId: string;
  title: string;
}
/** 只读内嵌资源，不读取外部文件或发生成请求。 */
export function listScripts(): Promise<ScriptInfo[]> {
  return invoke("engine_list_scripts");
}
/** 明确选择配置和剧本；startNew=false 重开，恢复生成须另行显式提交。 */
export function openSession(
  profileId: string,
  scriptId: string,
  startNew: boolean,
): Promise<OpenedSession> {
  return invoke("engine_open_session", { profileId, scriptId, startNew });
}
