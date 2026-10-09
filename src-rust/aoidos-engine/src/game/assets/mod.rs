//! 随应用发布的默认资源；编译时嵌入，运行时只按身份选择。

use crate::fault::Fault;
struct RawScript {
    id: &'static str,
    body: &'static [u8],
    notices: &'static [&'static str],
}
// 默认内容属于发布产物，固定路径只用于编译取材，不是运行时文件位置。
const BUNDLED: &[RawScript] = &[RawScript {
    id: "mistbell",
    body: include_bytes!("../../../../../resources/scripts/mistbell/Mistbell.md"),
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
    pub attributions: String,
}
/// # Errors
/// 未登记身份为 not-found，内嵌原文不合法为 not-ready。
pub fn scenario(id: &str) -> Result<aoidos_script::Scenario, Fault> {
    let raw = find(id)?;
    aoidos_script::decode(raw.id, raw.body).map_err(invalid_asset)
}
/// 发布版本按正文原字节冻结，资源更新不静默套入旧存档。
/// # Errors
/// 未登记身份为 not-found。
pub fn revision(id: &str) -> Result<String, Fault> {
    Ok(crate::record::format::hash(find(id)?.body))
}
/// # Errors
/// 当前登记原文不合法返回 not-ready，不发网络或读盘。
pub fn list() -> Result<Vec<ScriptInfo>, Fault> {
    BUNDLED
        .iter()
        .map(|raw| {
            Ok(ScriptInfo {
                script_id: raw.id.into(),
                title: scenario(raw.id)?.title,
                attributions: raw.notices.join("\n\n"),
            })
        })
        .collect()
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
#[cfg(test)]
mod tests;
