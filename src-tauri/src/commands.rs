//! 命令层：解参数、调逻辑、回包；逻辑文件计入覆盖率门禁（装配 lib.rs 不计）。

use crate::ipc::CmdError;
use mythos_store::db;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 示例命令：前端 `invoke("greet", { name })` 调用。首个真实命令落地时替换。
/// 契约要求所有命令统一 `Result<T, CmdError>` 形状；本命令当前无失败路径。
#[tauri::command]
pub fn greet(name: &str) -> Result<String, CmdError> {
    Ok(format!("Hello, {name}! You've been greeted from Rust!"))
}

// 应用数据目录下的业务库路径：lib.rs setup 注入（OnceLock 单例，进程内只设一次）。
static DB_PATH: OnceLock<PathBuf> = OnceLock::new();

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

/// 列出迁移备份（新到旧）。契约正例：`store_list_backups {}` → `{ items }`。
#[tauri::command]
pub fn store_list_backups() -> Result<serde_json::Value, CmdError> {
    list_backups_payload(db_path()?)
}

fn list_backups_payload(db_path: &Path) -> Result<serde_json::Value, CmdError> {
    let items: Vec<serde_json::Value> = db::list_backups(db_path)
        .map_err(CmdError::from)?
        .iter()
        .map(|backup| {
            serde_json::json!({
                "path": backup.path.display().to_string(),
                "version": backup.version,
                "size": backup.size,
            })
        })
        .collect();
    Ok(serde_json::json!({ "items": items }))
}

/// 当前迁移版本快照（from == to == user_version）。
#[tauri::command]
pub fn store_get_migration() -> Result<serde_json::Value, CmdError> {
    migration_snapshot(db_path()?)
}

fn migration_snapshot(db_path: &Path) -> Result<serde_json::Value, CmdError> {
    let version = db::current_version(db_path).map_err(CmdError::from)?;
    Ok(serde_json::json!({ "from": version, "to": version }))
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

        let empty = store_list_backups().unwrap();
        assert_eq!(empty["items"].as_array().unwrap().len(), 0);
        let snapshot = store_get_migration().unwrap();
        assert_eq!(snapshot["from"], 0);
        assert_eq!(snapshot["to"], 0);

        let migrations = ["CREATE TABLE heroes(id INTEGER PRIMARY KEY);"];
        let conn = db::open(&db_path, &migrations).unwrap();
        drop(conn);
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(backups.join("storage-v1-100.sqlite"), b"a").unwrap();
        std::fs::write(backups.join("storage-v1-200.sqlite"), b"bb").unwrap();

        let listed = store_list_backups().unwrap();
        let items = listed["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["size"], 2);
        let snapshot = store_get_migration().unwrap();
        assert_eq!(snapshot["from"], 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
