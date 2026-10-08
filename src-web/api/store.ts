import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

/** 命令拒绝时的统一形状；只按 code 分支，detail 为可选诊断上下文。 */
export interface CmdError {
  code: string;
  message: string;
  detail?: unknown;
}

/** 对应 Rust BackupItem；nanos 为 Unix epoch 纳秒的十进制字符串。 */
export interface BackupItem {
  path: string;
  version: number;
  nanos: string;
  size: number;
}

/** 对应 Rust BackupList；按时间戳、版本从新到旧排列，最多 50 项。 */
export interface BackupList {
  items: BackupItem[];
}

/** Rust 迁移流真实状态；事件投递错误不改变数据库终态。 */
export interface MigrationSnapshot {
  phase: "idle" | "running" | "completed" | "failed";
  migrationId?: string;
  from?: number;
  to: number;
  current?: number;
  lastStep?: { from: number; to: number };
  failedStep?: { from: number; to: number };
  seq: { progress: number; done: number; failed: number };
  error?: { code: string; message: string };
  deliveryError?: { code: "app.event-failed"; message: string };
}

/** 两个持久偏好；新 round 冻结后不受本次写入影响。 */
export interface UiPreferences {
  version: 1;
  panelPinned: boolean;
  diceMode: "manual" | "auto";
}

/** 读取首次默认或已持久化偏好。 */
export function getUiPreferences(): Promise<UiPreferences> {
  return invoke("store_get_ui_preferences");
}

/** 完整替换两个可写字段，成功响应即为持久化确认。 */
export function setUiPreferences(
  panelPinned: boolean,
  diceMode: UiPreferences["diceMode"],
): Promise<UiPreferences> {
  return invoke("store_set_ui_preferences", { panelPinned, diceMode });
}

/** 获取最新 50 项备份；invoke 只透传返回值，不执行运行时校验。 */
export function listBackups(): Promise<BackupList> {
  return invoke("store_list_backups");
}

/** 读取当前迁移诊断；不会启动迁移，业务库被冻结时仍可查询。 */
export function getMigration(): Promise<MigrationSnapshot> {
  return invoke("store_get_migration");
}

/** 对应 Rust MigrationEvent；每种事件独立排序，快照负责缺口恢复。 */
export interface MigrationEvents {
  progress: { migrationId: string; from: number; to: number; current: number; target: number };
  done: { migrationId: string; from: number; to: number; current: number };
  failed: {
    migrationId: string;
    from?: number;
    to: number;
    current?: number;
    failedStep?: { from: number; to: number };
    code: string;
    message: string;
  };
}

/** 完整透传事件载荷；薄 API 不把运行期校验混入 invoke / listen。 */
export function listenMigration<K extends keyof MigrationEvents>(
  kind: K,
  handler: (event: { seq: number; data: MigrationEvents[K] }) => void,
): Promise<UnlistenFn> {
  return listen<{ seq: number; data: MigrationEvents[K] }>(
    `store:migration:${kind}`,
    (event) => handler(event.payload),
    { target: { kind: "AnyLabel", label: "main" } },
  );
}
