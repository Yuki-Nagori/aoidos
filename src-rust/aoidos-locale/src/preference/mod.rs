//! 单项语言偏好表；读写使用由 engine 持有的共享 SQLite 连接。

use aoidos_store::error::{Result, StoreError};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocaleChoice {
    #[serde(rename = "system")]
    System,
    #[serde(rename = "zh-Hans")]
    ZhHans,
    #[serde(rename = "en")]
    En,
}

impl LocaleChoice {
    pub fn resolve(self) -> Locale {
        match self {
            Self::System => system_locale(),
            Self::ZhHans => Locale::ZhHans,
            Self::En => Locale::En,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Locale {
    #[serde(rename = "zh-Hans")]
    ZhHans,
    #[serde(rename = "en")]
    En,
}

impl Locale {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ZhHans => "zh-Hans",
            Self::En => "en",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalePreference {
    pub version: u32,
    pub locale: LocaleChoice,
}

impl Default for LocalePreference {
    fn default() -> Self {
        Self {
            version: 1,
            locale: LocaleChoice::System,
        }
    }
}

/// 返回持久化的偏好；不存在时使用 system，但不插入默认行。
///
/// # Errors
/// 未知 schema 版本或存储值返回 store.corrupt。
pub fn get(connection: &Connection) -> Result<LocalePreference> {
    let stored: Option<(i64, String)> = connection
        .query_row(
            "SELECT version, locale FROM locale_preference WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(aoidos_store::db::sqlite_error)?;
    match stored {
        None => Ok(LocalePreference::default()),
        Some((1, locale)) => {
            parse_choice(&locale).map(|locale| LocalePreference { version: 1, locale })
        }
        Some(_) => Err(corrupt()),
    }
}

/// 事务化替换语言选项，不改写其他偏好。
///
/// # Errors
/// 非当前版本或 SQLite 失败返回 store.*。
pub fn set(connection: &mut Connection, preference: LocalePreference) -> Result<LocalePreference> {
    if preference.version != 1 {
        return Err(corrupt());
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(aoidos_store::db::sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO locale_preference (id, version, locale) VALUES (1, 1, ?1) ON CONFLICT(id) DO UPDATE SET version=excluded.version, locale=excluded.locale",
            [choice_str(preference.locale)],
        )
        .map_err(aoidos_store::db::sqlite_error)?;
    transaction
        .commit()
        .map_err(aoidos_store::db::sqlite_error)?;
    Ok(preference)
}

/// 规范化受信任的 OS 语言标记；只接受产品已翻译的简体中文与英语。
pub fn resolve_language_tag(tag: Option<&str>) -> Locale {
    let language = tag
        .unwrap_or_default()
        .split(['-', '_'])
        .next()
        .unwrap_or_default();
    if language.eq_ignore_ascii_case("zh") {
        Locale::ZhHans
    } else {
        Locale::En
    }
}

fn system_locale() -> Locale {
    resolve_language_tag(sys_locale::get_locale().as_deref())
}

fn choice_str(choice: LocaleChoice) -> &'static str {
    match choice {
        LocaleChoice::System => "system",
        LocaleChoice::ZhHans => "zh-Hans",
        LocaleChoice::En => "en",
    }
}

fn parse_choice(value: &str) -> Result<LocaleChoice> {
    match value {
        "system" => Ok(LocaleChoice::System),
        "zh-Hans" => Ok(LocaleChoice::ZhHans),
        "en" => Ok(LocaleChoice::En),
        _ => Err(corrupt()),
    }
}

fn corrupt() -> StoreError {
    StoreError::Corrupt("locale preference is invalid".into())
}

#[cfg(test)]
mod tests;
