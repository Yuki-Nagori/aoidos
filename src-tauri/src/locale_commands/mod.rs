//! 界面语言命令持久化受限偏好；原生适配失败仍确认已提交选项。

use crate::{ipc::CmdError, store_commands::StorageService};
use aoidos_locale::preference::{Locale, LocalePreference};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::menu::Menu;

include!(concat!(env!("OUT_DIR"), "/native_locales.rs"));

mod native;

pub struct LocaleService {
    current: Mutex<Locale>,
    gate: tokio::sync::Mutex<()>,
    native: Box<dyn NativeLocale + Send + Sync>,
}

pub(super) trait NativeLocale {
    fn apply(&self, locale: Locale) -> NativeStatus;
    fn window_menu(&self) -> Menu<tauri::Wry>;
}

impl LocaleService {
    fn new(current: Locale, native: Box<dyn NativeLocale + Send + Sync>) -> Self {
        Self {
            current: Mutex::new(current),
            gate: tokio::sync::Mutex::new(()),
            native,
        }
    }

    pub fn window_menu(&self) -> Menu<tauri::Wry> {
        self.native.window_menu()
    }

    pub fn current(&self) -> Locale {
        *self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    async fn serialize(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.gate.lock().await
    }

    fn confirm(&self, locale: Locale) {
        *self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = locale;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocaleResult {
    preference: LocalePreference,
    resolved_locale: Locale,
    native_status: NativeStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NativeStatus {
    Applied,
    Pending,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetLocale {
    preference: LocalePreference,
}

/// 读取已保存选项并重申平台文案；不轮询系统语言或重写缺省值。
///
/// # Errors
/// 存储迁移或读取失败返回 store 域错误。
#[tauri::command]
pub async fn locale_get_preference(
    service: tauri::State<'_, LocaleService>,
    storage: tauri::State<'_, StorageService>,
) -> Result<LocaleResult, CmdError> {
    let _guard = service.serialize().await;
    storage.ready()?;
    let storage = storage.storage.clone();
    let preference = aoidos_engine::blocking::run(move || {
        storage.with_database(|connection| {
            aoidos_locale::preference::get(connection).map_err(Into::into)
        })
    })
    .await
    .map_err(CmdError::from)?;
    Ok(result(&service, preference))
}

/// 持久化后应用窗口文案；原生更新失败保留已保存偏好并返回 pending。
///
/// # Errors
/// 非法版本为 app.bad-request，数据库错误按 store 域返回。
#[tauri::command]
pub async fn locale_set_preference(
    service: tauri::State<'_, LocaleService>,
    storage: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<LocaleResult, CmdError> {
    let SetLocale { preference } = crate::ipc::decode_request(&request, "语言参数不合法")?;
    if preference.version != 1 {
        return Err(CmdError::new(
            "app.bad-request",
            "语言偏好版本不受支持",
            None,
        ));
    }
    let _guard = service.serialize().await;
    storage.ready()?;
    let storage = storage.storage.clone();
    let preference = aoidos_engine::blocking::run(move || {
        storage.with_database(|connection| {
            aoidos_locale::preference::set(connection, preference).map_err(Into::into)
        })
    })
    .await
    .map_err(CmdError::from)?;
    Ok(result(&service, preference))
}

fn result(service: &LocaleService, preference: LocalePreference) -> LocaleResult {
    let locale = preference.locale.resolve();
    let native_status = service.native.apply(locale);
    service.confirm(locale);
    LocaleResult {
        preference,
        resolved_locale: locale,
        native_status,
    }
}

pub use native::{apply_native, handle_menu_event};

/// 所有表面均成功才确认 applied；单项失败不能短路其他表面的更新。
fn native_status(updated: [bool; 7]) -> NativeStatus {
    if updated.into_iter().all(|updated| updated) {
        NativeStatus::Applied
    } else {
        NativeStatus::Pending
    }
}

pub fn native_message(locale: Locale, key: &str) -> String {
    static MESSAGES: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    let messages = MESSAGES.get_or_init(|| {
        serde_json::from_str(NATIVE_LOCALES_JSON)
            .expect("build.rs validates embedded native messages")
    });
    messages
        .get(locale.as_str())
        .and_then(|messages| messages.get(key))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Aoidos")
        .to_owned()
}

pub fn native_prompt_text(locale: Locale) -> aoidos_llm::platform::NativePromptText {
    aoidos_llm::platform::NativePromptText {
        title: native_message(locale, "credentialTitle"),
        message: native_message(locale, "credentialPrompt"),
        save: native_message(locale, "credentialSave"),
        cancel: native_message(locale, "credentialCancel"),
    }
}

#[cfg(test)]
mod tests;
