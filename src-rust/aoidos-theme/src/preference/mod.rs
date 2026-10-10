//! 独立主题偏好表；调用者借用业务 SQLite 连接，不创建第二写者。

use aoidos_store::error::{Result, StoreError};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemePreference {
    pub version: u32,
    pub theme: String,
}

impl Default for ThemePreference {
    fn default() -> Self {
        Self {
            version: 1,
            theme: "dark".into(),
        }
    }
}

/// 读取已确认的偏好；未设置时返回 dark，不会创建或修改数据。
///
/// # Errors
/// 不支持的持久版本或非法主题值返回 store.corrupt。
pub fn get(connection: &Connection) -> Result<ThemePreference> {
    let stored: Option<(i64, String)> = connection
        .query_row(
            "SELECT version, theme FROM theme_preference WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(aoidos_store::db::sqlite_error)?;
    match stored {
        None => Ok(ThemePreference::default()),
        Some((1, theme)) => parse_theme(&theme),
        Some(_) => Err(corrupt()),
    }
}

/// 单项事务替换主题偏好，避免改写其它产品设置。
///
/// # Errors
/// 事务失败返回对应 store 错误。
pub fn set(connection: &mut Connection, theme: String) -> Result<ThemePreference> {
    if !crate::catalog::themes().is_some_and(|themes| themes.iter().any(|item| item.id == theme)) {
        return Err(corrupt());
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(aoidos_store::db::sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO theme_preference (id, version, theme) VALUES (1, 1, ?1) ON CONFLICT(id) DO UPDATE SET version=excluded.version, theme=excluded.theme",
            [&theme],
        )
        .map_err(aoidos_store::db::sqlite_error)?;
    transaction
        .commit()
        .map_err(aoidos_store::db::sqlite_error)?;
    Ok(ThemePreference { version: 1, theme })
}

fn parse_theme(theme: &str) -> Result<ThemePreference> {
    if !crate::catalog::valid_theme_id(theme) {
        return Err(corrupt());
    }
    Ok(ThemePreference {
        version: 1,
        theme: theme.to_owned(),
    })
}

fn corrupt() -> StoreError {
    StoreError::Corrupt("theme preference is invalid".into())
}

#[cfg(test)]
mod tests;
