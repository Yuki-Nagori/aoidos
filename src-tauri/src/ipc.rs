//! 命令层错误形状：`CmdError` 序列化为 `{ code, message, detail? }`，
//! 命名空间与映射规则见 ai-docs/architecture/ipc-contract.md。

use mythos_store::error::StoreError;
use serde::Serialize;

/// 所有命令的统一错误形状。`code` 是前端分支的唯一依据；
/// `message` 是可展示中文；`detail` 为 `None` 时不出现在序列化结果里。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CmdError {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<serde_json::Value>,
}

impl From<StoreError> for CmdError {
    fn from(err: StoreError) -> Self {
        let detail = match &err {
            StoreError::InvalidPath(raw) => Some(serde_json::json!({ "path": raw })),
            StoreError::LockedTimeout { path } => {
                Some(serde_json::json!({ "path": path.display().to_string() }))
            }
            // 契约看门狗表：迁移失败「原因放 detail」。
            StoreError::Migration { version, source } => Some(serde_json::json!({
                "version": version,
                "reason": source.to_string(),
            })),
            _ => None,
        };
        Self {
            code: format!("store.{}", err.code()),
            message: store_message(err.code()),
            detail,
        }
    }
}

/// `StoreError::code()` 的九个返回值 → 可展示中文。新增码时在此补一行。
fn store_message(code: &str) -> String {
    let text = match code {
        "invalid-path" => "路径不合法",
        "already-running" => "应用已在运行",
        "locked" => "目标被占用，请稍后重试",
        "migration" => "数据库迁移失败",
        "disk-full" => "磁盘空间不足",
        "permission" => "没有操作权限",
        "not-found" => "文件或目录不存在",
        "corrupt" => "存储数据损坏",
        _ => "存储读写失败",
    };
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn err_for(code: &'static str) -> StoreError {
        match code {
            "invalid-path" => StoreError::InvalidPath("p".into()),
            "already-running" => StoreError::AlreadyRunning {
                lock_path: "l".into(),
            },
            "locked" => StoreError::LockedTimeout { path: "t".into() },
            "migration" => StoreError::Migration {
                version: 1,
                source: rusqlite::Error::InvalidColumnName("x".into()),
            },
            "disk-full" => StoreError::Io {
                code,
                source: io::Error::from(io::ErrorKind::StorageFull),
            },
            "permission" => StoreError::Io {
                code,
                source: io::Error::from(io::ErrorKind::PermissionDenied),
            },
            "not-found" => StoreError::Io {
                code,
                source: io::Error::from(io::ErrorKind::NotFound),
            },
            "corrupt" => StoreError::Corrupt("c".into()),
            _ => StoreError::Io {
                code,
                source: io::Error::other("storage io"),
            },
        }
    }

    #[test]
    fn from_store_error_prefixes_and_maps_all_codes() {
        for code in [
            "invalid-path",
            "already-running",
            "locked",
            "migration",
            "disk-full",
            "permission",
            "not-found",
            "corrupt",
            "io",
        ] {
            let cmd: CmdError = err_for(code).into();
            assert_eq!(cmd.code, format!("store.{code}"));
            assert!(!cmd.message.is_empty());
        }
    }

    #[test]
    fn cmd_error_serializes_contract_shape() {
        let with_detail: CmdError = StoreError::LockedTimeout { path: "t".into() }.into();
        let value = serde_json::to_value(&with_detail).unwrap();
        assert_eq!(value["code"], "store.locked");
        assert_eq!(value["detail"]["path"], "t");

        let plain: CmdError = StoreError::Corrupt("c".into()).into();
        let value = serde_json::to_value(&plain).unwrap();
        assert_eq!(value["code"], "store.corrupt");
        assert!(
            value.get("detail").is_none(),
            "detail 为 None 时字段必须省略"
        );
    }

    #[test]
    fn migration_reason_lands_in_detail() {
        // 契约看门狗表：迁移失败「原因放 detail」。
        let cmd: CmdError = err_for("migration").into();
        let detail = cmd.detail.unwrap();
        assert_eq!(detail["version"], 1);
        assert!(detail["reason"].as_str().is_some());
    }
}
