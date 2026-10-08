//! 可信场景及固定结束 / 失败注入；请求沿用真实模型能力，传输始终替换为本地 Provider。

use mythos_engine::{
    fault::Fault,
    game::{catalog::*, domain::*, execution::Context},
    record::{
        facts::*,
        format::{self, Header},
        history::HistoryPort,
        projection::{Budget, Shape},
        session::Session,
        world::WorldPort,
    },
    request::PreparedGeneration,
    storage::Storage,
    turn::Coordinator,
};
use mythos_llm::{
    config::ProfileStore,
    guard::GuardSpec,
    provider::{Provider, ProviderInput, RequestMode},
    schedule::{AllowAll, GenerationRequest, RunPolicy},
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub struct Factory {
    pub dir: PathBuf,
    pub storage: Arc<Storage>,
}
impl RoundFactory for Factory {
    fn freeze(&self) -> Result<FrozenRound, Fault> {
        let profile = ProfileStore::new(&self.dir)
            .load()
            .map_err(Fault::from)?
            .into_iter()
            .find(|profile| profile.profile_id == "ipc-fixture")
            .ok_or_else(|| Fault::new("app.not-found", "夹具配置不存在"))?;
        let frozen = mythos_llm::config::freeze(
            &profile,
            &mythos_llm::credentials::CredentialFile::new(&self.dir),
        )
        .map_err(Fault::from)?;
        let provider = mythos_llm::providers::build_provider(
            frozen,
            &mythos_llm::proxy::SystemProxySnapshot::default(),
            None,
        )
        .map_err(build_error)?;
        let capabilities = provider.capabilities(&profile.model, RequestMode::Completion);
        let budget = Budget::for_model(&capabilities, profile.sampling.max_tokens)
            .map_err(projection_error)?;
        let mode = self.storage.preferences.get()?.dice_mode;
        Ok(FrozenRound {
            profile_id: profile.profile_id.clone(),
            profile_revision: "fixture-v1".into(),
            dice_mode: mode,
            generation: Arc::new(LocalGeneration {
                provider: Arc::new(mythos_engine::fixture::FixtureProvider(provider)),
                profile,
                budget,
            }),
        })
    }
}
fn build_error(_: mythos_llm::providers::ProviderBuildError) -> Fault {
    Fault::new("app.not-ready", "本地夹具请求构造失败")
}
fn projection_error(error: mythos_engine::record::projection::ProjectionError) -> Fault {
    error.fault()
}
struct LocalGeneration {
    provider: Arc<dyn Provider>,
    profile: mythos_llm::config::LlmProfile,
    budget: Budget,
}
impl Generation for LocalGeneration {
    fn shape(&self) -> Shape {
        Shape::Completion
    }
    fn projection_budget(&self) -> Budget {
        self.budget.clone()
    }
    fn prepare(
        &self,
        input: ProviderInput,
        guard: GuardSpec,
        _: bool,
    ) -> Result<PreparedGeneration, Fault> {
        let capabilities = self
            .provider
            .capabilities(&self.profile.model, RequestMode::Completion);
        PreparedGeneration::new(
            self.provider.clone(),
            GenerationRequest {
                provider_request: mythos_llm::provider::ProviderRequest {
                    model: self.profile.model.clone(),
                    input,
                    sampling: self.profile.sampling.clone(),
                    stops: vec![],
                },
                guard,
            },
            RunPolicy::default_for(self.profile.sampling.temperature, &capabilities),
            Arc::new(AllowAll),
        )
    }
}
pub fn header() -> Header {
    let prefix = "[MYTHOS:STATIC]\n只推进已知场景。\n[/MYTHOS:STATIC]\n".to_owned();
    Header {
        kind: "header".into(),
        format_version: 1,
        grammar_version: 1,
        projection_version: 1,
        script_id: "demo".into(),
        session_id: uuid::Uuid::new_v4().to_string(),
        created_at: format::now(),
        static_prefix_hash: format::hash(prefix.as_bytes()),
        static_prefix: prefix,
        script_revision: format::hash(b"game-fixture"),
    }
}
struct LocalDomain {
    catalog: SceneCatalog,
    storage: Arc<Storage>,
    scenario: Option<String>,
}
impl WorldPort for LocalDomain {
    fn registered(&self, kind: &str, version: u32) -> bool {
        kind == "sessionEnded" && version == 1
    }
    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault> {
        self.storage.with_database(|connection| {
            mythos_store::applied::contains(connection, id, hash).map_err(Fault::from)
        })
    }
    fn apply(&mut self, mutation: &WorldMutation, hash: &str) -> Result<(), Fault> {
        self.storage.with_database(|connection| {
            mythos_store::applied::apply_once(connection, &mutation.mutation_id, hash, &mut |_| {
                Ok(())
            })
            .map(|_| ())
            .map_err(Fault::from)
        })
    }
}
impl HistoryPort for LocalDomain {
    fn verify_target(&self, session: &Session, target: u64) -> Result<(), Fault> {
        mythos_engine::game::control::verify_boundary(session, target)
    }
    fn verify_replay_target(
        &self,
        session: &Session,
        parent: u64,
        target: u64,
    ) -> Result<(), Fault> {
        mythos_engine::game::control::verify_replay_boundary(session, parent, target)
    }
    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault> {
        WorldPort::applied(self, id, hash)
    }
    fn rebuild(&mut self, _: &Session, _: &[u64], id: &str, hash: &str) -> Result<(), Fault> {
        // 本夹具无领域变更，重建只确认原子控制标记；真实世界仍须登记自己的解释器。
        self.storage.with_database(|connection| {
            mythos_store::applied::apply_once(connection, id, hash, &mut |_| Ok(()))
                .map(|_| ())
                .map_err(Fault::from)
        })
    }
}
impl Domain for LocalDomain {
    fn catalog(&self) -> &SceneCatalog {
        &self.catalog
    }
    fn baseline_scene(&self) -> Option<ScenePosition> {
        Some(
            self.catalog
                .scene("room")
                .expect("registered scene")
                .position
                .clone(),
        )
    }
    fn confirmed(&self, session: &Session) -> Result<ConfirmedWorldView, Fault> {
        self.storage.ready()?;
        ConfirmedWorldView::capture(session, "0", BTreeMap::new())
    }
    fn context(&self, _: &ConfirmedWorldView) -> Result<String, Fault> {
        Ok("房间已确认，无额外世界属性。".into())
    }
    fn check_plan(
        &self,
        rule: &str,
        actor: &str,
        view: &ConfirmedWorldView,
    ) -> Result<CheckPlan, Fault> {
        if rule != "search" || actor != "player" {
            return Err(Fault::new("engine.invalid-phase", "夹具规则未登记"));
        }
        mythos_engine::game::dice::freeze_pbta(mythos_engine::game::dice::PlanSpec {
            rule_id: rule,
            actor_id: actor,
            modifiers: vec![],
            branches: ["success".into(), "costly".into(), "failure".into()],
            world_revision: view.world_revision(),
        })
    }
    fn consequences(
        &self,
        _: &Session,
        _: &mythos_engine::game::recovery::RoundFacts,
        _: &ConfirmedWorldView,
    ) -> Result<Vec<WorldMutation>, Fault> {
        if self.scenario.as_deref() == Some("failure") {
            return Err(Fault::new("engine.invalid-phase", "夹具结算失败"));
        }
        Ok(vec![])
    }
    fn exit_reason(&self, _: &str, _: &ConfirmedWorldView) -> Result<Option<String>, Fault> {
        Ok((self.scenario.as_deref() == Some("exit")).then(|| "夹具场景结束".into()))
    }
}
struct Completed;
impl CompletedRounds for Completed {
    fn completed(&self, _: CompletedRound) {}
}

pub async fn context(
    storage: Arc<Storage>,
    session: Arc<std::sync::Mutex<Session>>,
    coordinator: Coordinator,
    events: Arc<dyn mythos_engine::game::state::PhaseEvents>,
    scenario: Option<String>,
) -> Result<Arc<Context>, Fault> {
    let script_revision = session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .header()
        .script_revision
        .clone();
    let catalog = SceneCatalog::new(
        "fixture-v1".into(),
        script_revision,
        BTreeMap::from([(
            "room".into(),
            Node {
                kind: "scene".into(),
                title: "房间".into(),
                parent: None,
            },
        )]),
        BTreeMap::from([(
            "room".into(),
            Scene {
                position: ScenePosition {
                    scene_id: "room".into(),
                    path: vec![SceneNode {
                        kind: "scene".into(),
                        id: "room".into(),
                        title: "房间".into(),
                    }],
                },
                rules: SceneRuleView {
                    scene_id: "room".into(),
                    catalog_revision: "fixture-v1".into(),
                    advance_rule_ids: vec![],
                    progress_rule_ids: vec![],
                    hint_rules: vec![],
                    exit_rule_ids: vec![],
                },
                advance: vec![],
                check_rule_ids: vec!["search".into()],
                actor_ids: vec!["player".into()],
                forced_check: Some("search".into()),
                dice_disabled: false,
            },
        )]),
        BTreeMap::new(),
    )?;
    Context::open(
        session,
        Box::new(LocalDomain {
            catalog,
            storage,
            scenario,
        }),
        "player".into(),
        "player".into(),
        coordinator,
        events,
        Arc::new(Completed),
    )
    .await
}
