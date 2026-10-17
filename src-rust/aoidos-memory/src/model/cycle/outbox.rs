//! 引擎确认边界产生的待处理轮回事件。

use super::{MAX_SCENE_RULE_EVIDENCE, SceneRuleEvidence};
use crate::{
    error::{Error, Reason, Result},
    model::{MAX_INTEGER, canonical, hash, reject, valid_id, valid_name},
};
use serde::{Deserialize, Serialize};

pub const MAX_CYCLE_EVENT_BYTES: usize = 64 * 1024;
pub const MAX_CYCLE_EVENT_PAGE: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CycleEventKind {
    SceneEntered,
    RoundCompleted,
}

/// 预写的生命周期快照；仅在来源记录提交且通过引擎核验后消费。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CycleLifecycleEvent {
    pub version: u32,
    pub event_id: String,
    pub run_id: String,
    pub session_id: String,
    pub kind: CycleEventKind,
    pub source_record_seq: u64,
    pub history_revision: u64,
    pub scene_id: String,
    pub entry_record_seq: u64,
    pub catalog_revision: String,
    pub world_revision: String,
    pub round_id: Option<String>,
    pub evidence: Vec<SceneRuleEvidence>,
}

impl CycleLifecycleEvent {
    /// 生成稳定事件 ID；相同 run 和来源记录重试时会得到同一 ID。
    pub fn event_id_for(
        run_id: &str,
        kind: CycleEventKind,
        history_revision: u64,
        source_record_seq: u64,
    ) -> String {
        let kind = match kind {
            CycleEventKind::SceneEntered => "sceneEntered",
            CycleEventKind::RoundCompleted => "roundCompleted",
        };
        let mut identity = Vec::with_capacity(run_id.len() + kind.len() + 32);
        identity.extend_from_slice(run_id.as_bytes());
        identity.push(b':');
        identity.extend_from_slice(kind.as_bytes());
        identity.push(b':');
        identity.extend_from_slice(history_revision.to_string().as_bytes());
        identity.push(b':');
        identity.extend_from_slice(source_record_seq.to_string().as_bytes());
        hex(&hash(&identity))
    }

    /// 校验事件并规范化依据顺序，确保序列化载荷可稳定比较。
    ///
    /// # Errors
    /// 身份、事件字段或依据格式不合法时拒绝。
    pub fn normalize(mut self) -> Result<Self> {
        if self.version != 1
            || !valid_id(&self.run_id)
            || !valid_id(&self.session_id)
            || self.event_id
                != Self::event_id_for(
                    &self.run_id,
                    self.kind,
                    self.history_revision,
                    self.source_record_seq,
                )
            || self.source_record_seq > MAX_INTEGER
            || self.history_revision > MAX_INTEGER
            || !valid_name(&self.scene_id)
            || self.entry_record_seq > self.source_record_seq
            || self.catalog_revision.is_empty()
            || self.catalog_revision.len() > 256
            || self.world_revision.is_empty()
            || self.world_revision.len() > 256
            || self.evidence.len() > MAX_SCENE_RULE_EVIDENCE
        {
            return Err(reject());
        }
        match self.kind {
            CycleEventKind::SceneEntered
                if self.round_id.is_some() || self.source_record_seq != self.entry_record_seq =>
            {
                return Err(reject());
            }
            CycleEventKind::RoundCompleted
                if self.round_id.as_deref().is_none_or(|id| !valid_id(id))
                    || self.source_record_seq <= self.entry_record_seq =>
            {
                return Err(reject());
            }
            _ => {}
        }
        for item in &self.evidence {
            item.validate()?;
        }
        self.evidence.sort_by(|left, right| {
            left.rule_id
                .as_bytes()
                .cmp(right.rule_id.as_bytes())
                .then_with(|| left.evidence_hash.cmp(&right.evidence_hash))
        });
        if self.evidence.windows(2).any(|pair| {
            pair[0].rule_id == pair[1].rule_id && pair[0].evidence_hash == pair[1].evidence_hash
        }) {
            return Err(reject());
        }
        Ok(self)
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>> {
        let payload = canonical(self)?;
        if payload.len() > MAX_CYCLE_EVENT_BYTES {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        Ok(payload)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
