//! 最小正式入口：明确选择配置，持久登记一个活动会话；重开不启动生成。

use super::{
    builtin,
    domain::{FrozenRound, RoundFactory},
    generation::ProfileGeneration,
    runtime::Service,
    state::PhaseEvents,
};
use crate::{fault::Fault, record::calibration::Calibration, storage::Storage, turn::Coordinator};
use aoidos_llm::{
    config::FrozenProfile,
    provider::Provider,
    proxy::{ProxyAuth, SystemProxySnapshot},
    schedule::{AllowAll, BudgetPort},
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub struct ResolvedProfile {
    pub frozen: FrozenProfile,
    pub proxy_auth: Option<ProxyAuth>,
}
/// 保存配置 / 凭据的读取锁由实际所有者提供；不读取 Webview 的密钥或端点。
pub trait ProfileSource: Send + Sync {
    /// # Errors
    /// 未知配置、存储 / 凭据 / 代理读取失败按稳定错误返回。
    fn freeze(&self, id: &str) -> Result<ResolvedProfile, Fault>;
}
/// 可信 Rust 装配的 adapter 构造入口，测试只能在这里替换本地端点。
pub type ProviderBuilder = dyn Fn(FrozenProfile, &SystemProxySnapshot, Option<&ProxyAuth>) -> Result<Arc<dyn Provider>, Fault>
    + Send
    + Sync;
/// 全应用选择与回合冻结分离；运行中的 child 只保留已经冻结的副本。
pub struct ProfileFactory {
    selected: Mutex<Option<String>>,
    source: Arc<dyn ProfileSource>,
    storage: Arc<Storage>,
    snapshot: SystemProxySnapshot,
    builder: Arc<ProviderBuilder>,
    diagnostics: Arc<Mutex<Calibration>>,
    budget: Arc<dyn BudgetPort>,
}
impl ProfileFactory {
    pub fn new(
        source: Arc<dyn ProfileSource>,
        storage: Arc<Storage>,
        snapshot: SystemProxySnapshot,
        diagnostics: Arc<Mutex<Calibration>>,
    ) -> Arc<Self> {
        Self::with_builder(
            source,
            storage,
            snapshot,
            diagnostics,
            Arc::new(production_provider),
            Arc::new(AllowAll),
        )
    }
    /// 仅可信装配注入构造器及预算；配置 / 能力和投影校验不被绕开。
    pub fn with_builder(
        source: Arc<dyn ProfileSource>,
        storage: Arc<Storage>,
        snapshot: SystemProxySnapshot,
        diagnostics: Arc<Mutex<Calibration>>,
        builder: Arc<ProviderBuilder>,
        budget: Arc<dyn BudgetPort>,
    ) -> Arc<Self> {
        Arc::new(Self {
            selected: Mutex::new(None),
            source,
            storage,
            snapshot,
            builder,
            diagnostics,
            budget,
        })
    }
    fn for_profile(&self, id: &str) -> Result<FrozenRound, Fault> {
        aoidos_llm::config::validate_identifier("profileId", id).map_err(invalid_profile)?;
        self.storage.ready()?;
        let resolved = self.source.freeze(id)?;
        if resolved.frozen.profile.profile_id != id {
            return Err(Fault::bad_request());
        }
        let profile = resolved.frozen.profile.clone();
        let provider = (self.builder)(
            resolved.frozen,
            &self.snapshot,
            resolved.proxy_auth.as_ref(),
        )?;
        let dice = self.storage.preferences.get()?.dice_mode;
        ProfileGeneration::with_provider(
            profile,
            provider,
            dice,
            self.budget.clone(),
            self.diagnostics.clone(),
        )
    }
    fn select(&self, id: String) {
        *self
            .selected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id);
    }
}
impl RoundFactory for ProfileFactory {
    fn freeze(&self) -> Result<FrozenRound, Fault> {
        let id = self
            .selected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(no_selection)?;
        self.for_profile(&id)
    }
}
fn production_provider(
    frozen: FrozenProfile,
    snapshot: &SystemProxySnapshot,
    auth: Option<&ProxyAuth>,
) -> Result<Arc<dyn Provider>, Fault> {
    aoidos_llm::providers::build_provider(frozen, snapshot, auth).map_err(Fault::provider_build)
}
fn invalid_profile(_: aoidos_llm::config::ProfileError) -> Fault {
    Fault::bad_request()
}
fn no_selection() -> Fault {
    Fault::new("app.not-ready", "请先选择模型配置并打开剧本")
}

#[derive(serde::Deserialize, serde::Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Selection {
    version: u32,
    session_id: String,
    profile_id: String,
    script_id: String,
}
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedSession {
    pub session_id: String,
    pub profile_id: String,
    pub script_id: String,
    pub title: String,
}
pub struct Product {
    root: PathBuf,
    storage: Arc<Storage>,
    coordinator: Coordinator,
    events: Arc<dyn PhaseEvents>,
    pub factory: Arc<ProfileFactory>,
    opening: tokio::sync::Mutex<()>,
}
impl Product {
    /// 调用方用同一 factory / coordinator 建 Service，再交给 open；壳只持服务。
    pub fn new(
        root: &Path,
        storage: Arc<Storage>,
        coordinator: Coordinator,
        events: Arc<dyn PhaseEvents>,
        factory: Arc<ProfileFactory>,
    ) -> Self {
        Self {
            root: root.into(),
            storage,
            coordinator,
            events,
            factory,
            opening: tokio::sync::Mutex::new(()),
        }
    }
    /// 明确新建或重开已选择周目；预检不发 HTTP，重开保持暂停并等待显式恢复。
    /// # Errors
    /// 参数、门禁、配置、资源版本或磁盘失败拒绝；失败保留已存在存档。
    pub async fn open(
        &self,
        service: &Service,
        profile_id: String,
        script_id: String,
        start_new: bool,
    ) -> Result<OpenedSession, Fault> {
        aoidos_llm::config::validate_identifier("profileId", &profile_id)
            .map_err(invalid_profile)?;
        let _opening = self.opening.try_lock().map_err(open_busy)?;
        self.storage.ready()?;
        super::assets::scenario(&script_id)?;
        let root = self.root.clone();
        let previous = crate::blocking::run(move || read_selection(&root)).await?;
        if !start_new
            && let Some(previous) = &previous
            && previous.profile_id == profile_id
            && previous.script_id == script_id
        {
            match service.get_phase(&previous.session_id) {
                Ok(_) => return opened(previous),
                Err(error) if error.code == "app.not-found" => {}
                Err(error) => return Err(error),
            }
        }
        let _lease = self.coordinator.acquire()?;
        let factory = self.factory.clone();
        let check_id = profile_id.clone();
        crate::blocking::run(move || factory.for_profile(&check_id).map(|_| ())).await?;
        if let Some(previous) = &previous {
            match service.get_phase(&previous.session_id) {
                Ok(_) => {
                    service.close_session(&previous.session_id).await?;
                    let records = self.storage.records.clone();
                    let id = previous.session_id.clone();
                    crate::blocking::run(move || records.close(&id)).await?;
                }
                Err(error) if error.code == "app.not-found" => {}
                Err(error) => return Err(error),
            }
        }
        let records = self.storage.records.clone();
        let old = previous.clone();
        let selected_script = script_id.clone();
        let session = crate::blocking::run(move || match old {
            Some(selection) if !start_new && selection.script_id == selected_script => {
                records.open(&selection.script_id, &selection.session_id)
            }
            _ => records.create(builtin::header(&selected_script)?),
        })
        .await?;
        let id = session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .header()
            .session_id
            .clone();
        let context = match builtin::open(
            self.storage.clone(),
            session,
            self.coordinator.clone(),
            self.events.clone(),
        )
        .await
        {
            Ok(context) => context,
            Err(error) => {
                self.close_record(&id).await;
                return Err(error);
            }
        };
        if let Err(error) = service.register(context) {
            self.close_record(&id).await;
            return Err(error);
        }
        let selection = Selection {
            version: 1,
            session_id: id.clone(),
            profile_id,
            script_id,
        };
        let selected = selection.clone();
        let root = self.root.clone();
        if let Err(error) = crate::blocking::run(move || write_selection(&root, &selected)).await {
            let _ = service.close_session(&id).await;
            self.close_record(&id).await;
            return Err(error);
        }
        self.factory.select(selection.profile_id.clone());
        opened(&selection)
    }
    async fn close_record(&self, id: &str) {
        let records = self.storage.records.clone();
        let id = id.to_owned();
        let _ = crate::blocking::run(move || records.close(&id)).await;
    }
}
fn open_busy(_: tokio::sync::TryLockError) -> Fault {
    Fault::new("app.busy", "会话正在打开")
}
fn opened(selection: &Selection) -> Result<OpenedSession, Fault> {
    Ok(OpenedSession {
        session_id: selection.session_id.clone(),
        profile_id: selection.profile_id.clone(),
        script_id: selection.script_id.clone(),
        title: super::assets::scenario(&selection.script_id)?.title,
    })
}
fn read_selection(root: &Path) -> Result<Option<Selection>, Fault> {
    let Some(text) = aoidos_store::read::read_text_bounded(&root.join("active-session.json"), 4096)
        .map_err(Fault::from)?
    else {
        return Ok(None);
    };
    let selection: Selection = aoidos_json::decode(&text, 4096).map_err(corrupt_selection)?;
    if !aoidos_script::valid_id(&selection.script_id)
        || selection.version != 1
        || !crate::record::format::valid_uuid(&selection.session_id)
        || aoidos_llm::config::validate_identifier("profileId", &selection.profile_id).is_err()
    {
        return Err(corrupt_selection(aoidos_json::Error::InvalidShape));
    }
    Ok(Some(selection))
}
fn write_selection(root: &Path, selection: &Selection) -> Result<(), Fault> {
    let text = aoidos_json::to_string(selection).map_err(corrupt_selection)?;
    aoidos_store::atomic::write_text_atomic(&root.join("active-session.json"), &text)
        .map_err(Fault::from)
}
fn corrupt_selection(_: aoidos_json::Error) -> Fault {
    Fault::new("store.corrupt", "活动会话选择记录不可用")
}
#[cfg(test)]
mod tests;
