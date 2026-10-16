//! 场景在场与可信获知的 engine 边界；不依赖记忆协议或 UI 状态。

mod evidence;
pub(super) use crate::game::invalid_phase;
pub use evidence::{
    EvidenceKind, EvidenceSource, ObservedEvent, ObservedEvidenceInput, RumorDelivery,
    RumorUncertainty, SceneEvidence, TrustedEvidence,
};

use crate::{
    fault::Fault,
    game::catalog::{CanonicalEntity, EntityDirectory, EntityKind, SceneCatalog},
    record::{
        facts::{Fact, MutationData, WorldMutation},
        format,
        session::Session,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

const MAX_CHARACTERS: usize = 512;
const MAX_POLICY_BYTES: usize = 64 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 256 * 1024;
const MAX_RULES: usize = 256;
const MAX_EVIDENCE_RECORDS: usize = 4096;
const MAX_EVIDENCE_BYTES: usize = 16 * 1024 * 1024;

/// 进入新场景时由已登记目录明确提供的初始在场角色。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneEntryPresence {
    pub rule_id: String,
    pub rule_version: u32,
    pub character_ids: Vec<String>,
}

/// 允许世界解释器在指定场景中执行的成员变更，不授予模型或 UI 写入权限。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresenceChangeRule {
    pub rule_id: String,
    pub rule_version: u32,
    pub scene_id: String,
    pub entity_scope: String,
    pub allow_enter: bool,
    pub allow_leave: bool,
}

/// 可观察证据允许引用的正文类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// 首版只把公开场景旁白作为可观察动作来源。
pub enum ObservationSourceKind {
    SceneNarration,
}

/// 将具体记录区间登记为可观察证据的规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationRule {
    pub rule_id: String,
    pub rule_version: u32,
    pub scene_id: String,
    pub scope_id: String,
    pub source_kind: ObservationSourceKind,
}

/// 单次定向传闻交付规则；接收者必须在场。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RumorRule {
    pub rule_id: String,
    pub rule_version: u32,
    pub scene_id: String,
    pub scope_id: String,
    pub require_informant_present: bool,
}

/// SceneCatalog 的可信成员规则；所有列表顺序规范化，规则身份随 catalog revision 冻结。
#[derive(Debug, Clone)]
pub struct PresencePolicy {
    initial_scene_id: String,
    initial: SceneEntryPresence,
    entries: BTreeMap<String, SceneEntryPresence>,
    changes: BTreeMap<String, PresenceChangeRule>,
    observations: BTreeMap<String, ObservationRule>,
    rumors: BTreeMap<String, RumorRule>,
}

impl PresencePolicy {
    /// # Errors
    /// 空身份、重复角色、空规则、超限配置或规则版本无效时拒绝。
    pub fn new(
        initial_scene_id: String,
        initial: SceneEntryPresence,
        entries: BTreeMap<String, SceneEntryPresence>,
        changes: Vec<PresenceChangeRule>,
    ) -> Result<Self, Fault> {
        if !format::valid_id(&initial_scene_id)
            || !valid_entry(&initial)
            || entries.is_empty()
            || entries.len() > 1024
            || changes.len() > MAX_RULES
        {
            return Err(Fault::bad_request());
        }
        let mut by_id = BTreeMap::new();
        for rule in changes {
            if !format::valid_id(&rule.rule_id)
                || rule.rule_version == 0
                || !format::valid_id(&rule.scene_id)
                || !valid_label(&rule.entity_scope, 128)
                || (!rule.allow_enter && !rule.allow_leave)
                || by_id.insert(rule.rule_id.clone(), rule).is_some()
            {
                return Err(Fault::bad_request());
            }
        }
        if entries
            .iter()
            .any(|(scene_id, entry)| !format::valid_id(scene_id) || !valid_entry(entry))
        {
            return Err(Fault::bad_request());
        }
        let policy = Self {
            initial_scene_id,
            initial,
            entries,
            changes: by_id,
            observations: BTreeMap::new(),
            rumors: BTreeMap::new(),
        };
        let encoded = aoidos_json::to_vec(&policy.wire_value())
            .expect("presence policy contains only JSON-compatible fields");
        if encoded.len() > MAX_POLICY_BYTES {
            return Err(Fault::bad_request());
        }
        Ok(policy)
    }

    fn wire_value(&self) -> serde_json::Value {
        serde_json::json!({
            "initialSceneId": self.initial_scene_id,
            "initial": self.initial,
            "entries": self.entries,
            "changes": self.changes,
            "observations": self.observations,
            "rumors": self.rumors,
        })
    }

    pub(super) fn validate_catalog(&self, catalog: &SceneCatalog) -> Result<(), Fault> {
        if catalog.scene(&self.initial_scene_id).is_err()
            || self
                .entries
                .keys()
                .any(|scene_id| catalog.scene(scene_id).is_err())
            || catalog
                .scene_ids()
                .any(|scene_id| !self.entries.contains_key(scene_id))
            || self
                .changes
                .values()
                .any(|rule| catalog.scene(&rule.scene_id).is_err())
            || self
                .observations
                .values()
                .any(|rule| catalog.scene(&rule.scene_id).is_err())
            || self
                .rumors
                .values()
                .any(|rule| catalog.scene(&rule.scene_id).is_err())
            || self.entries.len() != catalog.scene_ids().count()
        {
            return Err(Fault::bad_request());
        }
        Ok(())
    }

    pub(super) fn validate_directory(&self, directory: &EntityDirectory) -> Result<(), Fault> {
        if !directory.is_complete()
            || self
                .entries
                .values()
                .chain(std::iter::once(&self.initial))
                .flat_map(|entry| &entry.character_ids)
                .any(|id| !directory.contains_character(id))
            || self
                .changes
                .values()
                .any(|rule| !directory.has_character_scope(&rule.entity_scope))
        {
            return Err(Fault::bad_request());
        }
        Ok(())
    }

    pub fn initial_scene_id(&self) -> &str {
        &self.initial_scene_id
    }

    pub fn initial(&self) -> &SceneEntryPresence {
        &self.initial
    }

    pub fn entry(&self, scene_id: &str) -> Option<&SceneEntryPresence> {
        self.entries.get(scene_id)
    }

    pub fn change_rule(&self, rule_id: &str) -> Option<&PresenceChangeRule> {
        self.changes.get(rule_id)
    }

    /// 在 presence policy 绑定到目录前，登记固定的观察 / 传闻规则身份。
    /// # Errors
    /// 重复、无版本、超限或身份冲突的规则会被拒绝。
    pub fn with_knowledge_rules(
        mut self,
        observations: Vec<ObservationRule>,
        rumors: Vec<RumorRule>,
    ) -> Result<Self, Fault> {
        if self
            .changes
            .len()
            .saturating_add(self.observations.len())
            .saturating_add(self.rumors.len())
            .saturating_add(observations.len())
            .saturating_add(rumors.len())
            > MAX_RULES
        {
            return Err(Fault::bad_request());
        }
        let mut ids = self
            .changes
            .keys()
            .chain(self.observations.keys())
            .chain(self.rumors.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        for rule in observations {
            if !valid_knowledge_rule(
                &rule.rule_id,
                rule.rule_version,
                &rule.scene_id,
                &rule.scope_id,
            ) || !ids.insert(rule.rule_id.clone())
                || self
                    .observations
                    .insert(rule.rule_id.clone(), rule)
                    .is_some()
            {
                return Err(Fault::bad_request());
            }
        }
        for rule in rumors {
            if !valid_knowledge_rule(
                &rule.rule_id,
                rule.rule_version,
                &rule.scene_id,
                &rule.scope_id,
            ) || !ids.insert(rule.rule_id.clone())
                || self.rumors.insert(rule.rule_id.clone(), rule).is_some()
            {
                return Err(Fault::bad_request());
            }
        }
        let encoded = aoidos_json::to_vec(&self.wire_value())
            .expect("presence policy contains only JSON-compatible fields");
        if encoded.len() > MAX_POLICY_BYTES {
            return Err(Fault::bad_request());
        }
        Ok(self)
    }

    pub fn observation_rule(&self, rule_id: &str) -> Option<&ObservationRule> {
        self.observations.get(rule_id)
    }

    pub fn rumor_rule(&self, rule_id: &str) -> Option<&RumorRule> {
        self.rumors.get(rule_id)
    }
}

fn valid_knowledge_rule(rule_id: &str, version: u32, scene_id: &str, scope_id: &str) -> bool {
    format::valid_id(rule_id)
        && version > 0
        && format::valid_id(scene_id)
        && valid_label(scope_id, 128)
}

fn valid_entry(entry: &SceneEntryPresence) -> bool {
    entry.rule_version > 0
        && format::valid_id(&entry.rule_id)
        && entry.character_ids.len() <= MAX_CHARACTERS
        && entry.character_ids.iter().all(|id| format::valid_id(id))
        && entry.character_ids.iter().collect::<BTreeSet<_>>().len() == entry.character_ids.len()
}

fn valid_label(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

/// 供产品在场查询与 030 只读消费的完整快照；成员必须来自完整实体目录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenePresenceSnapshot {
    pub session_id: String,
    pub scene_id: String,
    pub presence_revision: String,
    pub characters: Vec<CanonicalEntity>,
}

/// Rust 端只读构造当前有效路径上的在场快照；UI 返回值不能反向写入状态。
pub struct ScenePresence;

impl ScenePresence {
    /// # Errors
    /// pending / 不可恢复状态、缺失配置、目录不完整、失配回执或超限快照均拒绝。
    pub fn snapshot(
        connection: &rusqlite::Connection,
        session: &Session,
        catalog: &SceneCatalog,
        directory: &EntityDirectory,
    ) -> Result<ScenePresenceSnapshot, Fault> {
        if session.needs_recovery() || session.is_read_only() {
            return Err(super::invalid_phase());
        }
        let state = derive(connection, session, catalog, directory)?;
        let mut characters = state
            .members
            .iter()
            .map(|id| {
                directory
                    .entity(id)?
                    .filter(|entity| entity.kind == EntityKind::Character)
                    .cloned()
                    .ok_or_else(super::invalid_phase)
            })
            .collect::<Result<Vec<_>, _>>()?;
        characters.sort_by(|left, right| left.id.cmp(&right.id));
        let snapshot = ScenePresenceSnapshot {
            session_id: session.header().session_id.clone(),
            scene_id: state.scene_id,
            presence_revision: state.revision,
            characters,
        };
        let encoded = aoidos_json::to_vec(&snapshot)
            .expect("presence snapshot contains only JSON-compatible fields");
        if encoded.len() > MAX_SNAPSHOT_BYTES || snapshot.characters.len() > MAX_CHARACTERS {
            return Err(
                Fault::new("app.bad-request", "presence snapshot exceeds limits")
                    .with_detail(serde_json::json!({"reason":"presenceSnapshotLimit"})),
            );
        }
        Ok(snapshot)
    }

    /// 根据当前有效快照和已登记规则构造版本化变更意图。
    /// # Errors
    /// 缺少恢复状态、实体或规则，或变更过期 / 非法时，在写入记录前拒绝。
    pub fn prepare_change(
        connection: &rusqlite::Connection,
        session: &Session,
        catalog: &SceneCatalog,
        directory: &EntityDirectory,
        rule_id: &str,
        entity_id: &str,
        action: PresenceAction,
    ) -> Result<WorldMutation, Fault> {
        if session.needs_recovery() || session.is_read_only() {
            return Err(super::invalid_phase());
        }
        let state = derive(connection, session, catalog, directory)?;
        let policy = catalog.presence_policy().ok_or_else(super::invalid_phase)?;
        let rule = policy
            .change_rule(rule_id)
            .ok_or_else(super::invalid_phase)?;
        let entity = directory
            .entity(entity_id)?
            .filter(|entity| entity.kind == EntityKind::Character)
            .ok_or_else(Fault::bad_request)?;
        let allowed = match action {
            PresenceAction::Enter => rule.allow_enter,
            PresenceAction::Leave => rule.allow_leave,
        };
        if rule.scene_id != state.scene_id
            || entity.scope != rule.entity_scope
            || !allowed
            || match action {
                PresenceAction::Enter => state.members.contains(entity_id),
                PresenceAction::Leave => !state.members.contains(entity_id),
            }
        {
            return Err(Fault::bad_request());
        }
        let change = PresenceChange {
            version: 1,
            session_id: session.header().session_id.clone(),
            script_revision: session.header().script_revision.clone(),
            catalog_revision: catalog.revision().to_owned(),
            entity_id: entity_id.to_owned(),
            scene_id: state.scene_id,
            action,
            expected_presence_revision: state.revision,
            rule_id: rule.rule_id.clone(),
            rule_version: rule.rule_version,
        };
        Ok(WorldMutation {
            mutation_id: uuid::Uuid::new_v4().to_string(),
            data: MutationData {
                version: 1,
                kind: "scenePresenceChanged".into(),
                payload: aoidos_json::to_value(change)
                    .expect("presence change contains only JSON-compatible fields"),
            },
        })
    }

    /// 按当前已应用的在场状态校验完整、有序的 mutation 组。
    /// # Errors
    /// 未知规则、过期 revision、非法变更或超过快照上限时失败关闭。
    pub fn validate_mutations(
        connection: &rusqlite::Connection,
        session: &Session,
        catalog: &SceneCatalog,
        directory: &EntityDirectory,
        mutations: &[WorldMutation],
    ) -> Result<(), Fault> {
        if session.needs_recovery() || session.is_read_only() {
            return Err(super::invalid_phase());
        }
        let mut state = derive(connection, session, catalog, directory)?;
        let policy = catalog.presence_policy().ok_or_else(super::invalid_phase)?;
        evidence::validate_ordered(evidence::OrderedValidation {
            connection,
            state: &mut state,
            session,
            catalog,
            directory,
            policy,
            validation_revision: session.history_revision(),
            lookup_before_seq: None,
            mutations,
        })
    }

    /// 从 intent 之前的已应用状态校验原始有序 mutation 组，供崩溃恢复使用。
    /// # Errors
    /// intent 不在当前路径、分支边界缺失或组内任一变更失配时失败关闭。
    pub(crate) fn validate_recovery_mutations(
        connection: &rusqlite::Connection,
        session: &Session,
        catalog: &SceneCatalog,
        directory: &EntityDirectory,
        record_seq: u64,
        mutations: &[WorldMutation],
    ) -> Result<(), Fault> {
        let path = session.path_at(session.history_revision())?;
        if !path.contains(&record_seq) {
            return Err(super::invalid_phase());
        }
        let record = session
            .read(record_seq)?
            .body
            .ok_or_else(super::invalid_phase)?;
        let branch_revision = record.branch_seq.unwrap_or(0);
        let mut state = derive_before(
            connection,
            session,
            catalog,
            directory,
            record_seq,
            branch_revision,
        )?;
        let policy = catalog.presence_policy().ok_or_else(super::invalid_phase)?;
        evidence::validate_ordered(evidence::OrderedValidation {
            connection,
            state: &mut state,
            session,
            catalog,
            directory,
            policy,
            validation_revision: branch_revision,
            lookup_before_seq: Some(record_seq),
            mutations,
        })
    }

    /// 确认已提交的场景切换具有可信的进入成员快照。
    /// # Errors
    /// 目标场景没有已登记进入规则时失败关闭。
    pub fn validate_scene_advance(
        catalog: &SceneCatalog,
        position: &crate::record::facts::ScenePosition,
    ) -> Result<(), Fault> {
        let policy = catalog.presence_policy().ok_or_else(super::invalid_phase)?;
        if catalog.scene(&position.scene_id).is_err() || policy.entry(&position.scene_id).is_none()
        {
            return Err(super::invalid_phase());
        }
        Ok(())
    }
}

struct State {
    scene_id: String,
    members: BTreeSet<String>,
    revision: String,
    last_mutation_id: String,
}

fn derive(
    connection: &rusqlite::Connection,
    session: &Session,
    catalog: &SceneCatalog,
    directory: &EntityDirectory,
) -> Result<State, Fault> {
    derive_through(
        connection,
        session,
        catalog,
        directory,
        None,
        session.history_revision(),
    )
}

fn derive_before(
    connection: &rusqlite::Connection,
    session: &Session,
    catalog: &SceneCatalog,
    directory: &EntityDirectory,
    before_seq: u64,
    branch_revision: u64,
) -> Result<State, Fault> {
    derive_through(
        connection,
        session,
        catalog,
        directory,
        Some(before_seq),
        branch_revision,
    )
}

fn derive_through(
    connection: &rusqlite::Connection,
    session: &Session,
    catalog: &SceneCatalog,
    directory: &EntityDirectory,
    before_seq: Option<u64>,
    history_revision: u64,
) -> Result<State, Fault> {
    if directory.script_revision() != session.header().script_revision || !directory.is_complete() {
        return Err(super::invalid_phase());
    }
    let policy = catalog.presence_policy().ok_or_else(super::invalid_phase)?;
    if catalog.script_revision() != session.header().script_revision {
        return Err(super::invalid_phase());
    }
    let mut state = State {
        scene_id: policy.initial_scene_id.clone(),
        members: policy.initial.character_ids.iter().cloned().collect(),
        revision: String::new(),
        last_mutation_id: policy.initial.rule_id.clone(),
    };
    state.revision = revision(
        &session.header().session_id,
        0,
        &state.scene_id,
        &state.members,
        &state.last_mutation_id,
    );
    let path = session.path_at(session.history_revision())?;
    for seq in path {
        if before_seq.is_some_and(|before| seq >= before) {
            break;
        }
        let parsed = session.read(seq)?;
        let Some(record) = parsed.body else {
            continue;
        };
        let crate::record::format::Body::System { code, data, .. } = record.body else {
            continue;
        };
        match Fact::decode(&code, &data).map_err(Fault::from)? {
            Fact::SceneAdvanced {
                position,
                mutation_id,
                ..
            } => {
                ScenePresence::validate_scene_advance(catalog, &position)?;
                let mutation = scene_advanced_mutation(&code, &data, &mutation_id);
                if !is_applied(connection, &mutation)? {
                    continue;
                }
                let entry = policy
                    .entry(&position.scene_id)
                    .ok_or_else(super::invalid_phase)?;
                state.scene_id = position.scene_id;
                state.members = entry.character_ids.iter().cloned().collect();
                state.last_mutation_id = mutation_id;
                state.revision = revision(
                    &session.header().session_id,
                    record.branch_seq.unwrap_or(0),
                    &state.scene_id,
                    &state.members,
                    &state.last_mutation_id,
                );
            }
            Fact::SettlementPlanned { mutations, .. } => {
                for mutation in mutations {
                    if mutation.data.kind != "scenePresenceChanged" {
                        continue;
                    }
                    if !is_applied(connection, &mutation)? {
                        continue;
                    }
                    apply_presence_change(
                        &mut state,
                        &mutation,
                        policy,
                        directory,
                        session,
                        catalog,
                        record.branch_seq.unwrap_or(0),
                    )?;
                }
            }
            Fact::SessionEnded { .. } => return Err(super::invalid_phase()),
            _ => {}
        }
    }
    state.revision = revision(
        &session.header().session_id,
        history_revision,
        &state.scene_id,
        &state.members,
        &state.last_mutation_id,
    );
    Ok(state)
}

fn revision(
    session_id: &str,
    history_revision: u64,
    scene_id: &str,
    members: &BTreeSet<String>,
    last_mutation_id: &str,
) -> String {
    let digest = aoidos_json::canonical_string(&serde_json::json!({
        "sessionId": session_id,
        "historyRevision": history_revision,
        "sceneId": scene_id,
        "members": members,
        "lastMutationId": last_mutation_id,
    }))
    .expect("presence revisions contain only JSON-compatible values");
    format::hash(digest.as_bytes())
}

fn is_applied(connection: &rusqlite::Connection, mutation: &WorldMutation) -> Result<bool, Fault> {
    let hash = super::super::record::world::mutation_hash(mutation)?;
    aoidos_store::applied::contains(connection, &mutation.mutation_id, &hash).map_err(Fault::from)
}

fn scene_advanced_mutation(
    code: &str,
    data: &serde_json::Value,
    mutation_id: &str,
) -> WorldMutation {
    WorldMutation {
        mutation_id: mutation_id.to_owned(),
        data: MutationData {
            version: 1,
            kind: code.to_owned(),
            payload: data.clone(),
        },
    }
}

fn apply_presence_change(
    state: &mut State,
    mutation: &WorldMutation,
    policy: &PresencePolicy,
    directory: &EntityDirectory,
    session: &Session,
    catalog: &SceneCatalog,
    branch_revision: u64,
) -> Result<(), Fault> {
    let change: PresenceChange =
        aoidos_json::from_value(mutation.data.payload.clone()).map_err(|_| Fault::bad_request())?;
    let rule = policy
        .change_rule(&change.rule_id)
        .ok_or_else(super::invalid_phase)?;
    let entity = directory
        .entity(&change.entity_id)?
        .filter(|entity| entity.kind == EntityKind::Character)
        .ok_or_else(super::invalid_phase)?;
    if mutation.data.version != 1
        || change.version != 1
        || change.session_id != session.header().session_id
        || change.script_revision != session.header().script_revision
        || change.catalog_revision != catalog.revision()
        || change.scene_id != state.scene_id
        || change.scene_id != rule.scene_id
        || change.rule_version != rule.rule_version
        || change.expected_presence_revision
            != revision(
                &session.header().session_id,
                branch_revision,
                &state.scene_id,
                &state.members,
                &state.last_mutation_id,
            )
        || entity.scope != rule.entity_scope
        || (change.action == PresenceAction::Enter && !rule.allow_enter)
        || (change.action == PresenceAction::Leave && !rule.allow_leave)
        || (change.action == PresenceAction::Enter
            && !state.members.insert(change.entity_id.clone()))
        || (change.action == PresenceAction::Leave && !state.members.remove(&change.entity_id))
        || state.members.len() > MAX_CHARACTERS
    {
        return Err(super::invalid_phase());
    }
    state.last_mutation_id = mutation.mutation_id.clone();
    state.revision = revision(
        &session.header().session_id,
        branch_revision,
        &state.scene_id,
        &state.members,
        &state.last_mutation_id,
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PresenceAction {
    Enter,
    Leave,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresenceChange {
    pub version: u32,
    pub session_id: String,
    pub script_revision: String,
    pub catalog_revision: String,
    pub entity_id: String,
    pub scene_id: String,
    pub action: PresenceAction,
    pub expected_presence_revision: String,
    pub rule_id: String,
    pub rule_version: u32,
}
