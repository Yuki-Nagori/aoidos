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
    config::{FrozenProfile, LlmProfile},
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
    /// 验证配置结构但不读取密钥；入口会在真正生成回合时再冻结凭据。
    ///
    /// 默认实现保留旧 adapter 语义；使用独立 profile 存储的应用层应覆盖此方法。
    /// # Errors
    /// 未知配置或结构非法时返回稳定 Fault。
    fn validate_profile(&self, id: &str) -> Result<(), Fault> {
        let resolved = self.freeze(id)?;
        if resolved.frozen.profile.profile_id != id {
            return Err(Fault::bad_request());
        }
        Ok(())
    }

    /// 读取不含凭据的公开 profile 快照；应用持久化 adapter 应避免访问密钥库。
    ///
    /// # Errors
    /// 配置不存在或结构无效时返回稳定 Fault。
    fn public_profile(&self, id: &str) -> Result<LlmProfile, Fault> {
        let resolved = self.freeze(id)?;
        if resolved.frozen.profile.profile_id != id {
            return Err(Fault::bad_request());
        }
        Ok(resolved.frozen.profile)
    }

    /// # Errors
    /// 未知配置、存储 / 凭据 / 代理读取失败按稳定错误返回。
    fn freeze(&self, id: &str) -> Result<ResolvedProfile, Fault>;

    /// 使用周目冻结的公开配置读取当前凭据；不得以同 ID 的新配置替代快照。
    /// # Errors
    /// adapter 无法按快照解析凭据、配置不合法或凭据读取失败时拒绝请求。
    fn freeze_snapshot(&self, profile: &LlmProfile) -> Result<ResolvedProfile, Fault> {
        let resolved = self.freeze(&profile.profile_id)?;
        if resolved.frozen.profile != *profile {
            return Err(Fault::new("app.conflict", "周目冻结的模型配置无法恢复"));
        }
        Ok(resolved)
    }
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
    durable_billing: bool,
}
impl ProfileFactory {
    pub fn new(
        source: Arc<dyn ProfileSource>,
        storage: Arc<Storage>,
        snapshot: SystemProxySnapshot,
        diagnostics: Arc<Mutex<Calibration>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            selected: Mutex::new(None),
            source,
            snapshot,
            storage,
            builder: Arc::new(production_provider),
            diagnostics,
            budget: Arc::new(AllowAll),
            durable_billing: true,
        })
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
            durable_billing: false,
        })
    }
    /// 检查 profile 可用于新周目；由应用 adapter 决定是否需要凭据。
    /// # Errors
    /// 配置不存在或结构无效时返回稳定 Fault。
    pub fn validate_profile(&self, id: &str) -> Result<(), Fault> {
        aoidos_llm::config::validate_identifier("profileId", id).map_err(invalid_profile)?;
        self.source.validate_profile(id)
    }
    fn for_profile(
        &self,
        id: &str,
        provider_session_id: Option<&str>,
        billing_run_id: Option<&str>,
        legacy_billing_run_id: Option<&str>,
    ) -> Result<FrozenRound, Fault> {
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
        let budget: Arc<dyn BudgetPort> = match (self.durable_billing, billing_run_id) {
            (true, Some(run_id)) => {
                let route = billing_route_policy(&profile.provider_id);
                let capabilities = provider.capabilities(
                    &profile.model,
                    aoidos_llm::provider::RequestMode::from(profile.mode),
                );
                let max_input_tokens = u64::from(capabilities.context_limit.unwrap_or(0));
                Arc::new(
                    crate::billing::LedgerBudgetPort::new(
                        self.storage.clone(),
                        run_id.to_owned(),
                        profile.profile_id.clone(),
                        profile.provider_id.clone(),
                        profile.model.clone(),
                        route.to_owned(),
                        max_input_tokens,
                    )
                    .with_legacy_scope(legacy_billing_run_id.map(str::to_owned)),
                )
            }
            _ => self.budget.clone(),
        };
        ProfileGeneration::with_provider_for_session(
            profile,
            provider,
            dice,
            budget,
            self.diagnostics.clone(),
            provider_session_id.map(str::to_owned),
        )
    }

    fn for_snapshot(
        &self,
        profile: &LlmProfile,
        provider_session_id: &str,
        billing_run_id: Option<&str>,
        legacy_billing_run_id: Option<&str>,
    ) -> Result<FrozenRound, Fault> {
        profile.validate().map_err(invalid_profile)?;
        self.storage.ready()?;
        let resolved = self.source.freeze_snapshot(profile)?;
        if resolved.frozen.profile != *profile {
            return Err(Fault::new("app.conflict", "周目冻结的模型配置无法恢复"));
        }
        let provider = (self.builder)(
            resolved.frozen,
            &self.snapshot,
            resolved.proxy_auth.as_ref(),
        )?;
        let dice = self.storage.preferences.get()?.dice_mode;
        let budget: Arc<dyn BudgetPort> = match (self.durable_billing, billing_run_id) {
            (true, Some(run_id)) => {
                let capabilities = provider.capabilities(
                    &profile.model,
                    aoidos_llm::provider::RequestMode::from(profile.mode),
                );
                Arc::new(
                    crate::billing::LedgerBudgetPort::new(
                        self.storage.clone(),
                        run_id.to_owned(),
                        profile.profile_id.clone(),
                        profile.provider_id.clone(),
                        profile.model.clone(),
                        billing_route_policy(&profile.provider_id).to_owned(),
                        u64::from(capabilities.context_limit.unwrap_or(0)),
                    )
                    .with_legacy_scope(legacy_billing_run_id.map(str::to_owned)),
                )
            }
            _ => self.budget.clone(),
        };
        ProfileGeneration::with_provider_for_session(
            profile.clone(),
            provider,
            dice,
            budget,
            self.diagnostics.clone(),
            Some(provider_session_id.to_owned()),
        )
    }

    /// 返回可持久化的公开配置快照，不向目录写入任何凭据值。
    /// # Errors
    /// 配置缺失或结构无效时返回稳定 Fault。
    pub fn public_profile(&self, id: &str) -> Result<LlmProfile, Fault> {
        aoidos_llm::config::validate_identifier("profileId", id).map_err(invalid_profile)?;
        let profile = self.source.public_profile(id)?;
        if profile.profile_id != id {
            return Err(Fault::bad_request());
        }
        profile.validate().map_err(invalid_profile)?;
        Ok(profile)
    }

    /// 按已冻结周目 profile 与 canonical session 账务身份准备私有请求。
    /// 输入构造器接收同一份冻结 profile 的请求模式，不需要二次读取可能已变化的设置。
    /// # Errors
    /// profile、供应商、预算、模式或输入不合法时在发送前拒绝。
    pub fn prepare_private_for_session(
        &self,
        profile_id: &str,
        session_id: &str,
        legacy_billing_run_id: Option<&str>,
        build_input: impl FnOnce(
            aoidos_llm::provider::RequestMode,
        ) -> Result<
            (
                aoidos_llm::provider::ProviderInput,
                aoidos_llm::guard::GuardSpec,
            ),
            Fault,
        >,
    ) -> Result<crate::request::PreparedGeneration, Fault> {
        let frozen = if self.durable_billing {
            let target = self.storage.current_run_open_target(session_id)?;
            if target.profile.profile_id != profile_id {
                return Err(Fault::new("app.conflict", "周目的模型配置身份不一致"));
            }
            if legacy_billing_run_id.is_some_and(|id| id != target.memory_run_id) {
                return Err(Fault::new("app.conflict", "周目的记忆账务身份不一致"));
            }
            let legacy_scope =
                (target.memory_run_id != session_id).then_some(target.memory_run_id.as_str());
            self.for_snapshot(&target.profile, session_id, Some(session_id), legacy_scope)?
        } else {
            self.for_profile(profile_id, Some(session_id), None, legacy_billing_run_id)?
        };
        let mode = frozen.generation.request_mode();
        let (input, guard) = build_input(mode)?;
        frozen.generation.prepare(input, guard, false)
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
        if self.durable_billing {
            return Err(Fault::new(
                "budget.run-limit-missing",
                "缺少本局费用预算身份",
            ));
        }
        let id = self
            .selected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(no_selection)?;
        self.for_profile(&id, None, None, None)
    }
    fn freeze_for_session(&self, session_id: &str) -> Result<FrozenRound, Fault> {
        if !self.durable_billing {
            let id = self
                .selected
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .ok_or_else(no_selection)?;
            return self.for_profile(&id, Some(session_id), None, None);
        }
        let target = self.storage.current_run_open_target(session_id)?;
        let legacy_scope =
            (target.memory_run_id != session_id).then_some(target.memory_run_id.as_str());
        self.for_snapshot(&target.profile, session_id, Some(session_id), legacy_scope)
    }
}
/// 返回计费账本用于匹配当前供应商价格的路由身份。
pub fn billing_route_policy(provider_id: &str) -> &str {
    if provider_id == "openrouter" {
        "openrouter"
    } else {
        "direct"
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

#[derive(Debug, serde::Deserialize, serde::Serialize, Clone, PartialEq, Eq)]
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
    pub memory_run_id: String,
    pub profile_id: String,
    pub script_id: String,
    pub title: String,
    /// SQLite 已提交活动周目，但兼容文件未能写入时为 true；后续读取会重试修复。
    pub selection_mirror_pending: bool,
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
    /// 返回磁盘登记的当前会话及 profile；记忆请求使用它来继承游戏实际选择的模型。
    /// # Errors
    /// 当前会话缺失或元数据损坏时拒绝，不接受调用方指定其他 profile。
    pub fn active_profile(&self) -> Result<(String, String), Fault> {
        let selection =
            read_authoritative_selection(&self.storage, &self.root)?.ok_or_else(no_selection)?;
        Ok((selection.session_id, selection.profile_id))
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
        let storage = self.storage.clone();
        let (previous, selection_mirror_pending) =
            crate::blocking::run(move || read_authoritative_selection_with_status(&storage, &root))
                .await?;
        if !start_new
            && let Some(previous) = &previous
            && previous.script_id == script_id
            && previous.profile_id != profile_id
        {
            return Err(Fault::new(
                "app.conflict",
                "周目的模型配置已冻结；更换配置需要新建周目",
            ));
        }
        let mut frozen_profile = None;
        if !start_new
            && let Some(previous) = &previous
            && previous.script_id == script_id
        {
            let storage = self.storage.clone();
            let session_id = previous.session_id.clone();
            match crate::blocking::run(move || storage.current_run_open_target(&session_id)).await {
                Ok(target) => {
                    if target.profile.profile_id != profile_id {
                        return Err(Fault::new(
                            "app.conflict",
                            "周目的模型配置已冻结；更换配置需要新建周目",
                        ));
                    }
                    frozen_profile = Some(target.profile);
                }
                Err(error) if error.code == "app.not-found" => {
                    let factory = self.factory.clone();
                    let candidate_id = profile_id.clone();
                    let profile =
                        crate::blocking::run(move || factory.public_profile(&candidate_id)).await?;
                    let storage = self.storage.clone();
                    let session_id = previous.session_id.clone();
                    let candidate = profile.clone();
                    if crate::blocking::run(move || {
                        storage.run_profile_matches(&session_id, &candidate)
                    })
                    .await?
                        == Some(false)
                    {
                        return Err(Fault::new(
                            "app.conflict",
                            "周目的模型配置已冻结；更换配置内容需要新建周目",
                        ));
                    }
                    frozen_profile = Some(profile);
                }
                Err(error) => return Err(error),
            }
        }
        if !start_new
            && let Some(previous) = &previous
            && previous.profile_id == profile_id
            && previous.script_id == script_id
        {
            match service.get_phase(&previous.session_id) {
                Ok(_) => {
                    let run = self.storage.memory_run_for_session(&previous.session_id)?;
                    let mut result = opened(previous, &run.run_id)?;
                    result.selection_mirror_pending = selection_mirror_pending;
                    return Ok(result);
                }
                Err(error) if error.code == "app.not-found" => {}
                Err(error) => return Err(error),
            }
        }
        let _lease = self.coordinator.acquire()?;
        let preferences = self.storage.preferences.clone();
        crate::blocking::run(move || preferences.get()).await?;
        if start_new {
            let factory = self.factory.clone();
            let check_id = profile_id.clone();
            crate::blocking::run(move || factory.validate_profile(&check_id)).await?;
        }
        let profile_snapshot = if let Some(profile) = frozen_profile {
            profile
        } else {
            let factory = self.factory.clone();
            let check_id = profile_id.clone();
            crate::blocking::run(move || factory.public_profile(&check_id)).await?
        };
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
        let (session, created_new) = crate::blocking::run(move || match old {
            Some(selection) if !start_new && selection.script_id == selected_script => records
                .open(&selection.script_id, &selection.session_id)
                .map(|session| (session, false)),
            _ => records
                .create(builtin::header(&selected_script)?)
                .map(|session| (session, true)),
        })
        .await?;
        let header = session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .header()
            .clone();
        let id = header.session_id.clone();
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
        let memory_run = match self.storage.memory_run_for_session(&id) {
            Ok(run) => run,
            Err(error) => {
                self.close_record(&id).await;
                return Err(error);
            }
        };
        if let Err(error) = service.register(context) {
            self.close_record(&id).await;
            return Err(error);
        }
        if created_new {
            let registration = crate::run_catalog::NewRun {
                session_id: id.clone(),
                script_id: header.script_id,
                created_at: header.created_at,
                memory_run_id: memory_run.run_id.clone(),
                profile: profile_snapshot,
            };
            let storage = self.storage.clone();
            if let Err(error) =
                crate::blocking::run(move || storage.register_new_run(&registration)).await
            {
                let _ = service.close_session(&id).await;
                self.close_record(&id).await;
                return Err(error);
            }
        }
        let selection = Selection {
            version: 1,
            session_id: id.clone(),
            profile_id,
            script_id,
        };
        let selected = selection.clone();
        let root = self.root.clone();
        let storage = self.storage.clone();
        let active = match crate::blocking::run(move || storage.active_run_selection()).await {
            Ok(active) => active,
            Err(error) => {
                let _ = service.close_session(&id).await;
                self.close_record(&id).await;
                return Err(error);
            }
        };
        let has_authoritative_selection = active.as_ref().is_some_and(|active| {
            active.session_id == selection.session_id
                && active.profile_id == selection.profile_id
                && active.script_id == selection.script_id
        });
        let selection_mirror_pending = if has_authoritative_selection {
            crate::blocking::run(move || write_selection(&root, &selected))
                .await
                .is_err()
        } else {
            if let Err(error) =
                crate::blocking::run(move || write_selection(&root, &selected)).await
            {
                let _ = service.close_session(&id).await;
                self.close_record(&id).await;
                return Err(error);
            }
            false
        };
        self.factory.select(selection.profile_id.clone());
        let mut result = opened(&selection, &memory_run.run_id)?;
        result.selection_mirror_pending = selection_mirror_pending;
        Ok(result)
    }

    /// 玩家明确选择并恢复目录中的既有周目；恢复不自动发送模型请求。
    /// # Errors
    /// revision / 身份 / 剧本不匹配、owner 关闭、记录恢复或 SQLite 选择提交失败时拒绝切换。
    pub async fn open_selected(
        &self,
        service: &Service,
        session_id: String,
        expected_revision: u64,
    ) -> Result<OpenedSession, Fault> {
        if !crate::record::format::valid_uuid(&session_id) || expected_revision == 0 {
            return Err(Fault::bad_request());
        }
        let _opening = self.opening.try_lock().map_err(open_busy)?;
        self.storage.ready()?;
        let storage = self.storage.clone();
        let requested_id = session_id.clone();
        let target =
            crate::blocking::run(move || storage.run_open_target(&requested_id, expected_revision))
                .await?;
        let scenario = super::assets::scenario(&target.script_id)?;
        let _lease = self.coordinator.acquire()?;

        match service.get_phase(&session_id) {
            Ok(_) => {
                let updated_at = time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .map_err(|_| Fault::new("store.database", "读取周目切换时间失败"))?;
                let storage = self.storage.clone();
                let activate_target = target.clone();
                if let Err(error) = crate::blocking::run(move || {
                    storage.activate_run(&activate_target, &updated_at)
                })
                .await
                {
                    let storage = self.storage.clone();
                    if error.code != "store.commit-unknown" {
                        return Err(error);
                    }
                    match crate::blocking::run(move || storage.active_run_selection()).await {
                        Ok(Some(selected))
                            if activation_error_matches_selection(
                                &error,
                                &selected.session_id,
                                &target.session_id,
                            ) => {}
                        _ => return Err(error),
                    }
                }
                let selection = Selection {
                    version: 1,
                    session_id: session_id.clone(),
                    profile_id: target.profile.profile_id,
                    script_id: target.script_id,
                };
                let root = self.root.clone();
                let mirror = selection.clone();
                let selection_mirror_pending =
                    crate::blocking::run(move || write_selection(&root, &mirror))
                        .await
                        .is_err();
                self.factory.select(selection.profile_id.clone());
                let mut result = opened(&selection, &target.memory_run_id)?;
                result.selection_mirror_pending = selection_mirror_pending;
                return Ok(result);
            }
            Err(error) if error.code == "app.not-found" => {}
            Err(error) => return Err(error),
        }

        let previous = {
            let root = self.root.clone();
            let storage = self.storage.clone();
            crate::blocking::run(move || read_authoritative_selection(&storage, &root)).await?
        };
        let records = self.storage.records.clone();
        let open_script = target.script_id.clone();
        let open_id = target.session_id.clone();
        let session = crate::blocking::run(move || records.open(&open_script, &open_id)).await?;
        let header = session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .header()
            .clone();
        if header.session_id != target.session_id
            || header.script_id != target.script_id
            || header.created_at != target.created_at
        {
            self.close_record(&target.session_id).await;
            return Err(Fault::new("store.corrupt", "周目记录与目录身份不一致"));
        }
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
                self.close_record(&target.session_id).await;
                return Err(error);
            }
        };
        let memory_run = match self.storage.memory_run_for_session(&target.session_id) {
            Ok(memory_run) if memory_run.run_id == target.memory_run_id => memory_run,
            Ok(_) => {
                self.close_record(&target.session_id).await;
                return Err(Fault::new("app.conflict", "周目的记忆身份需要修复"));
            }
            Err(error) => {
                self.close_record(&target.session_id).await;
                return Err(error);
            }
        };
        let updated_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| Fault::new("store.database", "读取周目切换时间失败"))?;
        if let Err(error) = service.register(context) {
            self.close_record(&target.session_id).await;
            return Err(error);
        }
        if let Some(previous) = &previous
            && previous.session_id != session_id
        {
            match service.get_phase(&previous.session_id) {
                Ok(_) => {
                    if let Err(error) = service.close_session(&previous.session_id).await {
                        let _ = service.close_session(&target.session_id).await;
                        self.close_record(&target.session_id).await;
                        return Err(error);
                    }
                    let records = self.storage.records.clone();
                    let previous_id = previous.session_id.clone();
                    if let Err(error) =
                        crate::blocking::run(move || records.close(&previous_id)).await
                    {
                        let _ = service.close_session(&target.session_id).await;
                        self.close_record(&target.session_id).await;
                        return Err(error);
                    }
                }
                Err(error) if error.code == "app.not-found" => {}
                Err(error) => {
                    let _ = service.close_session(&target.session_id).await;
                    self.close_record(&target.session_id).await;
                    return Err(error);
                }
            }
        }
        let storage = self.storage.clone();
        let activate_target = target.clone();
        if let Err(error) =
            crate::blocking::run(move || storage.activate_run(&activate_target, &updated_at)).await
        {
            if error.code != "store.commit-unknown" {
                let _ = service.close_session(&target.session_id).await;
                self.close_record(&target.session_id).await;
                return Err(error);
            }
            let storage = self.storage.clone();
            let selected = crate::blocking::run(move || storage.active_run_selection()).await;
            match selected {
                Ok(Some(selection))
                    if activation_error_matches_selection(
                        &error,
                        &selection.session_id,
                        &target.session_id,
                    ) => {}
                Ok(_) => {
                    let _ = service.close_session(&target.session_id).await;
                    self.close_record(&target.session_id).await;
                    return Err(error);
                }
                Err(_) => {
                    // 提交结果和读回都不确定时保留目标 owner，避免 SQLite 指向已关闭记录。
                    return Err(error);
                }
            }
        }
        let selection = Selection {
            version: 1,
            session_id: target.session_id.clone(),
            profile_id: target.profile.profile_id.clone(),
            script_id: target.script_id,
        };
        let root = self.root.clone();
        let mirror = selection.clone();
        let selection_mirror_pending =
            crate::blocking::run(move || write_selection(&root, &mirror))
                .await
                .is_err();
        self.factory.select(selection.profile_id.clone());
        let mut result = opened(&selection, &memory_run.run_id)?;
        result.title = scenario.title;
        result.selection_mirror_pending = selection_mirror_pending;
        Ok(result)
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
fn activation_error_matches_selection(error: &Fault, actual: &str, expected: &str) -> bool {
    error.code == "store.commit-unknown" && actual == expected
}
fn opened(selection: &Selection, memory_run_id: &str) -> Result<OpenedSession, Fault> {
    Ok(OpenedSession {
        session_id: selection.session_id.clone(),
        memory_run_id: memory_run_id.into(),
        profile_id: selection.profile_id.clone(),
        script_id: selection.script_id.clone(),
        title: super::assets::scenario(&selection.script_id)?.title,
        selection_mirror_pending: false,
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

fn read_authoritative_selection(
    storage: &Storage,
    root: &Path,
) -> Result<Option<Selection>, Fault> {
    Ok(read_authoritative_selection_with_status(storage, root)?.0)
}

fn read_authoritative_selection_with_status(
    storage: &Storage,
    root: &Path,
) -> Result<(Option<Selection>, bool), Fault> {
    if let Some(selection) = storage.active_run_selection()? {
        let selection = Selection {
            version: 1,
            session_id: selection.session_id,
            profile_id: selection.profile_id,
            script_id: selection.script_id,
        };
        let mirror_pending = if read_selection(root).ok().flatten().as_ref() != Some(&selection) {
            // SQLite 是权威选择；镜像写入失败可在后续读取时重试，不阻断恢复。
            write_selection(root, &selection).is_err()
        } else {
            false
        };
        return Ok((Some(selection), mirror_pending));
    }
    Ok((read_selection(root)?, false))
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
