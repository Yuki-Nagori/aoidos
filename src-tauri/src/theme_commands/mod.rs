//! 主题命令只借用共享存储服务；皮肤读取限定在随包剧本资源根。

use crate::{ipc::CmdError, store_commands::StorageService};
use aoidos_theme::{
    catalog::ThemeInfo,
    preference::ThemePreference,
    skin::{self, Skin},
};
use serde::Deserialize;
use std::path::PathBuf;

pub struct ThemeService {
    resource_root: PathBuf,
}

impl ThemeService {
    pub fn new(resource_root: PathBuf) -> Self {
        Self { resource_root }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetTheme {
    theme: String,
}

#[tauri::command]
pub fn theme_list() -> Result<Vec<ThemeInfo>, CmdError> {
    themes_result(aoidos_theme::catalog::themes())
}

fn themes_result(themes: Option<Vec<ThemeInfo>>) -> Result<Vec<ThemeInfo>, CmdError> {
    themes.ok_or_else(|| CmdError::new("app.not-ready", "主题目录不可用", None))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LoadSkin {
    script_id: String,
}

fn decode<T: serde::de::DeserializeOwned>(
    request: &tauri::ipc::Request<'_>,
) -> Result<T, CmdError> {
    crate::ipc::decode_request(request, "主题参数不合法")
}

#[tauri::command]
pub async fn theme_get_preference(
    state: tauri::State<'_, StorageService>,
) -> Result<ThemePreference, CmdError> {
    state.ready()?;
    let storage = state.storage.clone();
    aoidos_engine::blocking::run(move || {
        storage.with_database(|connection| {
            aoidos_theme::preference::get(connection).map_err(Into::into)
        })
    })
    .await
    .map_err(CmdError::from)
}

#[tauri::command]
pub async fn theme_set_preference(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<ThemePreference, CmdError> {
    let SetTheme { theme } = decode(&request)?;
    if !aoidos_theme::catalog::themes()
        .is_some_and(|themes| themes.iter().any(|item| item.id == theme))
    {
        return Err(CmdError::new("app.bad-request", "主题未登记", None));
    }
    state.ready()?;
    let storage = state.storage.clone();
    aoidos_engine::blocking::run(move || {
        storage.with_database(|connection| {
            aoidos_theme::preference::set(connection, theme).map_err(Into::into)
        })
    })
    .await
    .map_err(CmdError::from)
}

#[tauri::command]
pub fn theme_skin_load(
    state: tauri::State<'_, ThemeService>,
    request: tauri::ipc::Request<'_>,
) -> Result<Skin, CmdError> {
    let LoadSkin { script_id } = decode(&request)?;
    skin::load(&state.resource_root, &script_id).map_err(map_skin_error)
}

fn map_skin_error(error: skin::SkinError) -> CmdError {
    match error {
        skin::SkinError::UnknownScript => CmdError::new("app.not-found", "剧本不存在", None),
        skin::SkinError::InvalidSkin => {
            CmdError::new("theme.invalid-skin", "剧本皮肤格式不受支持", None)
        }
        skin::SkinError::Storage => CmdError::new("store.io", "剧本皮肤读取失败", None),
    }
}

#[cfg(test)]
mod tests;
