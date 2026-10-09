//! 存储运行服务及薄记录命令；SQL / 格式 / 恢复均由 engine 和 store 所有。

mod events;
use crate::ipc::CmdError;
use aoidos_engine::{
    fault::Fault,
    preferences::UiPreferences,
    record::{
        facts::DiceMode,
        view::{BodyPage, Page, View},
    },
    storage::Storage,
};
pub use events::StorageEvents;
use std::{path::Path, sync::Arc};

pub struct StorageService {
    pub storage: Arc<Storage>,
    pub events: Arc<StorageEvents>,
}
impl StorageService {
    /// 实例锁由 setup 唯一持有；初始化失败仍保留可查询的迁移诊断。
    pub fn new(root: &Path, events: Arc<StorageEvents>) -> Self {
        Self {
            storage: Arc::new(Storage::new(root, events.clone(), events.clone())),
            events,
        }
    }
    pub fn ready(&self) -> Result<(), CmdError> {
        self.storage.ready().map_err(CmdError::from)
    }
}
/// 只读迁移诊断；业务库迁移失败仍可查询，不启动新迁移。
/// # Errors
/// 服务缺失由壳初始化处理，快照包含真实 SQL 成败与事件基线。
pub fn store_get_migration(
    state: tauri::State<'_, StorageService>,
) -> Result<aoidos_engine::migration::Snapshot, CmdError> {
    Ok(state.storage.migration.snapshot())
}
/// # Errors
/// 存储未开放为 not-ready，其余对应存储错误。
pub async fn store_get_ui_preferences(
    state: tauri::State<'_, StorageService>,
) -> Result<UiPreferences, CmdError> {
    state.ready()?;
    let preferences = state.storage.preferences.clone();
    storage_job(move || preferences.get()).await
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetPreferences {
    panel_pinned: bool,
    dice_mode: DiceMode,
}
fn decode<T: serde::de::DeserializeOwned>(
    request: &tauri::ipc::Request<'_>,
) -> Result<T, CmdError> {
    crate::ipc::decode_request(request, "存储参数不合法")
}
/// 解码完整请求体，拒绝额外可写字段；保存成功才确认偏好。
/// # Errors
/// 非法参数为 bad-request，存储未开放 / 保存失败按契约返回。
pub async fn store_set_ui_preferences(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<UiPreferences, CmdError> {
    let params: SetPreferences = decode(&request)?;
    state.ready()?;
    let preferences = state.storage.preferences.clone();
    storage_job(move || preferences.set(params.panel_pinned, params.dice_mode)).await
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PageArgs {
    session_id: String,
    cursor: Option<String>,
    limit: Option<u32>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ViewArgs {
    session_id: String,
    limit: Option<u32>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BodyArgs {
    session_id: String,
    body_ref: String,
    cursor: Option<String>,
}
/// # Errors
/// 未知会话、非法分页或损坏记录按公共契约返回。
pub async fn engine_get_record_page(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<Page, CmdError> {
    let PageArgs {
        session_id,
        cursor,
        limit,
    } = decode(&request)?;
    state.ready()?;
    let records = state.storage.records.clone();
    storage_job(move || {
        let session = records.get(&session_id)?;
        session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .page(cursor.as_deref(), limit)
    })
    .await
}
/// # Errors
/// 同记录 page；不会启动新生成或迁移。
pub async fn engine_get_record_view(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<View, CmdError> {
    let ViewArgs { session_id, limit } = decode(&request)?;
    state.ready()?;
    let records = state.storage.records.clone();
    storage_job(move || {
        let session = records.get(&session_id)?;
        session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .view(limit)
    })
    .await
}
/// # Errors
/// 过期正文引用为 bad-request；有效不存在身份为 not-found。
pub async fn engine_get_record_body(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<BodyPage, CmdError> {
    let BodyArgs {
        session_id,
        body_ref,
        cursor,
    } = decode(&request)?;
    state.ready()?;
    let records = state.storage.records.clone();
    storage_job(move || {
        let session = records.get(&session_id)?;
        session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .body(&body_ref, cursor.as_deref())
    })
    .await
}

async fn storage_job<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, Fault> + Send + 'static,
) -> Result<T, CmdError> {
    aoidos_engine::blocking::run(job)
        .await
        .map_err(CmdError::from)
}

#[cfg(test)]
mod tests;
