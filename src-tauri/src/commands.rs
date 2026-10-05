//! 命令层：解参数、调逻辑、回包；逻辑文件计入覆盖率门禁（装配 lib.rs 不计）。

use crate::ipc::CmdError;
use mythos_store::db;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 示例命令：前端 `invoke("greet", { name })` 调用。契约要求所有命令统一
/// `Result<T, CmdError>` 形状；本命令当前无失败路径。
/// store 域命令（`store_list_backups` / `store_get_migration`）已就位，
/// 本占位在 008 界面落地、前端改用真实命令时退役。
#[tauri::command]
pub fn greet(name: &str) -> Result<String, CmdError> {
    Ok(format!("Hello, {name}! You've been greeted from Rust!"))
}

// 应用数据目录下的业务库路径：lib.rs setup 注入（OnceLock 单例，进程内只设一次）。
static DB_PATH: OnceLock<PathBuf> = OnceLock::new();
const MAX_BACKUP_ITEMS: usize = 50;

/// setup 注入业务库路径；重复注入忽略。
pub(crate) fn init_db_path(db: PathBuf) {
    let _ = DB_PATH.set(db);
}

/// 业务库路径；setup 未注入（理论上不可达）时报 `app.not-ready`。
fn db_path() -> Result<&'static PathBuf, CmdError> {
    DB_PATH.get().ok_or_else(not_ready)
}

fn not_ready() -> CmdError {
    CmdError::new("app.not-ready", "存储尚未初始化", None)
}

/// IPC 备份条目；nanos 是 Unix epoch 纳秒的十进制字符串，避免 JS number 丢失精度。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupItem {
    path: String,
    version: u32,
    nanos: String,
    size: u64,
}

/// 最多返回最新 50 个备份；按时间戳、版本从新到旧排序。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupList {
    items: Vec<BackupItem>,
}

/// 当前未运行迁移任务的版本快照，不提供在飞流程状态或事件 seq 基线。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationSnapshot {
    from: u32,
    to: u32,
    phase: MigrationPhase,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum MigrationPhase {
    Idle,
}

/// 列出最新迁移备份，最多 50 项；备份目录不存在时 items 为空。
///
/// # Errors
///
/// 路径尚未注入时返回 app.not-ready；枚举失败返回对应 store.* 错误。
#[tauri::command]
pub fn store_list_backups() -> Result<BackupList, CmdError> {
    list_backups_payload(db_path()?)
}

fn list_backups_payload(db_path: &Path) -> Result<BackupList, CmdError> {
    let items = db::list_backups(db_path)
        .map_err(CmdError::from)?
        .into_iter()
        .take(MAX_BACKUP_ITEMS)
        .map(|backup| BackupItem {
            path: backup.path.display().to_string(),
            version: backup.version,
            nanos: backup.nanos.to_string(),
            size: backup.size,
        })
        .collect();
    Ok(BackupList { items })
}

/// 只读版本快照：from == to == user_version，phase 为 idle；新安装返回版本 0。
/// 不创建数据库或目录。006 接入真实迁移流时再提供运行阶段与各事件的 seq 基线。
///
/// # Errors
///
/// 路径尚未注入时返回 app.not-ready；读取失败返回对应 store.* 错误。
#[tauri::command]
pub fn store_get_migration() -> Result<MigrationSnapshot, CmdError> {
    migration_snapshot(db_path()?)
}

fn migration_snapshot(db_path: &Path) -> Result<MigrationSnapshot, CmdError> {
    let version = db::current_version(db_path).map_err(CmdError::from)?;
    Ok(MigrationSnapshot {
        from: version,
        to: version,
        phase: MigrationPhase::Idle,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mythos_store::db;

    fn tdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mythos-cmds-{}-{}",
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn greets_the_given_name() {
        assert_eq!(
            greet("Tauri").unwrap(),
            "Hello, Tauri! You've been greeted from Rust!"
        );
    }

    #[test]
    fn not_ready_maps_to_dedicated_code() {
        assert_eq!(not_ready().code(), "app.not-ready");
    }

    #[test]
    fn store_commands_round_trip() {
        let dir = tdir("round-trip");
        let db_path = dir.join("storage.sqlite");
        init_db_path(db_path.clone());

        let empty = serde_json::to_value(store_list_backups().unwrap()).unwrap();
        assert_eq!(empty["items"].as_array().unwrap().len(), 0);
        let snapshot = serde_json::to_value(store_get_migration().unwrap()).unwrap();
        assert_eq!(snapshot["from"], 0);
        assert_eq!(
            snapshot,
            serde_json::json!({ "from": 0, "to": 0, "phase": "idle" })
        );
        assert!(!db_path.exists(), "snapshot must not create the database");

        let migrations = ["CREATE TABLE heroes(id INTEGER PRIMARY KEY);"];
        let conn = db::open(&db_path, &migrations).unwrap();
        drop(conn);
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(backups.join("storage-v1-100.sqlite"), b"a").unwrap();
        std::fs::write(backups.join("storage-v1-200.sqlite"), b"bb").unwrap();

        let listed = serde_json::to_value(store_list_backups().unwrap()).unwrap();
        let items = listed["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["size"], 2);
        assert_eq!(items[0]["nanos"], "200");
        assert_eq!(items[0]["version"], 1);
        let snapshot = serde_json::to_value(store_get_migration().unwrap()).unwrap();
        assert_eq!(snapshot["from"], 1);

        // 重复注入忽略：仍读原库（OnceLock 单例语义）。
        init_db_path(dir.join("other.sqlite"));
        let snapshot = serde_json::to_value(store_get_migration().unwrap()).unwrap();
        assert_eq!(snapshot["from"], 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn snapshot_of_fresh_install_does_not_create_data_directory() {
        let dir = tdir("fresh-install");
        let parent = dir.join("missing");
        let value =
            serde_json::to_value(migration_snapshot(&parent.join("storage.sqlite")).unwrap())
                .unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "from": 0, "to": 0, "phase": "idle" })
        );
        assert!(!parent.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn backup_payload_preserves_nanos_and_limits_results() {
        let dir = tdir("limit");
        let backups = dir.join("backups");
        std::fs::create_dir(&backups).unwrap();
        let base = 1_700_000_000_000_000_000_u128;
        for offset in 0..51 {
            std::fs::write(
                backups.join(format!("storage-v1-{}.sqlite", base + offset)),
                b"x",
            )
            .unwrap();
        }
        let value =
            serde_json::to_value(list_backups_payload(&dir.join("storage.sqlite")).unwrap())
                .unwrap();
        let items = value["items"].as_array().unwrap();
        assert_eq!(items.len(), 50);
        assert_eq!(items[0]["nanos"], (base + 50).to_string());
        assert_eq!(items[49]["nanos"], (base + 1).to_string());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
