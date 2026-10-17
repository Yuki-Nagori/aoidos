//! 周目刻痕的版本化载荷与稳定身份；不赋予提交者规则授权。

use super::{Hash, MAX_INTEGER, SourceRef, canonical, hash, reject, valid_name, valid_script};
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod outbox;
pub use outbox::*;

pub const MAX_CYCLE_SUMMARY_BYTES: usize = 512;
pub const MAX_CYCLE_SOURCES: usize = 128;
pub const MAX_CYCLE_SUBJECTS: usize = 16;
pub const ACTIVE_CYCLE_CAPACITY: usize = 64;
pub const ABSOLUTE_CYCLE_CAPACITY: usize = 256;
pub const MAX_CYCLE_HINTS_PER_SCENE: u8 = 3;
pub const CYCLE_HINT_COOLDOWN_ROUNDS: u64 = 5;
pub const NODE_HINT_AFTER_STAGNANT_ROUNDS: u64 = 8;
pub const MAX_SCENE_PROGRESS_EVIDENCE: usize = 4_096;
pub const MAX_SCENE_RULE_EVIDENCE: usize = 32;
pub const MAX_SCENE_EVIDENCE_BYTES: usize = 8 * 1024;
pub const MAX_PROJECTED_CYCLE_MARKS: usize = 8;
pub const MAX_PROJECTED_CYCLE_SUMMARY_BYTES: usize = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CycleKind {
    Familiarity,
    Affinity,
    Wariness,
    Fragment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DetailPolicy {
    Vague,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CycleMarkStatus {
    Active,
    Archived,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCycleMark {
    pub mark: CycleMarkV1,
    pub status: CycleMarkStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneRuleEvidence {
    pub rule_id: String,
    pub evidence_hash: Hash,
    pub evidence_json: Vec<u8>,
}

impl SceneRuleEvidence {
    /// 校验规则证据身份、大小、摘要与规范 JSON 编码。
    /// # Errors
    /// 字段不合法、摘要不匹配或 JSON 非规范编码时拒绝。
    pub(crate) fn validate(&self) -> Result<()> {
        if !valid_name(&self.rule_id)
            || self.evidence_json.is_empty()
            || self.evidence_json.len() > MAX_SCENE_EVIDENCE_BYTES
            || hash(&self.evidence_json) != self.evidence_hash
            || !is_canonical_scene_evidence_json(&self.evidence_json)
        {
            return Err(reject());
        }
        Ok(())
    }
}

pub(crate) fn is_canonical_scene_evidence_json(bytes: &[u8]) -> bool {
    let Ok(value): std::result::Result<serde_json::Value, _> =
        aoidos_json::decode_bytes(bytes, MAX_SCENE_EVIDENCE_BYTES)
    else {
        return false;
    };
    aoidos_json::canonical_string(&value).is_ok_and(|text| text.as_bytes() == bytes)
}

/// 同一场景在某一有效历史分支中的一次进入实例。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneNodeKey<'a> {
    pub scene_id: &'a str,
    pub history_revision: u64,
    pub entry_record_seq: u64,
}

/// 初始化节点进度所需的可信场景快照。
pub struct BeginSceneProgress<'a> {
    pub run_id: &'a str,
    pub node: SceneNodeKey<'a>,
    pub catalog_revision: &'a str,
    pub baseline: &'a [SceneRuleEvidence],
}

/// 提交一个已由引擎确认完成的场内回合结果。
pub struct CompleteSceneProgress<'a> {
    pub run_id: &'a str,
    pub node: SceneNodeKey<'a>,
    pub catalog_revision: &'a str,
    pub round_id: &'a str,
    pub completed_record_seq: u64,
    pub evidence: &'a [SceneRuleEvidence],
}

/// 原子领取当前节点的一条固定提示。
pub struct ClaimSceneHint<'a> {
    pub run_id: &'a str,
    pub node: SceneNodeKey<'a>,
    pub expected_revision: u64,
    pub hint_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleProjection {
    pub marks: Vec<CycleMarkV1>,
    pub summary_bytes: usize,
}

/// 按固定规则选择有界刻痕投影，不修改已持久化的刻痕。
pub fn select_cycle_projection(marks: &[StoredCycleMark]) -> CycleProjection {
    let mut eligible = marks
        .iter()
        .filter(|stored| stored.status == CycleMarkStatus::Active)
        .map(|stored| &stored.mark)
        .collect::<Vec<_>>();
    eligible.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| right.cycle_gen.cmp(&left.cycle_gen))
            .then_with(|| left.mark_id.as_bytes().cmp(right.mark_id.as_bytes()))
    });

    let mut marks = Vec::new();
    let mut summary_bytes = 0_usize;
    for mark in eligible {
        if marks.len() == MAX_PROJECTED_CYCLE_MARKS {
            break;
        }
        let next_bytes = summary_bytes.saturating_add(mark.summary.len());
        if next_bytes > MAX_PROJECTED_CYCLE_SUMMARY_BYTES {
            continue;
        }
        summary_bytes = next_bytes;
        marks.push(mark.clone());
    }
    CycleProjection {
        marks,
        summary_bytes,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneProgress {
    pub run_id: String,
    pub scene_id: String,
    pub catalog_revision: String,
    /// 场景进入所在的有效历史修订，用于拒绝回退分支上的旧状态。
    pub entry_history_revision: u64,
    /// 场景进入记录序号；同一场景再次进入时会获得新的身份。
    pub entry_record_seq: u64,
    pub revision: u64,
    pub progress_count: u64,
    pub stagnation_rounds: u64,
    pub completed_rounds: u64,
    /// 最近一次计入进度的完成记录序号，按真实记录位置单调推进。
    pub last_completed_record_seq: u64,
    pub last_round_id: Option<String>,
    /// 最近一次完成回合依据集合的规范哈希，用于拒绝冲突重放。
    pub last_evidence_hash: Option<Hash>,
    pub hints_shown: u8,
    pub last_hint_round: Option<u64>,
    pub last_hint_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneProgressUpdate {
    pub state: SceneProgress,
    pub made_progress: bool,
    pub newly_observed_rules: Vec<String>,
}

/// 一条经 Rust 规则层授权的周目刻痕。持久化前仍须由引擎核验规则和来源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CycleMarkV1 {
    pub version: u32,
    pub mark_id: String,
    pub script_id: String,
    pub cycle_gen: u64,
    pub rule_id: String,
    pub rule_version: u32,
    pub subject_ids: Vec<String>,
    pub imprint_key: String,
    pub source_refs: Vec<CycleSourceRef>,
    pub kind: CycleKind,
    pub detail_policy: DetailPolicy,
    /// 只允许使用剧本登记的整数优先级；不接受模型自行评分。
    pub priority: u16,
    pub summary: String,
}

/// 记录来源被核验时的有效历史修订，供恢复与审计定位原始分支。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CycleSourceRef {
    pub source: SourceRef,
    pub history_revision: u64,
}

impl CycleMarkV1 {
    /// 身份不含摘要或权重，因此措辞变更不能生成第二条记忆。
    /// # Errors
    /// 身份、载荷、来源和容量超限时拒绝整条刻痕。
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(crate::error::Error::Rejected(
                crate::error::Reason::PolicyUnavailable,
            ));
        }
        if !valid_script(&self.script_id)
            || self.cycle_gen == 0
            || self.cycle_gen > MAX_INTEGER
            || !valid_name(&self.rule_id)
            || self.rule_version == 0
            || self.rule_version as u64 > MAX_INTEGER
            || self.subject_ids.is_empty()
            || self.subject_ids.len() > MAX_CYCLE_SUBJECTS
            || !valid_name(&self.imprint_key)
            || self.source_refs.is_empty()
            || self.source_refs.len() > MAX_CYCLE_SOURCES
            || self.priority > 1_000
            || self.summary.trim().is_empty()
            || self.summary.len() > MAX_CYCLE_SUMMARY_BYTES
            || self.summary.contains('\0')
        {
            return Err(reject());
        }
        if self.mark_id != self.stable_mark_id()? {
            return Err(reject());
        }

        let unique_subjects = self.subject_ids.iter().collect::<BTreeSet<_>>();
        if unique_subjects.len() != self.subject_ids.len()
            || self.subject_ids.iter().any(|id| !valid_name(id))
        {
            return Err(reject());
        }

        let mut unique_sources = BTreeSet::new();
        for source in &self.source_refs {
            source.source.validate()?;
            if source.source.script_id != self.script_id
                || source.history_revision > MAX_INTEGER
                || !unique_sources.insert(canonical(&source.source)?)
            {
                return Err(reject());
            }
        }
        Ok(())
    }

    /// 跨代去重键；每代仍保留独立标记与来源版本。
    /// # Errors
    /// 序列化规范化失败时返回内部错误。
    pub fn identity_hash(&self) -> Result<Hash> {
        let mut subjects = self.subject_ids.clone();
        subjects.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        let identity = (
            self.script_id.as_str(),
            self.rule_id.as_str(),
            &subjects,
            self.imprint_key.as_str(),
        );
        Ok(hash(&canonical(&identity)?))
    }

    /// UUID v8 派生 ID；同一周目代数、规则、对象和 imprintKey 重放得到同一身份。
    /// # Errors
    /// 规范化身份编码失败时返回内部错误。
    pub fn stable_mark_id(&self) -> Result<String> {
        let identity_hash = self.identity_hash()?;
        let digest = hash(&canonical(&(identity_hash, self.cycle_gen))?);
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x80;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Ok(uuid::Uuid::from_bytes(bytes).to_string())
    }
}

#[cfg(test)]
mod tests;
