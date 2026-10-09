//! 存储域错误：`code()` 返回裸码，调用方加 `store.` 前缀（契约见 ai-docs/architecture/ipc-contract.md）。
//! 前端按码分支、不匹配 Display 文本。

use std::fmt;
use std::io;
use std::path::PathBuf;

/// 存储失败。调用方匹配 [`StoreError::code`]，不匹配 Display 文本。
#[derive(Debug)]
pub enum StoreError {
    /// 路径非法：穿越、分隔符、盘符、空组件或消毒后为空。
    InvalidPath(String),
    /// 另一个实例持有存储锁。
    AlreadyRunning { lock_path: PathBuf },
    /// 目标被占用，重试耗尽后放弃（tmp 已清理）。
    LockedTimeout { path: PathBuf },
    /// 迁移执行失败；事务已回滚，`user_version` 未推进。
    Migration {
        version: u32,
        source: rusqlite::Error,
    },
    /// 数据库 schema 新于当前二进制（降级打开）。
    Corrupt(String),
    /// I/O 或 SQLite 失败。`code` 为 `disk-full`、`permission`、`not-found`、`locked`、`corrupt` 或 `io`。
    Io {
        code: &'static str,
        source: io::Error,
    },
}

impl StoreError {
    /// 判别码，命名空间 `store` 由调用方约定前缀。
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidPath(_) => "invalid-path",
            Self::AlreadyRunning { .. } => "already-running",
            Self::LockedTimeout { .. } => "locked",
            Self::Migration { .. } => "migration",
            Self::Corrupt(_) => "corrupt",
            Self::Io { code, .. } => code,
        }
    }

    /// 磁盘满、权限、找不到各有稳定码。其它 Kind 收成 `io`，避免把平台专有
    /// Kind 漏到前端。全仓唯一的 io → `StoreError` 映射，业务 crate 复用。
    #[must_use]
    pub fn from_io(source: io::Error) -> Self {
        let code = match source.kind() {
            io::ErrorKind::StorageFull => "disk-full",
            io::ErrorKind::PermissionDenied => "permission",
            io::ErrorKind::NotFound => "not-found",
            _ => "io",
        };
        Self::Io { code, source }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(raw) => write!(f, "invalid path component: {raw:?}"),
            Self::AlreadyRunning { lock_path } => {
                write!(f, "another instance holds {}", lock_path.display())
            }
            Self::LockedTimeout { path } => {
                write!(f, "target locked too long: {}", path.display())
            }
            Self::Migration { version, source } => {
                write!(f, "migration to version {version} failed: {source}")
            }
            Self::Corrupt(msg) => write!(f, "corrupt storage: {msg}"),
            Self::Io { code, source } => write!(f, "storage io error [{code}]: {source}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Migration { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_cover_all_variants() {
        assert_eq!(StoreError::InvalidPath("x".into()).code(), "invalid-path");
        assert_eq!(
            StoreError::AlreadyRunning {
                lock_path: PathBuf::from("l")
            }
            .code(),
            "already-running"
        );
        assert_eq!(
            StoreError::LockedTimeout {
                path: PathBuf::from("t")
            }
            .code(),
            "locked"
        );
        let migration = StoreError::Migration {
            version: 2,
            source: rusqlite::Error::InvalidColumnName("bad".into()),
        };
        assert_eq!(migration.code(), "migration");
        assert_eq!(StoreError::Corrupt("old".into()).code(), "corrupt");
        assert_eq!(
            StoreError::from_io(io::Error::from(io::ErrorKind::StorageFull)).code(),
            "disk-full"
        );
        assert_eq!(
            StoreError::from_io(io::Error::from(io::ErrorKind::PermissionDenied)).code(),
            "permission"
        );
        assert_eq!(
            StoreError::from_io(io::Error::from(io::ErrorKind::NotFound)).code(),
            "not-found"
        );
        assert_eq!(
            StoreError::from_io(io::Error::from(io::ErrorKind::Other)).code(),
            "io"
        );
    }

    #[test]
    fn display_covers_all_variants() {
        let errors = [
            StoreError::InvalidPath("?".into()),
            StoreError::AlreadyRunning {
                lock_path: PathBuf::from("l"),
            },
            StoreError::LockedTimeout {
                path: PathBuf::from("t"),
            },
            StoreError::Migration {
                version: 1,
                source: rusqlite::Error::InvalidColumnName("bad".into()),
            },
            StoreError::Corrupt("c".into()),
            StoreError::from_io(io::Error::from(io::ErrorKind::Other)),
        ];
        for e in &errors {
            assert_ne!(e.to_string(), "");
        }
    }

    #[test]
    fn source_is_some_only_for_wrapped_errors() {
        let migration = StoreError::Migration {
            version: 1,
            source: rusqlite::Error::InvalidColumnName("bad".into()),
        };
        assert!(std::error::Error::source(&migration).is_some());
        let io = StoreError::from_io(io::Error::from(io::ErrorKind::Other));
        assert!(std::error::Error::source(&io).is_some());
        assert!(std::error::Error::source(&StoreError::Corrupt("c".into())).is_none());
    }
}
