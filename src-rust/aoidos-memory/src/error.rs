//! 领域拒绝与存储失败分开；诊断不包含文件路径、正文或第三方错误原文。

use aoidos_store::error::StoreError;
use serde::{Deserialize, Serialize};

/// 本地拒绝原因；不能把容量或来源错误冒充供应商配额错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    InvalidSchema,
    InvalidSource,
    StaleVersion,
    CapacityBlocked,
    PolicyUnavailable,
    RecoveryRequired,
}
/// 存储端口错误；壳层只映射稳定类别。
pub enum Error {
    Rejected(Reason),
    NotFound,
    Corrupt,
    Store(StoreError),
}
pub type Result<T> = std::result::Result<T, Error>;
impl From<StoreError> for Error {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(reason) => write!(f, "memory rejected: {reason:?}"),
            Self::NotFound => f.write_str("memory identity not found"),
            Self::Corrupt => f.write_str("memory state is corrupt"),
            Self::Store(error) => write!(f, "memory storage: {}", error.code()),
        }
    }
}
impl std::error::Error for Error {}

pub(crate) fn sql(error: rusqlite::Error) -> Error {
    Error::Store(aoidos_store::db::sqlite_error(error))
}
pub(crate) fn invalid_json(_: aoidos_json::Error) -> Error {
    Error::Corrupt
}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl Error {
    /// 稳定命令码；具体本地拒绝原因保留在 Rejected 中，壳可映射脱敏 detail.reason。
    pub fn code(&self) -> String {
        match self {
            Self::Store(error) => format!("store.{}", error.code()),
            Self::Corrupt => "store.corrupt".into(),
            Self::NotFound => "app.not-found".into(),
            Self::Rejected(Reason::PolicyUnavailable | Reason::RecoveryRequired) => {
                "app.not-ready".into()
            }
            Self::Rejected(_) => "app.bad-request".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_codes_preserve_typed_reasons_without_storage_details() {
        for (reason, expected) in [
            (Reason::InvalidSchema, "app.bad-request"),
            (Reason::InvalidSource, "app.bad-request"),
            (Reason::StaleVersion, "app.bad-request"),
            (Reason::CapacityBlocked, "app.bad-request"),
            (Reason::PolicyUnavailable, "app.not-ready"),
            (Reason::RecoveryRequired, "app.not-ready"),
        ] {
            let error = Error::Rejected(reason);
            assert_eq!(error.code(), expected);
            assert!(matches!(error, Error::Rejected(actual) if actual == reason));
        }
        assert_eq!(Error::Corrupt.code(), "store.corrupt");
        assert_eq!(Error::NotFound.code(), "app.not-found");
        let error = Error::Store(StoreError::Corrupt("private path and body".into()));
        assert_eq!(error.code(), "store.corrupt");
        assert!(!error.code().contains("private"));
    }

    #[test]
    fn public_diagnostics_hide_underlying_storage_and_json_content() {
        for reason in [
            Reason::InvalidSchema,
            Reason::InvalidSource,
            Reason::StaleVersion,
            Reason::CapacityBlocked,
            Reason::PolicyUnavailable,
            Reason::RecoveryRequired,
        ] {
            assert_eq!(
                Error::Rejected(reason).to_string(),
                format!("memory rejected: {reason:?}")
            );
        }
        assert_eq!(Error::NotFound.to_string(), "memory identity not found");
        assert_eq!(Error::Corrupt.to_string(), "memory state is corrupt");
        let wrapped = Error::from(StoreError::Corrupt("/private/path secret body".into()));
        assert_eq!(wrapped.to_string(), "memory storage: corrupt");
        assert_eq!(format!("{wrapped:?}"), "memory storage: corrupt");
        assert!(std::error::Error::source(&wrapped).is_none());
        let sqlite = sql(rusqlite::Error::InvalidColumnName("secret column".into()));
        assert!(matches!(sqlite, Error::Store(_)));
        assert!(!format!("{sqlite:?}").contains("secret"));
        assert!(matches!(
            invalid_json(aoidos_json::Error::InvalidJson),
            Error::Corrupt
        ));
    }
}
