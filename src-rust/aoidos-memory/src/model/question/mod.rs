//! 待确认问题的稳定身份、候选集合与显式答案；不创建权威实体或修改认知正文。

use super::{Hash, SourceRef};
use serde::{Deserialize, Serialize};

/// 玩家需要显式确认的问题类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuestionKind {
    EntityAssociation,
    MemoryClassification,
}

/// 可展示给玩家的类型化候选；仓储保存目录快照引用，但不负责登记 canonical 实体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum QuestionCandidate {
    /// 来自周目绑定目录的实体。
    Entity {
        /// 稳定实体 ID，不是显示名称。
        entity_id: String,
    },
    /// 记忆分类候选，不携带正文或执行指令。
    MemoryClassification {
        /// 已登记的候选分类。
        classification: super::Classification,
    },
}

/// `event_slot` 由可信管线按原始素材位置分配，不来自模型候选；候选变化不会改变问题身份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionIdentity {
    /// 候选歧义所属类别。
    pub kind: QuestionKind,
    /// 同一原始来源中的稳定歧义序号。
    pub event_slot: u32,
    /// 提出歧义的完整有效来源集合。
    pub sources: Vec<SourceRef>,
}

/// 持久待确认问题的不可变版本输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionDraft {
    /// 持久 payload 版本。
    pub version: u32,
    /// 去重身份；不包含问题文案或模型生成的 ID。
    pub identity: QuestionIdentity,
    /// 创建候选时确认的世界历史版本。
    pub history_revision: u64,
    /// 创建候选时确认的实体目录版本。
    pub entity_revision: u64,
    /// 创建候选时确认的获知证据版本。
    pub evidence_revision: u64,
    /// 展示给玩家的有限选项。
    pub candidates: Vec<QuestionCandidate>,
}

/// 玩家对待确认问题作出的答案。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum QuestionAnswer {
    /// 选择当前问题明确提供的候选。
    SelectCandidate {
        /// 必须与问题当前候选完全相等。
        candidate: QuestionCandidate,
    },
    /// 建议一个尚未登记的新实体名称；不直接创建权威实体。
    ProposeEntity {
        /// 玩家提出的显示名称，仅作为提议持久保存。
        display_name: String,
    },
    /// 明确表示以上候选都不符合。
    None,
    /// 跳过问题，保留未提交的相关记忆处理。
    Skip,
}

/// 带 CAS 前置条件的回答请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionResolution {
    /// 要回答的问题身份。
    pub question_id: String,
    /// 读取问题时观察到的版本。
    pub expected_revision: u64,
    /// 读取问题时观察到的历史版本。
    pub expected_history_revision: u64,
    /// 玩家选择；仓储只保存答案，不直接应用记忆或实体变更。
    pub answer: QuestionAnswer,
}

/// 问题在待办队列中的持久状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuestionStatus {
    /// 可供玩家回答。
    Pending,
    /// 已记录玩家答案。
    Resolved,
    /// 原始来源或可信状态已失效。
    Invalidated,
}

/// 从持久仓储读取的问题快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryQuestion {
    /// 稳定 UUID。
    pub question_id: String,
    /// 所属周目。
    pub run_id: String,
    /// 按周目和稳定事件身份计算的去重键。
    pub question_key: Hash,
    /// 候选集合或状态改变时递增的 CAS 版本。
    pub revision: u64,
    /// 候选、来源及可信版本快照。
    pub draft: QuestionDraft,
    /// 持久生命周期状态。
    pub status: QuestionStatus,
    /// 已提交答案；待确认或失效问题没有答案。
    pub answer: Option<QuestionAnswer>,
}
