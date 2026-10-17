//! 可信场景目录与统一只读条件求值；模型只提交登记 ID，不提交规则或依据。

use crate::{
    fault::Fault,
    record::{facts::ScenePosition, format, session::Session},
};
use aoidos_memory::model::{CycleKind, CycleMarkAuthorization, DetailPolicy, Run};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

mod entities;
use super::presence::PresencePolicy;
pub use entities::{
    CanonicalEntity, ENTITY_REGISTRY_MIGRATION, EntityCreationCommit, EntityCreationIntent,
    EntityCreationRule, EntityDirectory, EntityKind, EntityRegistry, verify_entity_registry_schema,
};
use entities::{MAX_SCOPE_BYTES, valid_label};

/// 最小类型化条件；数值 / 键由可信剧本登记，不解释模型自由文本。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Condition {
    Always {},
    Equals { key: String, value: Value },
    AtLeast { key: String, value: i64 },
    All { conditions: Vec<Condition> },
    Any { conditions: Vec<Condition> },
    Not { condition: Box<Condition> },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EvidenceRef {
    Script {
        script_revision: String,
        rule_version: u32,
        key: String,
        value_hash: String,
    },
    Record {
        session_id: String,
        record_seq: u64,
        content_hash: String,
    },
}
/// 解释器交出的标量及其依据，不接受无依据的临时模型赋值。
#[derive(Debug, Clone)]
pub struct WorldFact {
    pub value: Value,
    pub evidence_refs: Vec<EvidenceRef>,
}
/// 只通过当前 Session 的有效路径核验构造，不允许从 IPC 传入世界投影。
#[derive(Debug, Clone)]
pub struct ConfirmedWorldView {
    session_id: String,
    script_revision: String,
    world_revision: String,
    history_revision: u64,
    facts: BTreeMap<String, WorldFact>,
}
impl ConfirmedWorldView {
    /// 调用方必须在世界所有者临界区提供已 applied 的 revision / 只读标量。
    /// # Errors
    /// pending、未知记录、非法 revision / 标量或不在有效路径的依据拒绝。
    pub fn capture(
        session: &Session,
        revision: &str,
        facts: BTreeMap<String, WorldFact>,
    ) -> Result<Self, Fault> {
        if session.needs_recovery() {
            return Err(super::invalid_phase());
        }
        if revision.is_empty() || revision.len() > 256 || facts.len() > 256 {
            return Err(Fault::bad_request());
        }
        let path = session
            .path_at(session.history_revision())?
            .into_iter()
            .collect::<BTreeSet<_>>();
        for (key, fact) in &facts {
            if !format::valid_id(key)
                || !scalar(&fact.value)
                || fact.evidence_refs.is_empty()
                || fact.evidence_refs.len() > 32
            {
                return Err(Fault::bad_request());
            }
            for evidence in &fact.evidence_refs {
                match evidence {
                    EvidenceRef::Script {
                        script_revision,
                        rule_version,
                        key: source,
                        value_hash,
                    } => {
                        if script_revision != &session.header().script_revision
                            || *rule_version == 0
                            || source != key
                            || *value_hash != value_hash_of(&fact.value)?
                        {
                            return Err(super::invalid_phase());
                        }
                    }
                    EvidenceRef::Record {
                        session_id,
                        record_seq,
                        content_hash,
                    } => {
                        if session_id != &session.header().session_id || !path.contains(record_seq)
                        {
                            return Err(super::invalid_phase());
                        }
                        let record = session.read(*record_seq).map_err(Fault::from)?;
                        if record.read_only || format::hash(record.raw.as_bytes()) != *content_hash
                        {
                            return Err(super::invalid_phase());
                        }
                    }
                }
            }
        }
        Ok(Self {
            session_id: session.header().session_id.clone(),
            script_revision: session.header().script_revision.clone(),
            world_revision: revision.into(),
            history_revision: session.history_revision(),
            facts,
        })
    }
    pub fn world_revision(&self) -> &str {
        &self.world_revision
    }
    pub fn history_revision(&self) -> u64 {
        self.history_revision
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}
fn scalar(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(text) => text.len() <= 4096,
        _ => false,
    }
}
fn value_hash_of(value: &Value) -> Result<String, Fault> {
    Ok(format::hash(
        format::line(value).map_err(Fault::from)?.as_bytes(),
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HintRule {
    pub hint_id: String,
    pub when_rule_id: String,
    pub text: String,
    pub priority: i32,
    pub target_scope: String,
}

/// 允许记忆刻痕作用到某类、某个可信目录范围内的实体。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CycleSubjectScope {
    kind: EntityKind,
    scope: String,
}

impl CycleSubjectScope {
    /// # Errors
    /// 类别范围名称为空、过长或不合法时拒绝。
    pub fn new(kind: EntityKind, scope: String) -> Result<Self, Fault> {
        if !valid_label(&scope, MAX_SCOPE_BYTES) {
            return Err(Fault::bad_request());
        }
        Ok(Self { kind, scope })
    }
}

/// 剧本可授权的刻痕规则；规则属性不来自模型提案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleMarkRule {
    rule_id: String,
    rule_version: u32,
    subject_scopes: Vec<CycleSubjectScope>,
    imprint_key: String,
    kind: CycleKind,
    detail_policy: DetailPolicy,
    priority: u16,
}

impl CycleMarkRule {
    /// # Errors
    /// 规则 ID、版本、对象范围或优先级超出硬限制时拒绝。
    pub fn new(
        rule_id: String,
        rule_version: u32,
        subject_scopes: Vec<CycleSubjectScope>,
        imprint_key: String,
        kind: CycleKind,
        detail_policy: DetailPolicy,
        priority: u16,
    ) -> Result<Self, Fault> {
        if !format::valid_id(&rule_id)
            || rule_version == 0
            || subject_scopes.is_empty()
            || subject_scopes.len() > 32
            || subject_scopes.iter().collect::<BTreeSet<_>>().len() != subject_scopes.len()
            || !format::valid_id(&imprint_key)
            || priority > 1_000
        {
            return Err(Fault::bad_request());
        }
        Ok(Self {
            rule_id,
            rule_version,
            subject_scopes,
            imprint_key,
            kind,
            detail_policy,
            priority,
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneRuleView {
    pub scene_id: String,
    pub catalog_revision: String,
    pub advance_rule_ids: Vec<String>,
    pub progress_rule_ids: Vec<String>,
    pub hint_rules: Vec<HintRule>,
    pub exit_rule_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneRuleMatch {
    pub rule_id: String,
    pub matched: bool,
    pub catalog_revision: String,
    pub world_revision: String,
    pub history_revision: u64,
    pub evidence_refs: Vec<EvidenceRef>,
}
/// 静态父子关系由 kind 与 id 的目录定义；标题仅展示，不作为选场 ID。
#[derive(Debug, Clone)]
pub struct Node {
    pub kind: String,
    pub title: String,
    pub parent: Option<String>,
}
#[derive(Debug, Clone)]
pub struct Advance {
    pub rule_id: String,
    pub target_scene_id: String,
    pub priority: i32,
}
#[derive(Debug, Clone)]
pub struct Scene {
    pub position: ScenePosition,
    pub rules: SceneRuleView,
    pub advance: Vec<Advance>,
    pub check_rule_ids: Vec<String>,
    pub actor_ids: Vec<String>,
    pub forced_check: Option<String>,
    pub dice_disabled: bool,
}
/// 只存可信登记；构造先验证所有路径、规则、预算，运行中不增补未知 ID。
#[derive(Clone)]
pub struct SceneCatalog {
    revision: String,
    script_revision: String,
    nodes: BTreeMap<String, Node>,
    scenes: BTreeMap<String, Scene>,
    conditions: BTreeMap<String, Condition>,
    entities: Option<EntityDirectory>,
    entity_creation_rules: BTreeMap<String, EntityCreationRule>,
    cycle_mark_rules: BTreeMap<String, CycleMarkRule>,
    presence: Option<PresencePolicy>,
}
impl SceneCatalog {
    /// # Errors
    /// 非法目录、路径、规则引用、父链循环或载荷超限拒绝整份登记。
    pub fn new(
        revision: String,
        script_revision: String,
        nodes: BTreeMap<String, Node>,
        scenes: BTreeMap<String, Scene>,
        conditions: BTreeMap<String, Condition>,
    ) -> Result<Self, Fault> {
        if revision.is_empty()
            || revision.len() > 256
            || !format::valid_hash(&script_revision)
            || nodes.len() > 4096
            || scenes.len() > 1024
            || conditions.len() > 4096
        {
            return Err(Fault::bad_request());
        }
        for (id, node) in &nodes {
            if !format::valid_id(id)
                || !format::valid_id(&node.kind)
                || node.title.len() > 256
                || node.title.trim().is_empty()
            {
                return Err(Fault::bad_request());
            }
            let mut ancestors = BTreeSet::new();
            let mut current = Some(id.as_str());
            while let Some(id) = current {
                if !ancestors.insert(id) || ancestors.len() > 16 {
                    return Err(Fault::bad_request());
                }
                current = nodes
                    .get(id)
                    .ok_or_else(Fault::bad_request)?
                    .parent
                    .as_deref();
            }
        }
        for (id, condition) in &conditions {
            if !format::valid_id(id) {
                return Err(Fault::bad_request());
            }
            validate_condition(condition, 0, &mut 0)?;
        }
        let catalog = Self {
            revision,
            script_revision,
            nodes,
            scenes,
            conditions,
            entities: None,
            entity_creation_rules: BTreeMap::new(),
            cycle_mark_rules: BTreeMap::new(),
            presence: None,
        };
        for (id, scene) in &catalog.scenes {
            if id != &scene.position.scene_id
                || id != &scene.rules.scene_id
                || scene.rules.catalog_revision != catalog.revision
                || !valid_ids(&scene.check_rule_ids)
                || !valid_ids(&scene.actor_ids)
                || scene.actor_ids.is_empty()
                || scene
                    .forced_check
                    .as_ref()
                    .is_some_and(|id| !scene.check_rule_ids.contains(id) || scene.dice_disabled)
            {
                return Err(Fault::bad_request());
            }
            catalog.verify_position(&scene.position)?;
            validate_rule_view(&scene.rules)?;
            for rule in all_rule_ids(&scene.rules) {
                if !catalog.conditions.contains_key(rule) {
                    return Err(Fault::bad_request());
                }
            }
            if scene.advance.len() > 64 {
                return Err(Fault::bad_request());
            }
            for advance in &scene.advance {
                if !scene.rules.advance_rule_ids.contains(&advance.rule_id)
                    || !catalog.scenes.contains_key(&advance.target_scene_id)
                {
                    return Err(Fault::bad_request());
                }
            }
        }
        Ok(catalog)
    }
    /// 将与同一剧本版本绑定的可信实体目录接入场景目录。
    ///
    /// 完整目录必须包含所有场景 actor；不完整目录可保留作状态提示，但不能解析名称，
    /// 也不会据此授予获知证据或登记别名。
    /// # Errors
    /// 剧本版本不匹配，或完整目录缺少任一场景 actor 时拒绝。
    pub fn with_entity_directory(mut self, directory: EntityDirectory) -> Result<Self, Fault> {
        if directory.script_revision() != self.script_revision {
            return Err(Fault::bad_request());
        }
        if directory.is_complete()
            && self
                .scenes
                .values()
                .flat_map(|scene| &scene.actor_ids)
                .any(|actor_id| !directory.contains_character(actor_id))
        {
            return Err(Fault::bad_request());
        }
        if let Some(policy) = &self.presence {
            policy.validate_directory(&directory)?;
        }
        self.entities = Some(directory);
        Ok(self)
    }
    /// 返回已接入快照；不完整状态可见，但 `resolve_exact` 会拒绝解析。
    pub fn entity_directory(&self) -> Option<&EntityDirectory> {
        self.entities.as_ref()
    }

    /// 将完整且带版本的场景在场策略绑定到可信目录。
    /// # Errors
    /// 场景未登记，或策略与实体目录不兼容时拒绝。
    pub fn with_presence_policy(mut self, policy: PresencePolicy) -> Result<Self, Fault> {
        policy.validate_catalog(&self)?;
        self.entities
            .as_ref()
            .map(|directory| policy.validate_directory(directory))
            .transpose()?;
        self.presence = Some(policy);
        Ok(self)
    }

    pub fn presence_policy(&self) -> Option<&PresencePolicy> {
        self.presence.as_ref()
    }

    /// 登记可由可信世界解释器调用的动态实体创建规则。
    ///
    /// 这些规则属于场景目录版本；调用方必须在规则变化时更新 `revision`。仅有规则登记不
    /// 授予模型、IPC 或记忆候选创建身份的能力。
    /// # Errors
    /// 重复规则身份、空规则集、超限或规则身份非法时拒绝。
    pub fn with_entity_creation_rules(
        mut self,
        rules: Vec<EntityCreationRule>,
    ) -> Result<Self, Fault> {
        if rules.is_empty() || rules.len() > 256 {
            return Err(Fault::bad_request());
        }
        let mut registered = BTreeMap::new();
        for rule in rules {
            if registered.insert(rule.rule_id.clone(), rule).is_some() {
                return Err(Fault::bad_request());
            }
        }
        self.entity_creation_rules = registered;
        Ok(self)
    }

    /// 登记与当前 catalog revision 绑定的跨周目刻痕授权规则。
    ///
    /// 规则只声明实体类别与 scope；实际主体 ID 在每次构造授权上下文时，
    /// 从对应 session 的完整实体快照展开，因而可包含已提交的动态实体。
    /// # Errors
    /// 规则集为空、超限或包含重复身份时拒绝。
    pub fn with_cycle_mark_rules(mut self, rules: Vec<CycleMarkRule>) -> Result<Self, Fault> {
        if rules.is_empty() || rules.len() > 64 {
            return Err(Fault::bad_request());
        }
        let mut registered = BTreeMap::new();
        for rule in rules {
            if registered.insert(rule.rule_id.clone(), rule).is_some() {
                return Err(Fault::bad_request());
            }
        }
        self.cycle_mark_rules = registered;
        Ok(self)
    }

    /// 以可信规则、当前 run 和完整实体目录生成提案可用的主体白名单。
    /// # Errors
    /// run 或实体快照无效、目录修订不匹配、展开集合超过硬上限时拒绝。
    pub fn cycle_mark_authorizations(
        &self,
        run: &Run,
        session: &Session,
        directory: &EntityDirectory,
    ) -> Result<BTreeMap<String, CycleMarkAuthorization>, Fault> {
        if session.needs_recovery()
            || run.script_id != session.header().script_id
            || run.session_id != session.header().session_id
            || !directory.is_complete()
            || !directory.is_session_snapshot_for(session)
            || session.header().script_revision != self.script_revision
            || directory.script_revision() != self.script_revision
            || !format::valid_uuid(&run.run_id)
            || run.cycle_gen == 0
            || run.cycle_gen > aoidos_memory::model::MAX_INTEGER
            || run.policy.validate().is_err()
        {
            return Err(Fault::bad_request());
        }
        let mut authorizations = BTreeMap::new();
        for rule in self.cycle_mark_rules.values() {
            let allowed_subject_ids = directory
                .entries()
                .filter(|entity| {
                    rule.subject_scopes
                        .iter()
                        .any(|scope| entity.kind == scope.kind && entity.scope == scope.scope)
                })
                .map(|entity| entity.id.clone())
                .collect::<BTreeSet<_>>();
            if allowed_subject_ids.is_empty() {
                continue;
            }
            if allowed_subject_ids.len() > 2_048 {
                return Err(Fault::bad_request());
            }
            authorizations.insert(
                rule.rule_id.clone(),
                CycleMarkAuthorization {
                    rule_id: rule.rule_id.clone(),
                    script_id: run.script_id.clone(),
                    run_id: run.run_id.clone(),
                    cycle_gen: run.cycle_gen,
                    rule_version: rule.rule_version,
                    allowed_subject_ids,
                    imprint_key: rule.imprint_key.clone(),
                    kind: rule.kind,
                    detail_policy: rule.detail_policy,
                    priority: rule.priority,
                },
            );
        }
        Ok(authorizations)
    }

    pub(crate) fn entity_creation_rule(&self, id: &str) -> Option<&EntityCreationRule> {
        self.entity_creation_rules.get(id)
    }
    pub fn revision(&self) -> &str {
        &self.revision
    }
    pub(crate) fn script_revision(&self) -> &str {
        &self.script_revision
    }
    /// # Errors
    /// 未登记场景为 not-found。
    pub fn scene(&self, id: &str) -> Result<&Scene, Fault> {
        self.scenes.get(id).ok_or_else(Fault::not_found)
    }
    pub(crate) fn scene_ids(&self) -> impl Iterator<Item = &str> {
        self.scenes.keys().map(String::as_str)
    }
    /// # Errors
    /// 旧目录 revision / 未知场景拒绝，不猜测规则集合。
    pub fn get_scene_rule_view(
        &self,
        scene_id: &str,
        revision: &str,
    ) -> Result<SceneRuleView, Fault> {
        if revision != self.revision {
            return Err(super::invalid_phase());
        }
        Ok(self.scene(scene_id)?.rules.clone())
    }
    /// 同一条件层供场景晋级和 032 消费，只读且不执行 RNG / LLM。
    /// # Errors
    /// 未登记规则、过期视图、缺依据及结果超过 32 个引用均显式拒绝。
    pub fn evaluate_scene_rule(
        &self,
        scene_id: &str,
        rule_id: &str,
        world: &ConfirmedWorldView,
        current_revision: &str,
        current_history: u64,
    ) -> Result<SceneRuleMatch, Fault> {
        if world.world_revision() != current_revision
            || world.history_revision() != current_history
            || world.script_revision != self.script_revision
        {
            return Err(super::invalid_phase());
        }
        let scene = self.scene(scene_id)?;
        if !all_rule_ids(&scene.rules).any(|id| id == rule_id) {
            return Err(super::invalid_phase());
        }
        let condition = self
            .conditions
            .get(rule_id)
            .ok_or_else(super::invalid_phase)?;
        let mut evidence = vec![EvidenceRef::Script {
            script_revision: self.script_revision.clone(),
            rule_version: 1,
            key: rule_id.into(),
            value_hash: format::hash(format::line(condition).map_err(Fault::from)?.as_bytes()),
        }];
        let matched = evaluate(condition, world, &mut evidence)?;
        Ok(SceneRuleMatch {
            rule_id: rule_id.into(),
            matched,
            catalog_revision: self.revision.clone(),
            world_revision: world.world_revision.clone(),
            history_revision: world.history_revision,
            evidence_refs: evidence,
        })
    }
    /// 校验最大深度、唯一节点、目录 kind / 标题和精确父子关系。
    /// # Errors
    /// 未登记节点或路径不属于目标场景拒绝。
    pub fn verify_position(&self, position: &ScenePosition) -> Result<(), Fault> {
        if !self.scenes.contains_key(&position.scene_id)
            || position.path.is_empty()
            || position.path.len() > 16
            || position.path.last().map(|node| &node.id) != Some(&position.scene_id)
        {
            return Err(Fault::bad_request());
        }
        let mut parent = None;
        let mut seen = BTreeSet::new();
        for node in &position.path {
            let registered = self.nodes.get(&node.id).ok_or_else(Fault::bad_request)?;
            if !seen.insert(&node.id)
                || registered.parent.as_ref() != parent
                || registered.kind != node.kind
                || registered.title != node.title
            {
                return Err(Fault::bad_request());
            }
            parent = Some(&node.id);
        }
        Ok(())
    }
    /// 只输出通过统一条件层核验的有限候选，按可信优先级与 ID 稳定排序。
    /// # Errors
    /// 世界 / 规则不可求值时拒绝，不用空候选掩盖错误。
    pub fn candidates(
        &self,
        scene_id: &str,
        world: &ConfirmedWorldView,
    ) -> Result<(Vec<String>, bool), Fault> {
        let scene = self.scene(scene_id)?;
        let mut candidates = Vec::new();
        for advance in &scene.advance {
            if self
                .evaluate_scene_rule(
                    scene_id,
                    &advance.rule_id,
                    world,
                    world.world_revision(),
                    world.history_revision(),
                )?
                .matched
            {
                candidates.push((advance.priority, advance.target_scene_id.clone()));
            }
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let mut seen = BTreeSet::new();
        candidates.retain(|(_, id)| seen.insert(id.clone()));
        let truncated = candidates.len() > 32;
        candidates.truncate(32);
        Ok((
            candidates.into_iter().map(|(_, id)| id).collect(),
            truncated,
        ))
    }
}
fn all_rule_ids(view: &SceneRuleView) -> impl Iterator<Item = &String> {
    view.advance_rule_ids
        .iter()
        .chain(&view.progress_rule_ids)
        .chain(&view.exit_rule_ids)
        .chain(view.hint_rules.iter().map(|hint| &hint.when_rule_id))
}
fn validate_rule_view(view: &SceneRuleView) -> Result<(), Fault> {
    if [
        &view.advance_rule_ids,
        &view.progress_rule_ids,
        &view.exit_rule_ids,
    ]
    .iter()
    .any(|ids| !valid_ids(ids))
        || view.hint_rules.len() > 32
        || view.hint_rules.iter().any(|hint| {
            !format::valid_id(&hint.hint_id)
                || !format::valid_id(&hint.when_rule_id)
                || !format::valid_id(&hint.target_scope)
                || hint.text.trim().is_empty()
                || hint.text.len() > 512
        })
        || view
            .hint_rules
            .iter()
            .map(|hint| &hint.hint_id)
            .collect::<BTreeSet<_>>()
            .len()
            != view.hint_rules.len()
        || format::line(view).map_err(Fault::from)?.len() > 64 * 1024
    {
        return Err(Fault::bad_request());
    }
    Ok(())
}
fn valid_ids(ids: &[String]) -> bool {
    ids.len() <= 64
        && ids.iter().all(|id| format::valid_id(id))
        && ids.iter().collect::<BTreeSet<_>>().len() == ids.len()
}
fn validate_condition(condition: &Condition, depth: usize, count: &mut usize) -> Result<(), Fault> {
    *count += 1;
    if depth > 16 || *count > 128 {
        return Err(Fault::bad_request());
    }
    match condition {
        Condition::Always {} => {}
        Condition::Equals { key, value } => {
            if !format::valid_id(key) || !scalar(value) {
                return Err(Fault::bad_request());
            }
        }
        Condition::AtLeast { key, .. } => {
            if !format::valid_id(key) {
                return Err(Fault::bad_request());
            }
        }
        Condition::All { conditions } | Condition::Any { conditions } => {
            if conditions.is_empty() {
                return Err(Fault::bad_request());
            }
            for condition in conditions {
                validate_condition(condition, depth + 1, count)?;
            }
        }
        Condition::Not { condition } => validate_condition(condition, depth + 1, count)?,
    }
    Ok(())
}
fn evaluate(
    condition: &Condition,
    world: &ConfirmedWorldView,
    evidence: &mut Vec<EvidenceRef>,
) -> Result<bool, Fault> {
    let mut fact = |key: &str| -> Result<&Value, Fault> {
        let fact = world.facts.get(key).ok_or_else(super::invalid_phase)?;
        for item in &fact.evidence_refs {
            if !evidence.contains(item) {
                if evidence.len() == 32 {
                    return Err(super::invalid_phase());
                }
                evidence.push(item.clone());
            }
        }
        Ok(&fact.value)
    };
    Ok(match condition {
        Condition::Always {} => true,
        Condition::Equals { key, value } => {
            let actual = fact(key)?;
            if !matches!(
                (actual, value),
                (Value::Null, Value::Null)
                    | (Value::Bool(_), Value::Bool(_))
                    | (Value::Number(_), Value::Number(_))
                    | (Value::String(_), Value::String(_))
            ) {
                return Err(super::invalid_phase());
            }
            actual == value
        }
        Condition::AtLeast { key, value } => {
            fact(key)?.as_i64().ok_or_else(super::invalid_phase)? >= *value
        }
        Condition::All { conditions } => {
            let mut result = true;
            for condition in conditions {
                result &= evaluate(condition, world, evidence)?;
            }
            result
        }
        Condition::Any { conditions } => {
            let mut result = false;
            for condition in conditions {
                result |= evaluate(condition, world, evidence)?;
            }
            result
        }
        Condition::Not { condition } => !evaluate(condition, world, evidence)?,
    })
}

#[cfg(test)]
mod tests;
