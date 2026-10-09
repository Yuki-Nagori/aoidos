//! 持久类型与预算；字段白名单、身份和定点参数先于数据库写入核验。

use crate::error::{Error, Reason, Result, invalid_json};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub type Hash = [u8; 32];
pub const MAX_INTEGER: u64 = 9_007_199_254_740_991;
pub const MAX_VERSION_BYTES: usize = 32 * 1024;
pub const MAX_PLAN_BYTES: usize = 1024 * 1024;
pub const MAX_SCAN: usize = 256;
pub const MAX_SCAN_BYTES: usize = 4 * 1024 * 1024;

/// 身份只由受信 Rust 调用方生成，不接受模型文件名。
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub(crate) fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id)
}
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}
pub fn hash(bytes: &[u8]) -> Hash {
    Sha256::digest(bytes).into()
}
pub(crate) fn canonical<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    aoidos_json::canonical_string(value)
        .map(String::into_bytes)
        .map_err(invalid_json)
}
pub(crate) fn reject() -> Error {
    Error::Rejected(Reason::InvalidSchema)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Algorithm {
    Exponential,
    Segmented,
    None,
}
/// 周目冻结完整参数；开发值不冒充已标定生产默认。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub version: u32,
    pub algorithm_version: u32,
    pub config_version: u64,
    pub app_config_version: u64,
    pub algorithm: Algorithm,
    pub base_strength: u64,
    pub reinforcement_gain: u64,
    pub reinforcement_cap: u64,
    pub strength_cap: u64,
    pub half_life_rounds: u64,
    pub repeat_window_rounds: u64,
    pub repeat_limit: u64,
    pub revival_cooldown_rounds: u64,
    pub active_capacity: u64,
    /// 指数策略保存经版本管理的实际整数率，不在恢复时用浮点重新计算。
    pub rate_table_version: u32,
    pub decay_rate: u64,
}
impl Policy {
    /// # Errors
    /// 未支持版本返回 policyUnavailable；整数范围或跨字段约束不合法拒绝整组。
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 || self.algorithm_version != 1 || self.rate_table_version != 1 {
            return Err(Error::Rejected(Reason::PolicyUnavailable));
        }
        if self.config_version > MAX_INTEGER
            || self.app_config_version > MAX_INTEGER
            || !(50_000..=1_000_000).contains(&self.base_strength)
            || self.reinforcement_gain > 500_000
            || self.reinforcement_cap > 950_000
            || !(100_000..=1_000_000).contains(&self.strength_cap)
            || self.base_strength + self.reinforcement_cap > self.strength_cap
            || !(10..=10_000).contains(&self.half_life_rounds)
            || !(1..=10_000).contains(&self.repeat_window_rounds)
            || !(1..=10).contains(&self.repeat_limit)
            || !(self.repeat_window_rounds..=10_000).contains(&self.revival_cooldown_rounds)
            || !(100..=10_000).contains(&self.active_capacity)
            || !(1..=1_000_000).contains(&self.decay_rate)
        {
            return Err(reject());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InputMode {
    InCharacter,
    OutOfCharacter,
}
/// hash 和区间针对原始正文 UTF-8，不针对 JSON 转义或整行记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRef {
    pub script_id: String,
    pub run_id: String,
    pub session_id: String,
    pub record_seq: u64,
    pub body_hash: Hash,
    pub start_byte: u64,
    pub end_byte: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<InputMode>,
}
impl SourceRef {
    pub(crate) fn validate(&self) -> Result<()> {
        if !valid_script(&self.script_id)
            || !valid_id(&self.run_id)
            || !valid_id(&self.session_id)
            || self.record_seq == 0
            || self.record_seq > MAX_INTEGER
            || self.start_byte >= self.end_byte
            || self.end_byte > MAX_INTEGER
        {
            return Err(reject());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum Subject {
    PlayerPreference,
    Character { id: String },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Dimension {
    Story,
    Relationship,
    PlayerPreference,
    Foreshadowing,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Attitude {
    Unexpressed,
    Believed,
    Doubtful,
    Denied,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KnowledgeKind {
    Observed,
    Told,
    Rumor,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeEvidence {
    pub evidence_id: String,
    pub subject_id: String,
    pub source: SourceRef,
    pub kind: KnowledgeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub informant_id: Option<String>,
    pub scope: String,
    pub rule_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    New,
    Supplement,
    Correction,
    Restore,
}
/// 已发布版本不可改写；恢复以新版本记录 parent 与 restoredFrom。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Version {
    pub version: u32,
    pub entry_id: String,
    pub version_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_version_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored_from_version_id: Option<String>,
    pub subject: Subject,
    pub dimension: Dimension,
    pub summary: String,
    pub attitude: Attitude,
    pub inferred: bool,
    pub change: ChangeKind,
    pub sources: Vec<SourceRef>,
    pub evidence: Vec<KnowledgeEvidence>,
    pub source_set_hash: Hash,
    pub evidence_set_hash: Hash,
    pub policy_hash: Hash,
}
/// 集合规范化 hash 不依赖传入顺序；重复元素由版本校验拒绝。
pub fn set_hash<T: Serialize>(values: &[T]) -> Result<Hash> {
    let mut items = values.iter().map(canonical).collect::<Result<Vec<_>>>()?;
    items.sort();
    let mut digest = Sha256::new();
    digest.update(b"[");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            digest.update(b",");
        }
        digest.update(item);
    }
    digest.update(b"]");
    Ok(digest.finalize().into())
}

impl Version {
    /// # Errors
    /// 未知版本、容量、身份、来源或结构化主体不合法时拒绝。
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(Error::Rejected(Reason::PolicyUnavailable));
        }
        if !valid_id(&self.entry_id)
            || !valid_id(&self.version_id)
            || self
                .parent_version_id
                .as_ref()
                .is_some_and(|id| !valid_id(id) || id == &self.version_id)
            || self
                .restored_from_version_id
                .as_ref()
                .is_some_and(|id| !valid_id(id))
            || (self.change == ChangeKind::Restore) != self.restored_from_version_id.is_some()
            || self.summary.trim().is_empty()
            || self.summary.len() > 4096
            || self.sources.is_empty()
            || self.sources.len() > 128
            || self.evidence.len() > 128
        {
            return Err(reject());
        }
        let unique = self
            .sources
            .iter()
            .map(canonical)
            .collect::<Result<BTreeSet<_>>>()?;
        if unique.len() != self.sources.len()
            || self.source_set_hash != set_hash(&self.sources)?
            || self.evidence_set_hash != set_hash(&self.evidence)?
        {
            return Err(reject());
        }
        for source in &self.sources {
            source.validate()?;
        }
        match &self.subject {
            Subject::PlayerPreference
                if self.dimension == Dimension::PlayerPreference
                    && self.evidence.is_empty()
                    && self.sources.iter().all(|source| source.mode.is_some())
                    && !self.inferred => {}
            Subject::Character { id }
                if valid_name(id)
                    && self.dimension != Dimension::PlayerPreference
                    && !self.evidence.is_empty()
                    && self
                        .sources
                        .iter()
                        .all(|source| source.mode != Some(InputMode::OutOfCharacter)) =>
            {
                let mut seen = BTreeSet::new();
                for evidence in &self.evidence {
                    if !valid_id(&evidence.evidence_id)
                        || evidence.subject_id != *id
                        || !self.sources.contains(&evidence.source)
                        || !seen.insert(&evidence.evidence_id)
                        || !valid_name(&evidence.scope)
                        || !valid_name(&evidence.rule_id)
                        || evidence
                            .informant_id
                            .as_ref()
                            .is_some_and(|id| !valid_name(id))
                        || (evidence.kind == KnowledgeKind::Told && evidence.informant_id.is_none())
                    {
                        return Err(reject());
                    }
                }
            }
            _ => return Err(reject()),
        }
        if canonical(self)?.len() > MAX_VERSION_BYTES {
            return Err(reject());
        }
        Ok(())
    }
}
/// 调用方持有记录锁；状态与来源验证不能来自模型声明或跨锁缓存。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Boundary {
    pub script_id: String,
    pub session_id: String,
    pub history_revision: u64,
    pub entity_revision: u64,
    pub evidence_revision: u64,
}
pub trait CausalPort {
    /// # Errors
    /// pending 世界 / 回退恢复期间拒绝获得可写边界。
    fn boundary(&self) -> Result<Boundary>;
    /// # Errors
    /// 来源无法读取时返回存储错误；无效来源返回 false，不自动搜索替代。
    fn valid_source(&self, source: &SourceRef) -> Result<bool>;
    /// # Errors
    /// 证据必须由可信引擎建立；未知或已失效返回 false。
    fn valid_evidence(&self, evidence: &KnowledgeEvidence) -> Result<bool>;
    /// # Errors
    /// 返回精确区间原文；hash / 模式 / 资格不合法时返回 None。
    fn source_text(&self, source: &SourceRef) -> Result<Option<String>>;
    /// # Errors
    /// 必须匹配引擎已确认的场内完成事实及 roundId，不由任意来源推进时钟。
    fn completed_round(&self, round_id: &str, source: &SourceRef) -> Result<bool>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryState {
    Active,
    Archived,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EffectKind {
    Gain,
    Revival,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Effect {
    pub effect_id: String,
    pub kind: EffectKind,
    pub source: SourceRef,
    pub logical_position: u64,
    pub gain: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Expected {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    pub state_revision: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub entry_id: String,
    pub expected: Expected,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<Version>,
    pub state: EntryState,
    pub effects: Vec<Effect>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Plan {
    pub expected_logical_clock: u64,
    pub operation_id: String,
    pub run_id: String,
    pub history_revision: u64,
    pub entity_revision: u64,
    pub evidence_revision: u64,
    pub policy_hash: Hash,
    pub targets: Vec<Target>,
    /// 原始素材的稳定身份；不使用 batch 内短引用 id 作为幂等身份。
    pub processed_materials: Vec<String>,
    pub reason: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationState {
    Prepared,
    Ready,
    Applied,
    Rejected,
    Quarantined,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub entry_id: String,
    pub current_version_id: String,
    pub state_revision: u64,
    pub state: EntryState,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub run_id: String,
    pub script_id: String,
    pub session_id: String,
    pub policy: Policy,
    pub policy_hash: Hash,
}

#[cfg(test)]
mod tests;

pub(crate) fn valid_script(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && aoidos_store::paths::script_id_from_name(id) == id
}
