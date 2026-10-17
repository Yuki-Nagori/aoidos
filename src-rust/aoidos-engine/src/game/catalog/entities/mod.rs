//! 完整、版本化的规范实体目录；名称解析只允许精确命中，不推断近似名称。

use crate::{
    fault::Fault,
    record::{format, session::Session},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod registry;
#[cfg(test)]
mod tests;
pub use registry::{
    EntityCreationCommit, EntityCreationIntent, EntityRegistry,
    MIGRATION as ENTITY_REGISTRY_MIGRATION, verify_schema as verify_entity_registry_schema,
};

const MAX_ENTITIES: usize = 4096;
const MAX_NAME_BYTES: usize = 256;
pub(super) const MAX_SCOPE_BYTES: usize = 128;
const MAX_DIRECTORY_BYTES: usize = 1024 * 1024;
const MAX_DIRECTORY_REVISION: u64 = (1 << 53) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntityKind {
    Character,
    Place,
    Organization,
    Item,
    Term,
}

/// 一条由可信场景目录登记的动态身份创建许可；规则变化须提升 catalog revision。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityCreationRule {
    pub(crate) rule_id: String,
    pub(crate) kind: EntityKind,
    pub(crate) scope: String,
    pub(crate) max_instances: usize,
}

impl EntityCreationRule {
    /// # Errors
    /// 规则身份、范围或每规则数量超出静态硬上限时拒绝。
    pub fn new(
        rule_id: String,
        kind: EntityKind,
        scope: String,
        max_instances: usize,
    ) -> Result<Self, Fault> {
        if !format::valid_id(&rule_id)
            || !valid_label(&scope, MAX_SCOPE_BYTES)
            || max_instances == 0
            || max_instances > MAX_ENTITIES
        {
            return Err(Fault::bad_request());
        }
        Ok(Self {
            rule_id,
            kind,
            scope,
            max_instances,
        })
    }
}

/// 一个由剧本登记的规范实体；名称仅用于展示和精确查找，不承担实体身份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalEntity {
    pub id: String,
    pub kind: EntityKind,
    /// 类别内的解析范围，例如所属地区；全局范围用固定值 `global`。
    pub scope: String,
    pub canonical_name: String,
}

/// 与静态剧本版本和单调实体修订绑定的不可变目录快照。
#[derive(Debug, Clone)]
pub struct EntityDirectory {
    script_revision: String,
    revision: u64,
    complete: bool,
    snapshot_session_id: Option<String>,
    snapshot_history_revision: Option<u64>,
    by_id: BTreeMap<String, CanonicalEntity>,
    by_name: BTreeMap<(EntityKind, String, String), String>,
}

impl EntityDirectory {
    /// # Errors
    /// 重复身份 / 同范围规范名、非法字段、过期修订或目录超限时拒绝整份快照。
    pub fn new(
        script_revision: String,
        revision: u64,
        complete: bool,
        entries: Vec<CanonicalEntity>,
    ) -> Result<Self, Fault> {
        if !format::valid_hash(&script_revision)
            || revision == 0
            || revision > MAX_DIRECTORY_REVISION
            || entries.is_empty()
            || entries.len() > MAX_ENTITIES
        {
            return Err(Fault::bad_request());
        }
        let mut by_id = BTreeMap::new();
        let mut by_name = BTreeMap::new();
        for entity in entries {
            if !format::valid_id(&entity.id)
                || !valid_label(&entity.scope, MAX_SCOPE_BYTES)
                || !valid_label(&entity.canonical_name, MAX_NAME_BYTES)
                || by_id.contains_key(&entity.id)
            {
                return Err(Fault::bad_request());
            }
            let name_key = (
                entity.kind,
                entity.scope.clone(),
                entity.canonical_name.clone(),
            );
            if by_name.insert(name_key, entity.id.clone()).is_some() {
                return Err(Fault::bad_request());
            }
            by_id.insert(entity.id.clone(), entity);
        }
        let directory = Self {
            script_revision,
            revision,
            complete,
            snapshot_session_id: None,
            snapshot_history_revision: None,
            by_id,
            by_name,
        };
        if directory.encoded_len()? > MAX_DIRECTORY_BYTES {
            return Err(Fault::bad_request());
        }
        Ok(directory)
    }

    pub fn script_revision(&self) -> &str {
        &self.script_revision
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// 标记由注册表按当前 session 与有效历史读取的完整目录快照。
    /// # Errors
    /// session 身份非法或历史修订超出安全整数范围时拒绝绑定。
    pub(super) fn bind_session_snapshot(mut self, session: &Session) -> Result<Self, Fault> {
        let session_id = &session.header().session_id;
        let history_revision = session.history_revision();
        if session.needs_recovery()
            || !format::valid_uuid(session_id)
            || history_revision > MAX_DIRECTORY_REVISION
            || !self.complete
            || self.script_revision != session.header().script_revision
        {
            return Err(Fault::bad_request());
        }
        self.snapshot_session_id = Some(session_id.clone());
        self.snapshot_history_revision = Some(history_revision);
        Ok(self)
    }

    pub(super) fn is_session_snapshot_for(&self, session: &Session) -> bool {
        self.snapshot_session_id.as_deref() == Some(session.header().session_id.as_str())
            && self.snapshot_history_revision == Some(session.history_revision())
    }

    /// 合并已验证的动态身份，保留静态 completeness 声明并使用调用方的单调修订。
    /// # Errors
    /// 动态身份冲突、修订回退或合并后超出硬上限时拒绝。
    pub(super) fn with_dynamic(
        &self,
        revision: u64,
        dynamic: Vec<CanonicalEntity>,
    ) -> Result<Self, Fault> {
        if revision < self.revision {
            return Err(super::super::invalid_phase());
        }
        let mut entries = self.by_id.values().cloned().collect::<Vec<_>>();
        entries.extend(dynamic);
        Self::new(
            self.script_revision.clone(),
            revision,
            self.complete,
            entries,
        )
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = &CanonicalEntity> {
        self.by_id.values()
    }

    /// 完整目录内的精确规范名命中；未找到返回 `None`，近似匹配不在此层执行。
    /// # Errors
    /// 不完整快照拒绝解析，避免把局部缓存中的唯一命中误认为全局唯一。
    pub fn resolve_exact(
        &self,
        kind: EntityKind,
        scope: &str,
        name: &str,
    ) -> Result<Option<&CanonicalEntity>, Fault> {
        if !self.complete {
            return Err(super::super::invalid_phase());
        }
        if !valid_label(scope, MAX_SCOPE_BYTES) || !valid_label(name, MAX_NAME_BYTES) {
            return Err(Fault::bad_request());
        }
        Ok(self
            .by_name
            .get(&(kind, scope.to_owned(), name.to_owned()))
            .and_then(|id| self.by_id.get(id)))
    }

    /// 完整目录内按稳定 ID 取实体；局部快照不能作为权威上下文。
    /// # Errors
    /// 不完整目录拒绝返回实体身份信息。
    pub fn entity(&self, id: &str) -> Result<Option<&CanonicalEntity>, Fault> {
        if !self.complete {
            return Err(super::super::invalid_phase());
        }
        Ok(self.by_id.get(id))
    }

    pub(crate) fn has_character_scope(&self, scope: &str) -> bool {
        self.by_id
            .values()
            .any(|entity| entity.kind == EntityKind::Character && entity.scope == scope)
    }

    pub(crate) fn contains_character(&self, id: &str) -> bool {
        self.by_id
            .get(id)
            .is_some_and(|entity| entity.kind == EntityKind::Character)
    }

    fn encoded_len(&self) -> Result<usize, Fault> {
        let mut bytes = self.script_revision.len() + 32;
        for entity in self.by_id.values() {
            bytes = bytes
                .checked_add(entity.id.len())
                .and_then(|size| size.checked_add(entity.scope.len()))
                .and_then(|size| size.checked_add(entity.canonical_name.len()))
                .and_then(|size| size.checked_add(16))
                .ok_or_else(Fault::bad_request)?;
            if bytes > MAX_DIRECTORY_BYTES {
                return Ok(bytes);
            }
        }
        Ok(bytes)
    }
}

pub(super) fn valid_label(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty()
        && value.trim() == value
        && value.len() <= max_bytes
        && value.chars().all(|character| !character.is_control())
}
