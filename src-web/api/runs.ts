import { invoke } from "@tauri-apps/api/core";
import type { OpenedSession } from "./engine";
import type { LlmProfile } from "./llm";

export type RunState =
  | "ready"
  | "needs-profile-binding"
  | "billing-not-configured"
  | "billing-reconciliation-required"
  | "recovery-required"
  | "needs-repair"
  | "script-unavailable"
  | "settlement-pending"
  | "deleting";

export interface RunCursor {
  catalogRevision: number;
  afterCreatedAt: string;
  afterSessionId: string;
}

export interface RunSummary {
  sessionId: string;
  scriptId: string;
  createdAt: string;
  memoryRunId: string | null;
  billingRunId: string | null;
  profileId: string | null;
  revision: number;
  state: RunState;
  active: boolean;
  deletionOperationId: string | null;
}

export interface RunPage {
  entries: RunSummary[];
  nextCursor: RunCursor | null;
  catalogRevision: number;
  scanIncomplete: boolean;
  scanIssueCount: number;
  pendingDeletions: DeletionOperation[];
  pendingDeletionsTruncated: boolean;
}

export interface OpenedRun {
  session: OpenedSession;
  profile: LlmProfile;
}

export interface DeletionOperation {
  operationId: string;
  sessionId: string;
  state:
    | "requested"
    | "owner-closed"
    | "files-quarantined"
    | "database-purged"
    | "files-purged"
    | "needs-repair"
    | "completed";
  catalogRevision: number;
  diagnosticCode: string | null;
}

export interface LegacyScanResult {
  discovered: number;
  diagnosed: number;
  scanIncomplete: boolean;
  scanIssueCount: number;
}

export interface SelectedRunBudgetLimits {
  cnyLimit: string;
  usdLimit: string;
}

interface RunBudgetSettings {
  runId: string;
  cnyLimit: string;
  usdLimit: string;
  revision: number;
}

export interface LegacyBillingReconciliation {
  revision: number;
  merge: {
    movedRequests: number;
    movedWarnings: number;
    settings: RunBudgetSettings | null;
  };
}

/** 读取本地 SQLite 周目目录摘要，不打开记录、不修复文件或发送请求。 */
export function listRuns(cursor?: RunCursor, pageSize = 50): Promise<RunPage> {
  return invoke("run_catalog_page", { cursor: cursor ?? null, pageSize });
}

/** 按目录 revision 显式恢复既有周目，并返回该周目冻结的公开 profile。 */
export function openSelectedRun(sessionId: string, expectedRevision: number): Promise<OpenedRun> {
  return invoke("run_open_selected", { sessionId, expectedRevision });
}

/** 玩家确认后推进删除；同一操作编号可重试，大目录会返回待继续清理阶段。 */
export function deleteRun(
  operationId: string,
  sessionId: string,
  expectedRevision: number,
): Promise<DeletionOperation> {
  return invoke("run_delete", { operationId, sessionId, expectedRevision });
}

/** 玩家主动推进一个旧周目发现批次；普通列表读取不会触发扫描。 */
export function scanLegacyRuns(): Promise<LegacyScanResult> {
  return invoke("run_scan_legacy");
}

/** 显式绑定旧周目的公开 profile 快照；revision 必须来自当前目录页。 */
export function bindRunProfile(
  sessionId: string,
  expectedRevision: number,
  profileId: string,
): Promise<number> {
  return invoke("run_bind_profile", {
    sessionId,
    expectedRevision,
    profileId,
  });
}

/** 合并经目录身份核验的历史账务；仅预算冲突时提交玩家选定的最终上限。 */
export function reconcileRunBilling(
  sessionId: string,
  expectedRevision: number,
  selectedLimits?: SelectedRunBudgetLimits,
): Promise<LegacyBillingReconciliation> {
  return invoke("run_reconcile_billing", {
    sessionId,
    expectedRevision,
    selectedLimits: selectedLimits ?? null,
  });
}
