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
}
impl WorldPort for BuiltinDomain {
    fn registered(&self, kind: &str, version: u32) -> bool {
        kind == "sessionEnded" && version == 1
    }
    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault> {
        self.storage.with_database(|connection| {
            aoidos_store::applied::contains(connection, id, hash).map_err(Fault::from)
        })
    }
    fn apply(&mut self, mutation: &WorldMutation, hash: &str) -> Result<(), Fault> {
        if !self.registered(&mutation.data.kind, mutation.data.version) {
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
    fn confirmed(&self, session: &Session) -> Result<ConfirmedWorldView, Fault> {
        self.storage.ready()?;
        ConfirmedWorldView::capture(session, &self.revision, BTreeMap::new())
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
struct Completed;
impl CompletedRounds for Completed {
    fn completed(&self, _: CompletedRound) {}
}
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
    let revision = super::assets::revision(&id)?;
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
) -> Result<Arc<Context>, Fault> {
    let catalog = SceneCatalog::new(
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
    )?;
    Context::open(
        session,
        Box::new(BuiltinDomain {
            catalog,
            storage,
            revision,
        }),
        "player".into(),
        "player".into(),
        coordinator,
        events,
        Arc::new(Completed),
    )
    .await
}
fn asset_unavailable() -> Fault {
    Fault::new("app.not-ready", "内置剧本或存档版本不可用")
}
fn unsupported() -> Fault {
    Fault::new("engine.invalid-phase", "此规则或世界操作未登记")
}
#[cfg(test)]
mod tests;
