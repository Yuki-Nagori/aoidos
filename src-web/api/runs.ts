import { invoke } from "@tauri-apps/api/core";

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
}

export interface RunPage {
  entries: RunSummary[];
  nextCursor: RunCursor | null;
  catalogRevision: number;
  scanIncomplete: boolean;
  scanIssueCount: number;
}

export interface LegacyScanResult {
  discovered: number;
  diagnosed: number;
  scanIncomplete: boolean;
  scanIssueCount: number;
}

/** 读取本地 SQLite 周目目录摘要，不打开记录、不修复文件或发送请求。 */
export function listRuns(cursor?: RunCursor, pageSize = 50): Promise<RunPage> {
  return invoke("run_catalog_page", { cursor: cursor ?? null, pageSize });
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
