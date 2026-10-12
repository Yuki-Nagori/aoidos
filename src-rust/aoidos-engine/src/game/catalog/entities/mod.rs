//! 完整、版本化的规范实体目录；名称解析只允许精确命中，不推断近似名称。

use crate::{fault::Fault, record::format};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

const MAX_ENTITIES: usize = 4096;
const MAX_NAME_BYTES: usize = 256;
const MAX_SCOPE_BYTES: usize = 128;
const MAX_DIRECTORY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntityKind {
    Character,
    Place,
    Organization,
    Item,
    Term,
}

/// 一个由剧本登记的规范实体；名称仅用于展示和精确查找，不承担实体身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalEntity {
    pub id: String,
    pub kind: EntityKind,
    /// 类别内的解析范围，例如所属地区；全局范围用固定值 `global`。
    pub scope: String,
    pub canonical_name: String,
}

/// 与剧本修订绑定的不可变规范目录快照。
pub struct EntityDirectory {
    script_revision: String,
    revision: u64,
    complete: bool,
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

    pub(super) fn contains_character(&self, id: &str) -> bool {
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

fn valid_label(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty()
        && value.trim() == value
        && value.len() <= max_bytes
        && value.chars().all(|character| !character.is_control())
}
