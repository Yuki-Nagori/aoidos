//! 周目内经明确确认的实体别名；名称索引不能创建或合并权威实体。

use super::SourceRef;
use serde::{Deserialize, Serialize};

/// 与权威目录共享的实体分类标签，不承担身份登记职责。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntityCategory {
    Character,
    Place,
    Organization,
    Item,
    Term,
}
impl EntityCategory {
    pub(crate) fn as_db(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Place => "place",
            Self::Organization => "organization",
            Self::Item => "item",
            Self::Term => "term",
        }
    }
    pub(crate) fn from_db(value: &str) -> Option<Self> {
        Some(match value {
            "character" => Self::Character,
            "place" => Self::Place,
            "organization" => Self::Organization,
            "item" => Self::Item,
            "term" => Self::Term,
            _ => return None,
        })
    }
}

/// 由可信引擎目录返回的别名校验上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityAliasContext {
    pub entity_id: String,
    pub category: EntityCategory,
    pub scope: String,
    pub canonical_name: String,
}

/// 当前周目中可由精确别名命中的绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityAlias {
    pub alias_id: String,
    pub run_id: String,
    pub entity_id: String,
    pub category: EntityCategory,
    pub scope: String,
    /// 保留玩家原始写法；v1 仅按精确字符串查找，不做模糊合并。
    pub name: String,
    pub status: AliasStatus,
    pub revision: u64,
    pub registered_entity_revision: u64,
    pub sources: Vec<SourceRef>,
}

/// 显式登记与撤销都保留绑定身份，撤销不会删除历史来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AliasStatus {
    Active,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AliasRevocation {
    pub alias_id: String,
    pub expected_revision: u64,
    pub expected_entity_revision: u64,
}

#[cfg(test)]
mod tests;
