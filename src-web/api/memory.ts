import { invoke } from "@tauri-apps/api/core";

type MemoryAlgorithm = "exponential" | "segmented" | "none";

/** 与 Rust PolicyParameters 同型；参数单位与边界见记忆算法协议。 */
export interface MemoryPolicyParameters {
  algorithm: MemoryAlgorithm;
  baseStrength: number;
  reinforcementGain: number;
  reinforcementCap: number;
  strengthCap: number;
  halfLifeRounds: number;
  repeatWindowRounds: number;
  repeatLimit: number;
  revivalCooldownRounds: number;
  activeCapacity: number;
  rateTableVersion: number;
  decayRate: number;
}

/** 缺省字段继承应用值；显式 null 非法，合法的 0 仍是明确覆盖。 */
export type MemoryPolicyOverrides = Partial<MemoryPolicyParameters>;

export interface MemoryAppConfig {
  configVersion: number;
  algorithmVersion: number;
  parameters: MemoryPolicyParameters;
  defaultParameters: MemoryPolicyParameters;
}

export interface MemoryScriptConfig {
  scriptId: string;
  configVersion: number;
  appConfigVersion: number;
  algorithmVersion: number;
  overrides: MemoryPolicyOverrides;
  inherited: MemoryPolicyParameters;
  effective: MemoryPolicyParameters;
}

export interface MemoryConfigSaved {
  configVersion: number;
}

export interface MemorySourceRef {
  scriptId: string;
  runId: string;
  sessionId: string;
  recordSeq: number;
  bodyHash: number[];
  startByte: number;
  endByte: number;
  mode?: "inCharacter" | "outOfCharacter";
}

export type MemoryQuestionCandidate =
  | { kind: "entity"; entityId: string; displayName?: string }
  | {
      kind: "memoryClassification";
      classification: "repeat" | "supplement" | "new" | "correction" | "notWorthy" | "uncertain";
    };

export type MemoryQuestionAnswer =
  | { kind: "selectCandidate"; candidate: MemoryQuestionCandidate }
  | { kind: "proposeEntity"; displayName: string }
  | { kind: "none" }
  | { kind: "skip" };

export interface MemoryQuestion {
  questionId: string;
  runId: string;
  questionKey: number[];
  revision: number;
  draft: {
    version: number;
    identity: {
      kind: "entityAssociation" | "memoryClassification";
      eventSlot: number;
      sources: MemorySourceRef[];
    };
    historyRevision: number;
    entityRevision: number;
    evidenceRevision: number;
    mentionedName?: string;
    candidates: MemoryQuestionCandidate[];
  };
  status: "pending" | "resolved" | "invalidated";
  answer: MemoryQuestionAnswer | null;
}

export interface MemoryQuestionCursor {
  runId: string;
  historyRevision: number;
  snapshotMaxSequence: number;
  afterSequence: number;
}

export interface MemoryQuestionPage {
  questions: MemoryQuestion[];
  nextCursor: MemoryQuestionCursor | null;
  historyRevision: number;
}

export interface MemoryEntry {
  entryId: string;
  currentVersionId: string;
  stateRevision: number;
  state: "active" | "archived" | "unavailable";
}

export interface MemoryKnowledgeEvidence {
  evidenceId: string;
  subjectId: string;
  source: MemorySourceRef;
  kind: "observed" | "told" | "rumor";
  informantId?: string;
  scope: string;
  ruleId: string;
}

export interface MemoryEntryCursor {
  runId: string;
  historyRevision: number;
  snapshotMaxSequence: number;
  afterSequence: number;
}

export interface MemoryEntryPage {
  entries: MemoryEntry[];
  nextCursor: MemoryEntryCursor | null;
  historyRevision: number;
}

export interface MemoryEntryDetails {
  entry: MemoryEntry;
  version: {
    version: number;
    entryId: string;
    versionId: string;
    parentVersionId?: string;
    restoredFromVersionId?: string;
    subject: { kind: "playerPreference" } | { kind: "character"; id: string };
    dimension: "story" | "relationship" | "playerPreference" | "foreshadowing";
    summary: string;
    attitude: "unexpressed" | "believed" | "doubtful" | "denied";
    inferred: boolean;
    change: "new" | "supplement" | "correction" | "restore";
    sources: MemorySourceRef[];
    evidence: MemoryKnowledgeEvidence[];
    sourceSetHash: number[];
    evidenceSetHash: number[];
    policyHash: number[];
  };
}

export interface MemoryBatchStatus {
  batchId: string;
  runId: string;
  historyRevision: number;
  state: "queued" | "dispatched" | "received" | "completed" | "rejected" | "unknown";
  completion: {
    version: number;
    outcomes: {
      candidateId: string;
      outcome: "applied" | "noOp" | "rejected" | "pending" | "deferred";
      reasonCode?: string;
    }[];
    cycleMarks?: {
      candidateId: string;
      markId: string;
    }[];
  } | null;
  diagnosticCode: string | null;
}

/** 读取已确认的应用级默认参数。 */
export function getMemoryAppConfig(): Promise<MemoryAppConfig> {
  return invoke("memory_get_app_config");
}

/** 以期望版本 CAS 保存应用级完整参数。 */
export function saveMemoryAppConfig(
  expectedVersion: number,
  parameters: MemoryPolicyParameters,
): Promise<MemoryConfigSaved> {
  return invoke("memory_save_app_config", { expectedVersion, parameters });
}

/** 读取剧本显式覆盖及按字段继承后的有效参数。 */
export function getMemoryScriptConfig(scriptId: string): Promise<MemoryScriptConfig> {
  return invoke("memory_get_script_config", { scriptId });
}

/** 同时校验覆盖版本与应用默认版本，避免保存过期预览。 */
export function saveMemoryScriptConfig(
  scriptId: string,
  expectedVersion: number,
  expectedAppVersion: number,
  overrides: MemoryPolicyOverrides,
): Promise<MemoryConfigSaved> {
  return invoke("memory_save_script_config", {
    scriptId,
    expectedVersion,
    expectedAppVersion,
    overrides,
  });
}

/** 读取快照上界固定的待确认问题页。 */
export function listMemoryQuestions(
  runId: string,
  cursor: MemoryQuestionCursor | null,
  limit: number,
): Promise<MemoryQuestionPage> {
  return invoke("memory_list_questions", { runId, cursor, limit });
}

/** 以当前问题及因果版本回答，不自动重试过期答案。 */
export function resolveMemoryQuestion(
  runId: string,
  resolution: {
    questionId: string;
    expectedRevision: number;
    expectedHistoryRevision: number;
    answer: MemoryQuestionAnswer;
  },
): Promise<MemoryQuestion> {
  return invoke("memory_resolve_question", { runId, resolution });
}

/** 按稳定条目 ID 和历史修订分页读取条目索引。 */
export function listMemoryEntries(
  runId: string,
  cursor: MemoryEntryCursor | null,
  limit: number,
): Promise<MemoryEntryPage> {
  return invoke("memory_list_entries", { runId, cursor, limit });
}

/** 读取指定版本仍为当前版本的条目正文。 */
export function getMemoryEntry(
  runId: string,
  entryId: string,
  expectedVersionId: string,
): Promise<MemoryEntryDetails> {
  return invoke("memory_get_entry", { runId, entryId, expectedVersionId });
}

/** 主动恢复持久批次结果，不会重发供应商请求。 */
export function getMemoryBatchStatus(runId: string, batchId: string): Promise<MemoryBatchStatus> {
  return invoke("memory_get_batch_status", { runId, batchId });
}
