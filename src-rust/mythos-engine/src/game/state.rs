//! 阶段 IPC 只包含身份、确认修订与恢复位置，不携带正文或世界数据库。

use crate::{
    fault::Fault,
    ports::Outcome,
    record::facts::{DiceMode, ScenePosition},
};
use serde::{Deserialize, Serialize};

/// 五个公开执行阶段；失败 / 取消属于操作结果，不新增阶段枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Idle,
    Generating,
    AwaitingCheck,
    Settling,
    Advancing,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckStatus {
    Waiting,
    Rolling,
}
/// 暂停检查点的下一项缺失工作；已有骰子时 Check 表示只补分档。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckpointStage {
    Check,
    Narration,
    Settle,
    Advance,
}
/// 只引用当前有效路径中已确认的普通事实；不是事件基线或最后物理位置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub source_round_id: String,
    pub through_seq: u64,
    pub stage: CheckpointStage,
}
/// private 子调用省略 turnId；公开 turn 的空快照须先可读，再发布此身份。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InFlight {
    pub operation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub round_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}
/// 只供展示冻结计划与恰一次等待 / 执行状态，不接受客户端骰值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckSummary {
    pub plan_id: String,
    pub status: CheckStatus,
    pub mode: DiceMode,
    pub rule_id: String,
    pub actor_id: String,
    pub expression: String,
    pub modifier_total: i32,
}
/// 接纳与终态共用最近操作摘要；事件可能属于较旧操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationOutcome {
    Accepted,
    Completed,
    Cancelled,
    Failed,
}
impl From<Outcome> for OperationOutcome {
    fn from(value: Outcome) -> Self {
        match value {
            Outcome::Completed => Self::Completed,
            Outcome::Cancelled => Self::Cancelled,
            Outcome::Failed => Self::Failed,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub operation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub round_id: Option<String>,
    pub outcome: OperationOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Fault>,
}
/// 与 PhaseSnapshot 共用整份状态，四事件跨流乱序按修订号合并。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseState {
    pub session_id: String,
    /// 每次打开独立分配；与记录视图的 viewEpoch 分开管理。
    pub state_epoch: String,
    /// 同一 stateEpoch 内跨四事件递增，同批状态可共享修订号。
    pub phase_revision: u64,
    /// 最近已 applied 的 historyFork 物理 seq；根路径为零。
    pub history_revision: u64,
    pub phase: Phase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene: Option<ScenePosition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_flight: Option<InFlight>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckSummary>,
    pub needs_recovery: bool,
    pub resume_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<Checkpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_operation: Option<Operation>,
}
/// 四种事件分别计数，不能以 phaseRevision 替代缺口检查。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sequences {
    pub phase_changed: u64,
    pub scene_advanced: u64,
    pub operation_done: u64,
    pub operation_failed: u64,
}
/// 状态与四条确认基线的原子副本，不包含正文或完整历史。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhaseSnapshot {
    #[serde(flatten)]
    pub state: PhaseState,
    pub seq: Sequences,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum PhaseEvent {
    Changed(PhaseState),
    Scene {
        #[serde(flatten)]
        state: PhaseState,
        #[serde(rename = "previousSceneId", skip_serializing_if = "Option::is_none")]
        previous_scene_id: Option<String>,
    },
    Done {
        #[serde(flatten)]
        state: PhaseState,
        #[serde(rename = "operationId")]
        operation_id: String,
        outcome: Outcome,
    },
    Failed {
        #[serde(flatten)]
        state: PhaseState,
        #[serde(rename = "operationId")]
        operation_id: String,
        code: String,
        message: String,
    },
}
impl PhaseEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Changed(_) => "engine:phase:changed",
            Self::Scene { .. } => "engine:scene:advanced",
            Self::Done { .. } => "engine:operation:done",
            Self::Failed { .. } => "engine:operation:failed",
        }
    }
    pub fn state(&self) -> &PhaseState {
        match self {
            Self::Changed(s)
            | Self::Scene { state: s, .. }
            | Self::Done { state: s, .. }
            | Self::Failed { state: s, .. } => s,
        }
    }
}
/// 载荷预留不能改变确认副本，投递失败不得撤回已提交事实。
pub trait PhaseEvents: Send + Sync {
    /// # Errors
    /// 载荷、序号或准备失败时拒绝该次状态确认。
    fn prepare(&self, event: &PhaseEvent) -> Result<u64, Fault>;
    /// # Errors
    /// 平台失败返回 event-failed；调用方保留已确认快照。
    fn deliver(&self, seq: u64, event: PhaseEvent) -> Result<(), Fault>;
    fn retire(&self, session_id: &str);
}
/// Rust 接纳身份；operationId 不是客户端命令重试幂等键。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedRound {
    pub operation_id: String,
    pub round_id: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedOperation {
    pub operation_id: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedCheck {
    pub round_id: String,
    pub plan_id: String,
    pub accepted: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelledRound {
    pub round_id: String,
    pub outcome: Outcome,
}
