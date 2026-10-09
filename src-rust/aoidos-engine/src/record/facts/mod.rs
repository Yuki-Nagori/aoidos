//! 012 已登记因果事实的数据类型；这里只验证 / 保存，不执行阶段或 RNG。

use super::format::{self, InputMode, Modifier};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiceMode {
    Manual,
    Auto,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundIdentity {
    pub version: u32,
    pub round_id: String,
    pub operation_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GenerationTarget {
    Narration,
    CharacterSpeech { speaker_id: String },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultPolicy {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dc: Option<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckPlan {
    pub plan_id: String,
    pub rule_id: String,
    pub rule_version: u32,
    pub actor_id: String,
    pub expression: String,
    pub modifiers: Vec<Modifier>,
    pub result_policy: ResultPolicy,
    pub success_branch_id: String,
    pub costly_success_branch_id: String,
    pub failure_branch_id: String,
    pub world_revision: String,
    pub plan_hash: String,
}
/// 属性 schema 未设计，版本化载荷只能交给显式登记的解释器，不解析模型 SQL。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldMutation {
    pub mutation_id: String,
    pub data: MutationData,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationData {
    pub version: u32,
    pub kind: String,
    pub payload: Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenePosition {
    pub scene_id: String,
    pub path: Vec<SceneNode>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneNode {
    pub kind: String,
    pub id: String,
    pub title: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    Proposal,
    Check,
    Narration,
    Settlement,
    Advance,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SkipReason {
    Disabled,
    NoCheck,
    OutOfCharacter,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForkMode {
    Rewind,
    Regenerate,
}
/// 普通事实严格使用登记数据；未知 code / version 仍由原行兼容通道只读保存。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "code",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Fact {
    RoundAccepted {
        #[serde(flatten)]
        identity: RoundIdentity,
        input_seq: u64,
        mode: InputMode,
        dice_mode: DiceMode,
        profile_id: String,
        profile_revision: String,
        target: GenerationTarget,
        scene_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_round_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        checkpoint_seq: Option<u64>,
    },
    CheckPlanned {
        #[serde(flatten)]
        identity: RoundIdentity,
        dice_mode: DiceMode,
        plan: CheckPlan,
    },
    CheckSkipped {
        #[serde(flatten)]
        identity: RoundIdentity,
        reason: SkipReason,
        world_revision: String,
    },
    SettlementPlanned {
        #[serde(flatten)]
        identity: RoundIdentity,
        narrative_seq: u64,
        world_revision: String,
        mutations: Vec<WorldMutation>,
    },
    RoundSettled {
        #[serde(flatten)]
        identity: RoundIdentity,
        narrative_seq: u64,
        settlement_seq: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        applied_through_mutation_id: Option<String>,
    },
    SceneAdvanced {
        #[serde(flatten)]
        identity: RoundIdentity,
        previous_scene_id: String,
        position: ScenePosition,
        world_revision: String,
        mutation_id: String,
    },
    SceneStayed {
        #[serde(flatten)]
        identity: RoundIdentity,
        scene_id: String,
        world_revision: String,
    },
    SessionEnded {
        #[serde(flatten)]
        identity: RoundIdentity,
        previous_scene_id: String,
        reason_id: String,
        world_revision: String,
        mutation_id: String,
    },
    RoundEnded {
        #[serde(flatten)]
        identity: RoundIdentity,
        outcome: crate::ports::Outcome,
        through_seq: u64,
        completed_steps: Vec<Step>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<crate::fault::Fault>,
    },
    AbandonCheckpoint {
        #[serde(flatten)]
        identity: RoundIdentity,
        source_round_id: String,
        checkpoint_seq: u64,
    },
    HistoryFork {
        version: u32,
        operation_id: String,
        mode: ForkMode,
        parent_control_seq: u64,
        target_seq: u64,
        prefix_hash: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_round_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reused_dice_seq: Option<u64>,
    },
    Orphan {
        version: u32,
        recovery_id: String,
    },
}
impl Fact {
    /// 统一生成已登记 system 块；消息只用于展示 / 上下文，因果执行始终读取 data。
    /// # Errors
    /// 序列化失败返回 corrupt；版本与字段引用继续由 Session 追加边界核验。
    pub fn body(&self, message: &str) -> aoidos_store::error::Result<format::Body> {
        let value = aoidos_json::to_value(self).map_err(decode_error)?;
        Ok(format::Body::System {
            code: value["code"].as_str().ok_or_else(format::corrupt)?.into(),
            message: message.into(),
            related_seq: None,
            turn_id: None,
            data: value["data"].clone(),
        })
    }
    /// # Errors
    /// 登记数据缺失或非法身份拒绝，不把未知版本解释为当前语义。
    pub fn decode(code: &str, data: &Value) -> aoidos_store::error::Result<Self> {
        let fact: Self = aoidos_json::from_value(serde_json::json!({"code":code,"data":data}))
            .map_err(decode_error)?;
        if data["version"] != 1 {
            return Err(format::corrupt());
        }
        if code != "orphan" {
            if !data["operationId"].as_str().is_some_and(format::valid_uuid) {
                return Err(format::corrupt());
            }
            if code != "historyFork" && !data["roundId"].as_str().is_some_and(format::valid_uuid) {
                return Err(format::corrupt());
            }
        }
        match &fact {
            Self::Orphan { recovery_id, .. } => {
                if !format::valid_hash(recovery_id) {
                    return Err(format::corrupt());
                }
            }
            Self::HistoryFork {
                parent_control_seq,
                target_seq,
                prefix_hash,
                ..
            } => {
                if *parent_control_seq > format::MAX_SEQ
                    || *target_seq > format::MAX_SEQ
                    || !format::valid_hash(prefix_hash)
                {
                    return Err(format::corrupt());
                }
            }
            Self::SettlementPlanned { mutations, .. } => {
                let mut ids = std::collections::HashSet::new();
                if mutations.len() > 32
                    || mutations.iter().any(|m| {
                        !format::valid_uuid(&m.mutation_id)
                            || m.data.version == 0
                            || !format::valid_id(&m.data.kind)
                            || !ids.insert(&m.mutation_id)
                    })
                {
                    return Err(format::corrupt());
                }
            }
            Self::CheckPlanned { plan, .. } => {
                if !format::valid_id(&plan.plan_id)
                    || !format::valid_id(&plan.rule_id)
                    || !format::valid_id(&plan.actor_id)
                    || plan.rule_version == 0
                    || !format::valid_hash(&plan.plan_hash)
                    || plan.modifiers.len() > 32
                    || plan.result_policy.dc.is_some_and(|dc| !dc.is_finite())
                {
                    return Err(format::corrupt());
                }
            }
            Self::RoundEnded { outcome, error, .. } => {
                if (*outcome == crate::ports::Outcome::Failed) != error.is_some() {
                    return Err(format::corrupt());
                }
            }
            Self::RoundAccepted {
                source_round_id,
                checkpoint_seq,
                ..
            } if source_round_id.is_some() != checkpoint_seq.is_some() => {
                return Err(format::corrupt());
            }
            _ => {}
        }
        Ok(fact)
    }
}
fn decode_error(_: aoidos_json::Error) -> aoidos_store::error::StoreError {
    format::corrupt()
}

#[cfg(test)]
mod tests;
