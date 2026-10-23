//! 已登记原文的可信最小章节适配；内容解析不自动赋予规则或世界写入权。

use super::{catalog::*, domain::*, execution::Context};
use crate::{
    fault::Fault,
    record::{
        facts::*,
        format::{self, Header},
        history::HistoryPort,
        session::Session,
        world::WorldPort,
    },
    storage::Storage,
    turn::Coordinator,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

fn prefix(id: &str) -> Result<String, Fault> {
    Ok(format!(
        "[AOIDOS:STATIC]\n{}\n[/AOIDOS:STATIC]\n",
        super::assets::scenario(id)?.body
    ))
}
/// 创建注册剧本的新周目，冻结原文及发布修订；不接受文件路径。
/// # Errors
/// 未登记身份或资源不合法拒绝，不修改旧记录。
pub fn header(id: &str) -> Result<Header, Fault> {
    let static_prefix = prefix(id)?;
    Ok(Header {
        kind: "header".into(),
        format_version: 1,
        grammar_version: 1,
        projection_version: 1,
        script_id: id.into(),
        session_id: uuid::Uuid::new_v4().to_string(),
        created_at: format::now(),
        static_prefix_hash: format::hash(static_prefix.as_bytes()),
        static_prefix,
        script_revision: super::assets::revision(id)?,
    })
}
struct BuiltinDomain {
    catalog: SceneCatalog,
    storage: Arc<Storage>,
    revision: String,
    entity_seed: EntityDirectory,
}
impl WorldPort for BuiltinDomain {
    fn validate(&self, session: &Session, mutations: &[WorldMutation]) -> Result<(), Fault> {
        if mutations
            .iter()
            .any(|mutation| match mutation.data.kind.as_str() {
                "sessionEnded"
                | "sceneAdvanced"
                | "scenePresenceChanged"
                | "entityCreated"
                | "observed"
                | "rumorDelivered" => mutation.data.version != 1,
                _ => true,
            })
        {
            return Err(unsupported());
        }
        let entity_mutations = mutations
            .iter()
            .filter(|mutation| mutation.data.kind == "entityCreated")
            .cloned()
            .collect::<Vec<_>>();
        let has_presence_or_evidence = mutations.iter().any(|mutation| {
            matches!(
                mutation.data.kind.as_str(),
                "scenePresenceChanged" | "observed" | "rumorDelivered"
            )
        });
        if entity_mutations.is_empty() && !has_presence_or_evidence {
            return Ok(());
        }
        self.storage.with_database(|connection| {
            if !entity_mutations.is_empty() {
                EntityRegistry.validate_group(
                    connection,
                    session,
                    &self.entity_seed,
                    &self.catalog,
                    &entity_mutations,
                )?;
            }
            if has_presence_or_evidence {
                let directory =
                    EntityRegistry.validation_snapshot(connection, session, &self.entity_seed)?;
                crate::game::presence::ScenePresence::validate_mutations(
                    connection,
                    session,
                    &self.catalog,
                    &directory,
                    mutations,
                )?;
            }
            Ok(())
        })
    }
    fn validate_recovery(
        &self,
        session: &Session,
        record_seq: u64,
        mutations: &[WorldMutation],
    ) -> Result<(), Fault> {
        let has_presence_or_evidence = mutations.iter().any(|mutation| {
            matches!(
                mutation.data.kind.as_str(),
                "scenePresenceChanged" | "observed" | "rumorDelivered"
            )
        });
        if !has_presence_or_evidence {
            return self.validate(session, mutations);
        }
        self.storage.with_database(|connection| {
            let directory =
                EntityRegistry.validation_snapshot(connection, session, &self.entity_seed)?;
            crate::game::presence::ScenePresence::validate_recovery_mutations(
                connection,
                session,
                &self.catalog,
                &directory,
                record_seq,
                mutations,
            )
        })
    }
    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault> {
        self.storage.with_database(|connection| {
            aoidos_store::applied::contains(connection, id, hash).map_err(Fault::from)
        })
    }
    fn apply(
        &mut self,
        session: &Session,
        record_seq: u64,
        mutation: &WorldMutation,
        hash: &str,
    ) -> Result<(), Fault> {
        if mutation.data.kind == "entityCreated" {
            return self.storage.with_database(|connection| {
                EntityRegistry.apply(
                    connection,
                    session,
                    &self.entity_seed,
                    &self.catalog,
                    EntityCreationCommit {
                        record_seq,
                        mutation,
                        content_hash: hash,
                    },
                )
            });
        }
        if mutation.data.kind == "sceneAdvanced"
            && let Fact::SceneAdvanced { position, .. } =
                Fact::decode(&mutation.data.kind, &mutation.data.payload).map_err(Fault::from)?
        {
            crate::game::presence::ScenePresence::validate_scene_advance(&self.catalog, &position)?;
        }
        if !matches!(
            mutation.data.kind.as_str(),
            "sessionEnded"
                | "sceneAdvanced"
                | "scenePresenceChanged"
                | "observed"
                | "rumorDelivered"
        ) || mutation.data.version != 1
        {
            return Err(unsupported());
        }
        self.storage.with_database(|connection| {
            aoidos_store::applied::apply_once(connection, &mutation.mutation_id, hash, &mut |_| {
                Ok(())
            })
            .map(|_| ())
            .map_err(Fault::from)
        })
    }
}
impl HistoryPort for BuiltinDomain {
    fn verify_target(&self, session: &Session, target: u64) -> Result<(), Fault> {
        super::control::verify_boundary(session, target)
    }
    fn verify_replay_target(
        &self,
        session: &Session,
        parent: u64,
        target: u64,
    ) -> Result<(), Fault> {
        super::control::verify_replay_boundary(session, parent, target)
    }
    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault> {
        WorldPort::applied(self, id, hash)
    }
    fn rebuild(&mut self, _: &Session, _: &[u64], id: &str, hash: &str) -> Result<(), Fault> {
        self.storage.with_database(|connection| {
            aoidos_store::applied::apply_once(connection, id, hash, &mut |_| Ok(()))
                .map(|_| ())
                .map_err(Fault::from)
        })
    }
}
impl Domain for BuiltinDomain {
    fn catalog(&self) -> &SceneCatalog {
        &self.catalog
    }
    fn baseline_scene(&self) -> Option<ScenePosition> {
        self.catalog
            .scene("entry")
            .ok()
            .map(|scene| scene.position.clone())
    }
    fn scene_presence(
        &self,
        session: &Session,
    ) -> Result<crate::game::presence::ScenePresenceSnapshot, Fault> {
        self.storage.with_database(|connection| {
            let directory = EntityRegistry.snapshot(connection, session, &self.entity_seed)?;
            crate::game::presence::ScenePresence::snapshot(
                connection,
                session,
                &self.catalog,
                &directory,
            )
        })
    }
    fn confirmed(&self, session: &Session) -> Result<ConfirmedWorldView, Fault> {
        self.storage.ready()?;
        let mut facts = BTreeMap::new();
        let path = session.path_at(session.history_revision())?;
        let mut accepted_rounds = std::collections::BTreeSet::new();
        let mut committed_round_count = 0_usize;
        let mut latest_evidence = None;
        for seq in path {
            let record = session.read(seq).map_err(Fault::from)?;
            let Some(record_body) = record.body.map(|record| record.body) else {
                continue;
            };
            let format::Body::System { code, data, .. } = record_body else {
                continue;
            };
            match code.as_str() {
                "roundAccepted" => {
                    if let Fact::RoundAccepted {
                        identity,
                        mode: format::InputMode::InCharacter,
                        ..
                    } = Fact::decode(&code, &data).map_err(Fault::from)?
                    {
                        accepted_rounds.insert(identity.round_id);
                    }
                }
                "roundSettled" => {
                    let Fact::RoundSettled { identity, .. } =
                        Fact::decode(&code, &data).map_err(Fault::from)?
                    else {
                        continue;
                    };
                    if !accepted_rounds.remove(&identity.round_id) {
                        continue;
                    }
                    committed_round_count += 1;
                    latest_evidence = Some(EvidenceRef::Record {
                        session_id: session.header().session_id.clone(),
                        record_seq: seq,
                        content_hash: format::hash(record.raw.as_bytes()),
                    });
                }
                _ => {}
            }
        }
        if let Some(evidence) = latest_evidence {
            facts.insert(
                "committed-round-count".into(),
                WorldFact {
                    value: serde_json::json!(committed_round_count),
                    evidence_refs: vec![evidence],
                },
            );
        }
        ConfirmedWorldView::capture(session, &self.revision, facts)
    }
    fn context(&self, _: &ConfirmedWorldView) -> Result<String, Fault> {
        Ok("当前登记为最小探索章节，玩家身份 player，场景身份 entry，规则 pbta-explore-v1。原文提供背景，未登记的多幕计数、结局与数值不具备执行权。没有出口条件，场景提议输出合法 stay。".into())
    }
    fn check_plan(
        &self,
        rule: &str,
        actor: &str,
        view: &ConfirmedWorldView,
    ) -> Result<CheckPlan, Fault> {
        if rule != "pbta-explore-v1" || actor != "player" || view.world_revision() != self.revision
        {
            return Err(unsupported());
        }
        super::dice::freeze_pbta(super::dice::PlanSpec {
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
        _: &super::recovery::RoundFacts,
        _: &ConfirmedWorldView,
    ) -> Result<Vec<WorldMutation>, Fault> {
        Ok(vec![])
    }
    fn exit_reason(&self, _: &str, _: &ConfirmedWorldView) -> Result<Option<String>, Fault> {
        Ok(None)
    }
}
type Completed = super::cycle::CycleRuntime;
/// 在原始 header 完整匹配时登记固定目录；未知旧版本保留磁盘事实，不猜测迁移。
/// # Errors
/// 不支持的剧本版本为 app.not-ready；解释器 / 场景或存储恢复错误原样返回。
pub async fn open(
    storage: Arc<Storage>,
    session: Arc<Mutex<Session>>,
    coordinator: Coordinator,
    events: Arc<dyn super::state::PhaseEvents>,
) -> Result<Arc<Context>, Fault> {
    let id = session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .header()
        .script_id
        .clone();
    let original = super::assets::scenario(&id)?;
    let expected_revision = session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .header()
        .script_revision
        .clone();
    let entity_directory = super::assets::entity_directory_for_revision(&id, &expected_revision)?;
    let revision = entity_directory.script_revision().to_owned();
    let expected_prefix = prefix(&id)?;
    {
        let record = session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let actual = record.header();
        if actual.script_revision != revision
            || actual.static_prefix_hash != format::hash(expected_prefix.as_bytes())
            || actual.static_prefix != expected_prefix
        {
            return Err(asset_unavailable());
        }
    }
    open_context(
        storage,
        session,
        coordinator,
        events,
        original.title,
        revision,
        entity_directory,
    )
    .await
}

// 资源匹配与可信目录装配分开；目录仍核验输入标题，资源登记错误不得进入会话。
async fn open_context(
    storage: Arc<Storage>,
    session: Arc<Mutex<Session>>,
    coordinator: Coordinator,
    events: Arc<dyn super::state::PhaseEvents>,
    title: String,
    revision: String,
    entity_directory: EntityDirectory,
) -> Result<Arc<Context>, Fault> {
    let catalog = memory_scene_catalog(title, entity_directory.clone())?;
    let session_id = session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .header()
        .session_id
        .clone();
    let completed = Arc::new(Completed::new(
        storage.clone(),
        Arc::new(catalog.clone()),
        session_id.clone(),
    ));
    let context = Context::open(
        session,
        Box::new(BuiltinDomain {
            catalog: catalog.clone(),
            storage: storage.clone(),
            revision,
            entity_seed: entity_directory,
        }),
        "player".into(),
        "player".into(),
        coordinator,
        events,
        completed.clone(),
    )
    .await?;
    let run = storage.initialize_memory_run(&session_id, &catalog)?;
    completed.set_run_id(run.run_id)?;
    if !context.publisher.snapshot().state.needs_recovery {
        completed.drain_cycle_events()?;
        ensure_current_cycle_node(&context)?;
    }
    Ok(context)
}

/// 周目初始化或重新打开时，只为尚无状态的当前节点建立一次基线。
/// # Errors
/// 当前世界、规则快照与记忆因果边界不一致时拒绝登记运行时。
pub(crate) fn ensure_current_cycle_node(context: &Context) -> Result<(), Fault> {
    let Some(scene) = context
        .publisher
        .snapshot()
        .state
        .scene
        .map(|scene| scene.scene_id)
    else {
        return Ok(());
    };
    let session = context
        .session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let domain = context
        .domain
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let world = domain.confirmed(&session)?;
    let history_revision = world.history_revision();
    let evidence = super::cycle::current_progress_evidence(domain.catalog(), &scene, &world)?;
    drop(domain);
    drop(session);
    context
        .completed
        .ensure_current_node(&scene, history_revision, &evidence)
}

/// 活动内置领域与记忆证据读取共用同一目录构造，避免规则装配分叉。
pub(crate) fn memory_scene_catalog(
    title: String,
    entity_directory: EntityDirectory,
) -> Result<SceneCatalog, Fault> {
    let revision = entity_directory.script_revision().to_owned();
    SceneCatalog::new(
        "minimal-explore-v1".into(),
        revision.clone(),
        BTreeMap::from([(
            "entry".into(),
            Node {
                kind: "scene".into(),
                title: title.clone(),
                parent: None,
            },
        )]),
        BTreeMap::from([(
            "entry".into(),
            Scene {
                position: ScenePosition {
                    scene_id: "entry".into(),
                    path: vec![SceneNode {
                        kind: "scene".into(),
                        id: "entry".into(),
                        title: title.clone(),
                    }],
                },
                rules: SceneRuleView {
                    scene_id: "entry".into(),
                    catalog_revision: "minimal-explore-v1".into(),
                    advance_rule_ids: vec![],
                    progress_rule_ids: vec![],
                    hint_rules: vec![],
                    exit_rule_ids: vec![],
                },
                advance: vec![],
                check_rule_ids: vec!["pbta-explore-v1".into()],
                actor_ids: vec!["player".into()],
                forced_check: None,
                dice_disabled: false,
            },
        )]),
        BTreeMap::new(),
    )?
    .with_entity_directory(entity_directory)?
    .with_presence_policy(entry_presence_policy())
}

// 此策略只含源码固定的内置规则；通用构造器的输入错误由 PresencePolicy 测试覆盖。
fn entry_presence_policy() -> crate::game::presence::PresencePolicy {
    crate::game::presence::PresencePolicy::new(
        "entry".into(),
        crate::game::presence::SceneEntryPresence {
            rule_id: "mistbell-entry-presence-v1".into(),
            rule_version: 1,
            character_ids: vec!["player".into()],
        },
        BTreeMap::from([(
            "entry".into(),
            crate::game::presence::SceneEntryPresence {
                rule_id: "mistbell-entry-presence-v1".into(),
                rule_version: 1,
                character_ids: vec!["player".into()],
            },
        )]),
        vec![],
    )
    .expect("内置初始在场策略固定有效")
    .with_knowledge_rules(
        vec![crate::game::presence::ObservationRule {
            rule_id: "mistbell-entry-observation-v1".into(),
            rule_version: 1,
            scene_id: "entry".into(),
            scope_id: "mistbell-entry-visible-v1".into(),
            source_kind: crate::game::presence::ObservationSourceKind::SceneNarration,
        }],
        vec![crate::game::presence::RumorRule {
            rule_id: "mistbell-entry-rumor-v1".into(),
            rule_version: 1,
            scene_id: "entry".into(),
            scope_id: "mistbell-entry-rumor-v1".into(),
            require_informant_present: true,
        }],
    )
    .expect("内置获知规则固定有效")
}

fn asset_unavailable() -> Fault {
    Fault::new("app.not-ready", "内置剧本或存档版本不可用")
}
fn unsupported() -> Fault {
    Fault::new("engine.invalid-phase", "此规则或世界操作未登记")
}
#[cfg(test)]
mod tests;
