//! 随应用发布的默认资源；编译时嵌入，运行时只按身份选择。

use crate::fault::Fault;
use std::collections::BTreeMap;
struct RawScript {
    id: &'static str,
    body: &'static [u8],
    i18n: &'static str,
    entities: &'static [u8],
    cycle_input: &'static [u8],
    legacy_cycle_input: &'static [u8],
    notices: &'static [&'static str],
}
// 默认内容属于发布产物，固定路径只用于编译取材，不是运行时文件位置。
const BUNDLED: &[RawScript] = &[RawScript {
    id: "mistbell",
    body: include_bytes!("../../../../../resources/scripts/mistbell/Mistbell.md"),
    i18n: include_str!("../../../../../resources/scripts/mistbell/i18n.json"),
    entities: include_bytes!("../../../../../resources/scripts/mistbell/entities.json"),
    cycle_input: include_bytes!("../../../../../resources/scripts/mistbell/cycle-input-v2.json"),
    legacy_cycle_input: include_bytes!(
        "../../../../../resources/scripts/mistbell/cycle-input-v1.json"
    ),
    notices: &[
        include_str!("../../../../../resources/scripts/mistbell/LICENSE.md"),
        include_str!("../../../../../resources/scripts/mistbell/SRD-5.2.1.md"),
    ],
}];
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptInfo {
    pub script_id: String,
    pub title: String,
    pub display_names: BTreeMap<String, String>,
    pub suggested_actions: BTreeMap<String, String>,
    pub attributions: String,
}
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScriptI18n {
    version: u32,
    display_names: BTreeMap<String, String>,
    suggested_actions: BTreeMap<String, String>,
}
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EntityManifest {
    version: u32,
    revision: u64,
    complete: bool,
    entities: Vec<EntityEntry>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CycleInputManifest {
    pub(crate) version: u32,
    pub(crate) revision: u64,
    #[serde(default)]
    pub(crate) story_character_ids: Vec<String>,
    #[serde(default)]
    pub(crate) protagonist_character_id: Option<String>,
    #[serde(default)]
    pub(crate) characters: Vec<CharacterProfile>,
    pub(crate) state_fields: Vec<StateField>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CharacterProfile {
    pub(crate) character_id: String,
    #[serde(default)]
    pub(crate) is_guide: bool,
    pub(crate) truth: CharacterTruth,
    pub(crate) initial_knowledge: Vec<String>,
    pub(crate) presentation: CharacterPresentation,
    #[serde(default)]
    pub(crate) opening_role: Option<String>,
    #[serde(default)]
    pub(crate) opening_relationship: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CharacterTruth {
    pub(crate) personal_name: String,
    pub(crate) background: String,
    pub(crate) identity: String,
    pub(crate) race: String,
    pub(crate) age: String,
    pub(crate) gender: String,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CharacterPresentation {
    pub(crate) display_name: String,
    pub(crate) identity: String,
    pub(crate) race: String,
    pub(crate) age: String,
    pub(crate) gender: String,
    pub(crate) appearance: String,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StateField {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) source: String,
    pub(crate) min: i64,
    pub(crate) max: i64,
    pub(crate) operators: Vec<String>,
    pub(crate) descriptions: BTreeMap<String, String>,
}
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EntityEntry {
    id: String,
    kind: EntityKind,
    scope: String,
    canonical_name: String,
}
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum EntityKind {
    Character,
    Place,
    Organization,
    Item,
    Term,
}
const MAX_ENTITY_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_CYCLE_INPUT_BYTES: usize = 128 * 1024;
/// # Errors
/// 未登记身份为 not-found，内嵌原文不合法为 not-ready。
pub fn scenario(id: &str) -> Result<aoidos_script::Scenario, Fault> {
    let raw = find(id)?;
    aoidos_script::decode(raw.id, raw.body).map_err(invalid_asset)
}
/// 返回绑定原文、实体清单和周目输入配置的发布版本。
/// # Errors
/// 未登记身份为 not-found；损坏或不支持的实体清单为 not-ready。
pub fn revision(id: &str) -> Result<String, Fault> {
    let raw = find(id)?;
    Ok(entity_directory_from_raw(raw)?.script_revision().to_owned())
}
/// 返回校验后的可信状态字段和角色资料 / 引导者标记，供开局初始化构造使用。
/// # Errors
/// 未登记剧本或损坏 / 未注册的状态源、角色引用拒绝返回。
pub(crate) fn cycle_input(id: &str) -> Result<CycleInputManifest, Fault> {
    let raw = find(id)?;
    let entities = decode_entity_manifest(raw.entities)?;
    let input = decode_cycle_input(raw.cycle_input, &entities)?;
    let directory = entity_directory_from_raw(raw)?;
    validate_character_profiles(&input, &directory)?;
    Ok(input)
}
/// Captures the exact resources a new initialization must later validate against.
#[cfg(test)]
pub(crate) struct InitializationAssetSnapshots {
    pub(crate) script: Vec<u8>,
    pub(crate) entities: Vec<u8>,
    pub(crate) cycle_input: Vec<u8>,
}
#[cfg(test)]
pub(crate) fn initialization_asset_snapshots(
    id: &str,
    expected_revision: &str,
) -> Result<InitializationAssetSnapshots, Fault> {
    let raw = find(id)?;
    let directory = entity_directory_from_raw(raw)?;
    if directory.script_revision() != expected_revision {
        return Err(invalid_cycle_input());
    }
    validate_frozen_initialization_assets(
        id,
        expected_revision,
        raw.body,
        raw.entities,
        raw.cycle_input,
    )?;
    Ok(InitializationAssetSnapshots {
        script: raw.body.to_vec(),
        entities: raw.entities.to_vec(),
        cycle_input: raw.cycle_input.to_vec(),
    })
}
/// 测试读取与指定剧本配套的原始实体清单字节。
#[cfg(test)]
pub(crate) fn entity_manifest_snapshot(id: &str) -> Result<Vec<u8>, Fault> {
    Ok(find(id)?.entities.to_vec())
}
/// 以新剧本版本或历史 v1 版本重建实体目录，确保旧周目不被新输入资源改写。
/// # Errors
/// 目标版本不是该已登记剧本的当前或 legacy 修订时拒绝。
pub(crate) fn entity_directory_for_revision(
    id: &str,
    expected_revision: &str,
) -> Result<super::catalog::EntityDirectory, Fault> {
    let raw = find(id)?;
    let manifest = decode_entity_manifest(raw.entities)?;
    let legacy_revision = legacy_script_revision(raw.body, raw.entities, manifest.revision);
    let current_revision = revision(id)?;
    let previous_input: CycleInputManifest =
        aoidos_json::decode_bytes(raw.legacy_cycle_input, MAX_CYCLE_INPUT_BYTES)
            .map_err(|_| invalid_cycle_input())?;
    let previous_revision = script_revision(
        raw.body,
        raw.entities,
        raw.legacy_cycle_input,
        manifest.revision,
        previous_input.revision,
    );
    if expected_revision != current_revision
        && expected_revision != previous_revision
        && expected_revision != legacy_revision
    {
        return Err(invalid_entities());
    }
    let full = entity_directory_from_raw(raw)?;
    if expected_revision == current_revision {
        return Ok(full);
    }
    super::catalog::EntityDirectory::new(
        expected_revision.to_owned(),
        full.revision(),
        full.is_complete(),
        full.entries().cloned().collect(),
    )
    .map_err(|_| invalid_entities())
}
/// 返回版本绑定的规范实体目录；名称只参与精确查找，不提供别名或获知证据。
/// # Errors
/// 未登记身份为 not-found；损坏、超限或不支持的清单为 not-ready。
pub fn entity_directory(id: &str) -> Result<super::catalog::EntityDirectory, Fault> {
    let raw = find(id)?;
    entity_directory_from_raw(raw)
}
fn entity_directory_from_raw(raw: &RawScript) -> Result<super::catalog::EntityDirectory, Fault> {
    let manifest = decode_entity_manifest(raw.entities)?;
    let directory_revision = manifest.revision;
    let complete = manifest.complete;
    // New entity snapshots follow the complete script revision, including versioned cycle input.
    let cycle_input = decode_cycle_input(raw.cycle_input, &manifest)?;
    let script_revision = script_revision(
        raw.body,
        raw.entities,
        raw.cycle_input,
        directory_revision,
        cycle_input.revision,
    );
    let entries = manifest
        .entities
        .into_iter()
        .map(|entry| super::catalog::CanonicalEntity {
            id: entry.id,
            kind: match entry.kind {
                EntityKind::Character => super::catalog::EntityKind::Character,
                EntityKind::Place => super::catalog::EntityKind::Place,
                EntityKind::Organization => super::catalog::EntityKind::Organization,
                EntityKind::Item => super::catalog::EntityKind::Item,
                EntityKind::Term => super::catalog::EntityKind::Term,
            },
            scope: entry.scope,
            canonical_name: entry.canonical_name,
        })
        .collect();
    super::catalog::EntityDirectory::new(script_revision, directory_revision, complete, entries)
        .map_err(|_| invalid_entities())
}
/// 返回指定脚本原 v1 资源哈希，用于读取历史周目及其记忆目录。
/// # Errors
/// 未登记身份或嵌入清单损坏时拒绝返回。
#[cfg(test)]
pub(crate) fn legacy_revision(id: &str) -> Result<String, Fault> {
    let raw = find(id)?;
    let manifest = decode_entity_manifest(raw.entities)?;
    Ok(legacy_script_revision(
        raw.body,
        raw.entities,
        manifest.revision,
    ))
}
/// # Errors
/// 当前登记正文 / 本地化 / 实体清单不合法返回 not-ready，不发网络或读盘。
pub fn list() -> Result<Vec<ScriptInfo>, Fault> {
    BUNDLED
        .iter()
        .map(|raw| {
            let _cycle_input = cycle_input(raw.id)?;
            let localization = parse_i18n(raw.i18n)?;
            Ok(ScriptInfo {
                script_id: raw.id.into(),
                title: scenario(raw.id)?.title,
                display_names: localization.display_names,
                suggested_actions: localization.suggested_actions,
                attributions: raw.notices.join("\n\n"),
            })
        })
        .collect()
}
fn decode_entity_manifest(source: &[u8]) -> Result<EntityManifest, Fault> {
    let manifest: EntityManifest = aoidos_json::decode_bytes(source, MAX_ENTITY_MANIFEST_BYTES)
        .map_err(|_| invalid_entities())?;
    if manifest.version != 1 || manifest.revision == 0 || manifest.entities.is_empty() {
        return Err(invalid_entities());
    }
    Ok(manifest)
}
fn decode_cycle_input(
    source: &[u8],
    entities: &EntityManifest,
) -> Result<CycleInputManifest, Fault> {
    let manifest: CycleInputManifest = aoidos_json::decode_bytes(source, MAX_CYCLE_INPUT_BYTES)
        .map_err(|_| invalid_cycle_input())?;
    if !matches!(manifest.version, 1 | 2)
        || manifest.revision == 0
        || manifest.state_fields.len() > 64
        || (manifest.version == 1 && !manifest.story_character_ids.is_empty())
        || (manifest.version == 2
            && (manifest.story_character_ids.is_empty()
                || manifest.story_character_ids.len() > 128))
    {
        return Err(invalid_cycle_input());
    }
    let mut ids = std::collections::BTreeSet::new();
    for field in &manifest.state_fields {
        if !crate::record::format::valid_id(&field.id)
            || field.id != "committed-round-count"
            || !ids.insert(&field.id)
            || field.kind != "integer"
            || field.source != "engine.committed-round-count.v1"
            || field.min < 1
            || field.max < field.min
            || field.max > 1_000_000
            || field.operators.is_empty()
            || field.operators.len() > 3
            || field
                .operators
                .iter()
                .any(|operator| !matches!(operator.as_str(), "equals" | "atLeast" | "atMost"))
            || field
                .operators
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != field.operators.len()
            || !valid_localizations(1, &field.descriptions, 256)
        {
            return Err(invalid_cycle_input());
        }
    }
    if manifest.characters.len() > 64 {
        return Err(invalid_cycle_input());
    }
    let mut character_ids = std::collections::BTreeSet::new();
    let mut story_character_ids = std::collections::BTreeSet::new();
    let mut guide_count = 0;
    for character_id in &manifest.story_character_ids {
        if !crate::record::format::valid_id(character_id)
            || !story_character_ids.insert(character_id)
            || !entities.entities.iter().any(|entity| {
                entity.id == *character_id && matches!(entity.kind, EntityKind::Character)
            })
        {
            return Err(invalid_cycle_input());
        }
    }
    if let Some(protagonist_id) = &manifest.protagonist_character_id
        && (!crate::record::format::valid_id(protagonist_id)
            || (manifest.version == 2 && !story_character_ids.contains(protagonist_id))
            || !entities.entities.iter().any(|entity| {
                entity.id == *protagonist_id && matches!(entity.kind, EntityKind::Character)
            }))
    {
        return Err(invalid_cycle_input());
    }
    for character in &manifest.characters {
        let registered_character = entities
            .entities
            .iter()
            .find(|entity| entity.id == character.character_id);
        if !crate::record::format::valid_id(&character.character_id)
            || !character_ids.insert(&character.character_id)
            || !registered_character
                .is_some_and(|entity| matches!(entity.kind, EntityKind::Character))
            || (manifest.version == 2 && !story_character_ids.contains(&character.character_id))
            || character.is_guide
                && manifest.protagonist_character_id.as_deref()
                    == Some(character.character_id.as_str())
            || [
                &character.truth.personal_name,
                &character.truth.background,
                &character.truth.identity,
                &character.truth.race,
                &character.truth.age,
                &character.truth.gender,
                &character.presentation.display_name,
                &character.presentation.identity,
                &character.presentation.race,
                &character.presentation.age,
                &character.presentation.gender,
                &character.presentation.appearance,
            ]
            .iter()
            .any(|value| invalid_profile_text(value))
            || character.initial_knowledge.len() > 6
            || character.initial_knowledge.iter().any(|field| {
                !matches!(
                    field.as_str(),
                    "personalName" | "background" | "identity" | "race" | "age" | "gender"
                )
            })
            || character
                .initial_knowledge
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != character.initial_knowledge.len()
            || [&character.opening_role, &character.opening_relationship]
                .iter()
                .filter_map(|value| value.as_ref())
                .any(|value| invalid_profile_text(value))
            || character.is_guide
                && (character.opening_role.is_none() || character.opening_relationship.is_none())
        {
            return Err(invalid_cycle_input());
        }
        guide_count += usize::from(character.is_guide);
    }
    if guide_count > 1 {
        return Err(invalid_cycle_input());
    }
    Ok(manifest)
}
pub(crate) fn validate_cycle_input_snapshot(
    cycle_input: &[u8],
    entity_snapshot: &[u8],
) -> Result<(CycleInputManifest, std::collections::BTreeSet<String>), Fault> {
    let entities = decode_entity_manifest(entity_snapshot)?;
    if !entities.complete {
        return Err(invalid_cycle_input());
    }
    let entries = entities
        .entities
        .iter()
        .map(|entity| super::catalog::CanonicalEntity {
            id: entity.id.clone(),
            kind: match entity.kind {
                EntityKind::Character => super::catalog::EntityKind::Character,
                EntityKind::Place => super::catalog::EntityKind::Place,
                EntityKind::Organization => super::catalog::EntityKind::Organization,
                EntityKind::Item => super::catalog::EntityKind::Item,
                EntityKind::Term => super::catalog::EntityKind::Term,
            },
            scope: entity.scope.clone(),
            canonical_name: entity.canonical_name.clone(),
        })
        .collect();
    super::catalog::EntityDirectory::new(
        crate::record::format::hash(b"cycle-input-snapshot-validation"),
        entities.revision,
        entities.complete,
        entries,
    )
    .map_err(|_| invalid_cycle_input())?;
    let input = decode_cycle_input(cycle_input, &entities)?;
    let entity_ids = entities
        .entities
        .into_iter()
        .map(|entity| entity.id)
        .collect();
    Ok((input, entity_ids))
}
/// Verifies frozen initialization resources without consulting the currently installed script.
pub(crate) fn validate_frozen_initialization_assets(
    script_id: &str,
    expected_revision: &str,
    body: &[u8],
    entity_snapshot: &[u8],
    cycle_input_snapshot: &[u8],
) -> Result<(), Fault> {
    if !aoidos_script::valid_id(script_id) || !crate::record::format::valid_hash(expected_revision)
    {
        return Err(invalid_cycle_input());
    }
    let manifest = decode_entity_manifest(entity_snapshot)?;
    let (input, _) = validate_cycle_input_snapshot(cycle_input_snapshot, entity_snapshot)?;
    // New initialization snapshots must use the current cycle-input schema. Historical v1
    // inputs remain readable through the compatibility path, but cannot seed new runs.
    if input.version != 2 {
        return Err(invalid_cycle_input());
    }
    let computed = script_revision(
        body,
        entity_snapshot,
        cycle_input_snapshot,
        manifest.revision,
        input.revision,
    );
    if computed != expected_revision {
        return Err(invalid_cycle_input());
    }
    Ok(())
}
fn invalid_profile_text(value: &str) -> bool {
    value.trim().is_empty() || value.len() > 4096 || value.chars().any(char::is_control)
}
fn validate_character_profiles(
    cycle_input: &CycleInputManifest,
    directory: &super::catalog::EntityDirectory,
) -> Result<(), Fault> {
    for character in &cycle_input.characters {
        if directory
            .entity(&character.character_id)
            .ok()
            .flatten()
            .is_none_or(|entity| entity.kind != super::catalog::EntityKind::Character)
        {
            return Err(invalid_cycle_input());
        }
    }
    Ok(())
}
fn script_revision(
    body: &[u8],
    entities: &[u8],
    cycle_input: &[u8],
    directory_revision: u64,
    cycle_input_revision: u64,
) -> String {
    // 长度前缀和域标签避免正文、清单拼接歧义；任一发布输入变化均使旧存档失配。
    let mut bytes = b"AOIDOS-SCRIPT-REVISION\0v2".to_vec();
    for part in [body, entities, cycle_input] {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    bytes.extend_from_slice(&directory_revision.to_be_bytes());
    bytes.extend_from_slice(&cycle_input_revision.to_be_bytes());
    crate::record::format::hash(&bytes)
}
pub(crate) fn legacy_script_revision(
    body: &[u8],
    entities: &[u8],
    directory_revision: u64,
) -> String {
    let mut bytes = b"AOIDOS-SCRIPT-REVISION\0v1".to_vec();
    for part in [body, entities] {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    bytes.extend_from_slice(&(8_u64).to_be_bytes());
    bytes.extend_from_slice(&directory_revision.to_be_bytes());
    crate::record::format::hash(&bytes)
}
fn parse_i18n(source: &str) -> Result<ScriptI18n, Fault> {
    let metadata: ScriptI18n = serde_json::from_str(source).map_err(invalid_localization)?;
    if !valid_localizations(metadata.version, &metadata.display_names, 256)
        || !valid_localizations(metadata.version, &metadata.suggested_actions, 512)
    {
        return Err(invalid_localization_data());
    }
    Ok(metadata)
}
fn valid_localizations(version: u32, values: &BTreeMap<String, String>, max_bytes: usize) -> bool {
    version == 1
        && values.len() == 2
        && ["en", "zh-Hans"]
            .iter()
            .all(|locale| values.contains_key(*locale))
        && values
            .values()
            .all(|value| !value.trim().is_empty() && value.len() <= max_bytes)
}
fn find(id: &str) -> Result<&'static RawScript, Fault> {
    BUNDLED.iter().find(|raw| raw.id == id).ok_or_else(unknown)
}
fn unknown() -> Fault {
    Fault::new("app.not-found", "剧本未登记")
}
fn invalid_asset(_: aoidos_script::Error) -> Fault {
    Fault::new("app.not-ready", "内嵌剧本原文不可用")
}
fn invalid_localization(_: serde_json::Error) -> Fault {
    invalid_localization_data()
}
fn invalid_localization_data() -> Fault {
    Fault::new("app.not-ready", "内嵌剧本本地化资源不可用")
}
fn invalid_entities() -> Fault {
    Fault::new("app.not-ready", "内嵌剧本实体目录不可用")
}
fn invalid_cycle_input() -> Fault {
    Fault::new("app.not-ready", "内嵌剧本周目输入配置不可用")
}
#[cfg(test)]
mod tests;
