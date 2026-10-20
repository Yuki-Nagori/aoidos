//! 随应用发布的默认资源；编译时嵌入，运行时只按身份选择。

use crate::fault::Fault;
use std::collections::BTreeMap;
struct RawScript {
    id: &'static str,
    body: &'static [u8],
    i18n: &'static str,
    entities: &'static [u8],
    notices: &'static [&'static str],
}
// 默认内容属于发布产物，固定路径只用于编译取材，不是运行时文件位置。
const BUNDLED: &[RawScript] = &[RawScript {
    id: "mistbell",
    body: include_bytes!("../../../../../resources/scripts/mistbell/Mistbell.md"),
    i18n: include_str!("../../../../../resources/scripts/mistbell/i18n.json"),
    entities: include_bytes!("../../../../../resources/scripts/mistbell/entities.json"),
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
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EntityManifest {
    version: u32,
    revision: u64,
    complete: bool,
    entities: Vec<EntityEntry>,
}
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EntityEntry {
    id: String,
    kind: EntityKind,
    scope: String,
    canonical_name: String,
}
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum EntityKind {
    Character,
    Place,
    Organization,
    Item,
    Term,
}
const MAX_ENTITY_MANIFEST_BYTES: usize = 1024 * 1024;
/// # Errors
/// 未登记身份为 not-found，内嵌原文不合法为 not-ready。
pub fn scenario(id: &str) -> Result<aoidos_script::Scenario, Fault> {
    let raw = find(id)?;
    aoidos_script::decode(raw.id, raw.body).map_err(invalid_asset)
}
/// 返回绑定原文、实体清单字节与目录 revision 的发布版本。
/// # Errors
/// 未登记身份为 not-found；损坏或不支持的实体清单为 not-ready。
pub fn revision(id: &str) -> Result<String, Fault> {
    Ok(entity_directory(id)?.script_revision().to_owned())
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
    let script_revision = script_revision(raw.body, raw.entities, directory_revision);
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
/// # Errors
/// 当前登记正文 / 本地化 / 实体清单不合法返回 not-ready，不发网络或读盘。
pub fn list() -> Result<Vec<ScriptInfo>, Fault> {
    BUNDLED
        .iter()
        .map(|raw| {
            entity_directory(raw.id)?;
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
fn script_revision(body: &[u8], entities: &[u8], directory_revision: u64) -> String {
    // 长度前缀和域标签避免正文、清单拼接歧义；任一发布输入变化均使旧存档失配。
    let mut bytes = b"AOIDOS-SCRIPT-REVISION\0v1".to_vec();
    for part in [body, entities] {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    let revision_bytes = directory_revision.to_be_bytes();
    bytes.extend_from_slice(&(revision_bytes.len() as u64).to_be_bytes());
    bytes.extend_from_slice(&revision_bytes);
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
#[cfg(test)]
mod tests;
