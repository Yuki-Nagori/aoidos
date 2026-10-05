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

/** 当前没有运行迁移任务，from 与 to 均为已持久化版本；缺失库为 0。 */
export interface MigrationSnapshot {
  from: number;
  to: number;
  phase: "idle";
}

/** 获取最新 50 项备份；invoke 只透传返回值，不执行运行时校验。 */
export function listBackups(): Promise<BackupList> {
  return invoke("store_list_backups");
}

/** 读取静态版本快照；不会创建库或执行迁移，失败时原样拒绝 CmdError。 */
export function getMigration(): Promise<MigrationSnapshot> {
  return invoke("store_get_migration");
}
