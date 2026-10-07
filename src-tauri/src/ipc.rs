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

impl CmdError {
    /// 判别码（含域前缀），与序列化后的 `code` 字段一致。
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    /// 命令层自建错误（`app.*` 命名空间）的统一入口；域错误走各自的 `From`。
    pub(crate) fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        detail: Option<serde_json::Value>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail,
        }
    }
}

impl From<StoreError> for CmdError {
    fn from(err: StoreError) -> Self {
        let code = err.code();
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
            StoreError::AlreadyRunning { lock_path } => {
                Some(serde_json::json!({ "lockPath": lock_path.display().to_string() }))
            }
            // 损坏原因区分处置方式：库新于二进制 → 升级应用；文件损坏 → 恢复备份。
            StoreError::Corrupt(reason) => Some(serde_json::json!({ "reason": reason })),
            _ => None,
        };
        Self::new(format!("store.{code}"), store_message(code), detail)
    }
}

/// 平台投递失败的诊断上下文；载荷序列化和序号错误由 events 模块处理。
pub(crate) fn event_delivery_error(event: &str, source: String) -> CmdError {
    CmdError::new(
        "app.event-failed",
        format!("事件 {event} 发送失败"),
        Some(serde_json::json!({ "event": event, "source": source })),
    )
}

/// `StoreError::code()` 的九个返回值 → 可展示中文。新增码先在
/// ai-docs/architecture/ipc-contract.md 错误码目录登记，再在此补一行。
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
    fn store_messages_are_display_chinese_not_codes() {
        // message 是可展示中文，不参与前端分支；新增码须按注释先登记契约再补行。
        let cases = [
            ("invalid-path", "路径不合法"),
            ("already-running", "应用已在运行"),
            ("locked", "目标被占用，请稍后重试"),
            ("migration", "数据库迁移失败"),
            ("disk-full", "磁盘空间不足"),
            ("permission", "没有操作权限"),
            ("not-found", "文件或目录不存在"),
            ("corrupt", "存储数据损坏"),
            ("io", "存储读写失败"),
        ];
        for (code, text) in cases {
            assert_eq!(store_message(code), text, "code {code}");
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

        // Io 类错误不携带 detail；detail 为 None 时字段必须省略。
        let plain: CmdError = StoreError::Io {
            code: "io",
            source: io::Error::other("storage io"),
        }
        .into();
        assert_eq!(plain.code(), "store.io");
        let value = serde_json::to_value(&plain).unwrap();
        assert_eq!(value["code"], "store.io");
        assert!(
            value.get("detail").is_none(),
            "detail 为 None 时字段必须省略"
        );
    }

    #[test]
    fn detail_carries_variant_context() {
        // AlreadyRunning 带锁文件路径；Corrupt 带损坏原因（区分「库新于二进制 → 升级应用」与「文件损坏 → 恢复备份」）。
        let running: CmdError = StoreError::AlreadyRunning {
            lock_path: "l".into(),
        }
        .into();
        assert_eq!(running.detail.as_ref().unwrap()["lockPath"], "l");

        let corrupt: CmdError = StoreError::Corrupt("库新于二进制".into()).into();
        assert_eq!(corrupt.detail.as_ref().unwrap()["reason"], "库新于二进制");
    }

    #[test]
    fn migration_reason_lands_in_detail() {
        // 契约看门狗表：迁移失败「原因放 detail」。
        let cmd: CmdError = err_for("migration").into();
        let detail = cmd.detail.unwrap();
        assert_eq!(detail["version"], 1);
        assert!(detail["reason"].as_str().is_some());
    }

    #[test]
    fn event_delivery_error_serializes_context() {
        let error = event_delivery_error("store:migration:progress", "platform failure".into());
        let value = serde_json::to_value(error).unwrap();
        assert_eq!(value["code"], "app.event-failed");
        assert_eq!(value["detail"]["event"], "store:migration:progress");
        assert_eq!(value["detail"]["source"], "platform failure");
        let invalid: CmdError = StoreError::InvalidPath("original<>path".into()).into();
        assert_eq!(
            serde_json::to_value(invalid).unwrap()["detail"]["path"],
            "original<>path"
        );
    }
}
