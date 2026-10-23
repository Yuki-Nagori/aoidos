//! 开局目标、标签和初始任务的严格响应校验；此投影本身不能激活周目。

use crate::fault::Fault;
use aoidos_json::{decode_bytes, to_vec};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_OUTPUT_BYTES: usize = 128 * 1024;
const MAX_TAGS: usize = 24;
const MAX_RULES: usize = 96;
const MAX_CONDITIONS_PER_RULE: usize = 8;
const MAX_TASKS: usize = 12;
const MAX_TASK_DEPENDENCIES: usize = 8;
const MAX_CHAIN_STEPS: usize = 8;
const MAX_KEY_BYTES: usize = 48;
const MAX_TITLE_BYTES: usize = 160;
const MAX_DESCRIPTION_BYTES: usize = 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Response {
    version: u32,
    locale: String,
    goal: GoalResponse,
    tags: Vec<TagResponse>,
    initial_tasks: Vec<TaskResponse>,
    guide_npc: Option<GeneratedGuideResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeneratedGuideResponse {
    truth: crate::game::assets::CharacterTruth,
    initial_knowledge: Vec<String>,
    presentation: crate::game::assets::CharacterPresentation,
    opening_role: String,
    opening_relationship: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LocalizedText {
    pub(crate) en: String,
    #[serde(rename = "zh-Hans")]
    pub(crate) zh_hans: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GoalResponse {
    text: LocalizedText,
    achieved_tag_key: String,
    unmet_tag_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TagResponse {
    key: String,
    title: LocalizedText,
    description: LocalizedText,
    visibility: Visibility,
    #[serde(default)]
    conflict_group_key: Option<String>,
    #[serde(default)]
    conflict_priority: Option<u32>,
    rules: Vec<RuleResponse>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Visibility {
    Public,
    Private,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuleResponse {
    conditions: Vec<ConditionResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConditionResponse {
    field_id: String,
    operator: String,
    value: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TaskResponse {
    key: String,
    title: LocalizedText,
    description: LocalizedText,
    kind: TaskKind,
    #[serde(default)]
    chain_key: Option<String>,
    #[serde(default)]
    chain_title: Option<LocalizedText>,
    #[serde(default)]
    chain_step: Option<u16>,
    #[serde(default)]
    depends_on: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TaskKind {
    Task,
    ChainStep,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum InitialTaskState {
    Available,
    Locked,
}

/// 已通过可信剧本 schema 校验的目标、标签和任务投影；人物档案和 playable 事务另行接线。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OpeningContent {
    pub(crate) version: u32,
    pub(crate) locale: String,
    pub(crate) goal: GoalSnapshot,
    pub(crate) tags: Vec<TagSnapshot>,
    pub(crate) initial_tasks: Vec<TaskSnapshot>,
    pub(crate) guide: GuideSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum GuideSnapshot {
    Fixed {
        character_id: String,
        truth: crate::game::assets::CharacterTruth,
        initial_knowledge: Vec<String>,
        presentation: crate::game::assets::CharacterPresentation,
        opening_role: String,
        opening_relationship: String,
    },
    Generated {
        character_id: String,
        truth: crate::game::assets::CharacterTruth,
        initial_knowledge: Vec<String>,
        presentation: crate::game::assets::CharacterPresentation,
        opening_role: String,
        opening_relationship: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GoalSnapshot {
    pub(crate) text: LocalizedText,
    pub(crate) achieved_tag_id: String,
    pub(crate) unmet_tag_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TagSnapshot {
    pub(crate) id: String,
    pub(crate) order: u16,
    pub(crate) title: LocalizedText,
    pub(crate) description: LocalizedText,
    pub(crate) visibility: Visibility,
    pub(crate) conflict_group_id: Option<String>,
    pub(crate) conflict_priority: Option<u32>,
    /// 规则之间是 OR；每条规则内的条件是 AND。空规则列表明确表示永不命中。
    pub(crate) rules: Vec<RuleSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RuleSnapshot {
    pub(crate) id: String,
    pub(crate) conditions: Vec<ConditionSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConditionSnapshot {
    pub(crate) field_id: String,
    pub(crate) operator: String,
    pub(crate) value: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TaskSnapshot {
    pub(crate) id: String,
    pub(crate) order: u16,
    pub(crate) title: LocalizedText,
    pub(crate) description: LocalizedText,
    pub(crate) kind: TaskKind,
    pub(crate) state: InitialTaskState,
    pub(crate) chain_id: Option<String>,
    pub(crate) chain_title: Option<LocalizedText>,
    pub(crate) chain_step: Option<u16>,
    pub(crate) depends_on: Vec<String>,
}

/// 校验并将模型临时 key 转为由 engine 派生的稳定 ID。
///
/// 身份只取自已持久化 run / attempt；调用方必须提供该 run 冻结的可信输入清单。
/// 本函数不访问数据库、Provider 或隐藏游戏状态，也不激活周目。
pub fn validate_and_normalize(
    bytes: &[u8],
    session_id: &str,
    request_id: &str,
    locale: &str,
    cycle_input_snapshot: &[u8],
) -> Result<Vec<u8>, Fault> {
    if bytes.len() > MAX_OUTPUT_BYTES
        || !crate::record::format::valid_uuid(session_id)
        || !crate::record::format::valid_uuid(request_id)
        || !matches!(locale, "zh-Hans" | "en")
    {
        return Err(invalid_output());
    }
    let input: crate::game::assets::CycleInputManifest =
        decode_bytes(cycle_input_snapshot, MAX_OUTPUT_BYTES).map_err(|_| invalid_output())?;
    let response: Response = decode_bytes(bytes, MAX_OUTPUT_BYTES).map_err(|_| invalid_output())?;
    if response.version != 1
        || response.locale != locale
        || response.tags.is_empty()
        || response.tags.len() > MAX_TAGS
        || response.initial_tasks.len() > MAX_TASKS
    {
        return Err(invalid_output());
    }
    validate_localized(&response.goal.text, 1, MAX_DESCRIPTION_BYTES)?;
    validate_key(&response.goal.achieved_tag_key)?;
    validate_key(&response.goal.unmet_tag_key)?;
    if response.goal.achieved_tag_key == response.goal.unmet_tag_key {
        return Err(invalid_output());
    }

    if input.version != 1 || input.revision == 0 || input.state_fields.len() > 64 {
        return Err(invalid_output());
    }
    let guide = normalize_guide(&input, response.guide_npc, session_id, request_id)?;
    let mut fields = BTreeMap::new();
    for field in &input.state_fields {
        if field.id.is_empty()
            || field.id.len() > 64
            || !field.id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
            })
            || field.kind != "integer"
            || field.source.is_empty()
            || field.min > field.max
            || field.operators.is_empty()
            || field
                .operators
                .iter()
                .any(|operator| !matches!(operator.as_str(), "equals" | "atLeast" | "atMost"))
            || field.operators.iter().collect::<BTreeSet<_>>().len() != field.operators.len()
            || fields.insert(field.id.as_str(), field).is_some()
        {
            return Err(invalid_output());
        }
    }
    let mut tag_ids = BTreeMap::new();
    let mut conflict_groups: BTreeMap<&str, BTreeSet<u32>> = BTreeMap::new();
    let mut conflict_members: BTreeMap<&str, usize> = BTreeMap::new();
    let mut rule_count = 0usize;
    for tag in &response.tags {
        validate_key(&tag.key)?;
        validate_localized(&tag.title, 1, MAX_TITLE_BYTES)?;
        validate_localized(&tag.description, 1, MAX_DESCRIPTION_BYTES)?;
        if tag.rules.len() > 8 || tag_ids.contains_key(tag.key.as_str()) {
            return Err(invalid_output());
        }
        match (&tag.conflict_group_key, tag.conflict_priority) {
            (None, None) => {}
            (Some(group), Some(priority)) => {
                validate_key(group)?;
                if !conflict_groups.entry(group).or_default().insert(priority) {
                    return Err(invalid_output());
                }
                *conflict_members.entry(group).or_default() += 1;
            }
            _ => return Err(invalid_output()),
        }
        tag_ids.insert(
            tag.key.as_str(),
            stable_id("tag", session_id, request_id, &tag.key),
        );
        for rule in &tag.rules {
            rule_count += 1;
            if rule.conditions.is_empty() || rule.conditions.len() > MAX_CONDITIONS_PER_RULE {
                return Err(invalid_output());
            }
            validate_rule(rule, &fields)?;
        }
    }
    if rule_count > MAX_RULES || conflict_members.values().any(|members| *members < 2) {
        return Err(invalid_output());
    }
    let achieved_id = tag_ids
        .get(response.goal.achieved_tag_key.as_str())
        .ok_or_else(invalid_output)?;
    let unmet_id = tag_ids
        .get(response.goal.unmet_tag_key.as_str())
        .ok_or_else(invalid_output)?;
    if response.tags.iter().any(|tag| {
        (tag.key == response.goal.achieved_tag_key || tag.key == response.goal.unmet_tag_key)
            && tag.visibility != Visibility::Public
    }) {
        return Err(invalid_output());
    }

    let tags = response
        .tags
        .iter()
        .enumerate()
        .map(|(index, tag)| {
            let id = tag_ids[tag.key.as_str()].clone();
            let conflict_group_id = tag
                .conflict_group_key
                .as_deref()
                .map(|key| stable_id("tag-conflict", session_id, request_id, key));
            let rules = tag
                .rules
                .iter()
                .enumerate()
                .map(|(rule_index, rule)| RuleSnapshot {
                    id: stable_id(
                        "rule",
                        session_id,
                        request_id,
                        &format!("{}:{rule_index}", tag.key),
                    ),
                    conditions: rule
                        .conditions
                        .iter()
                        .map(|condition| ConditionSnapshot {
                            field_id: condition.field_id.clone(),
                            operator: condition.operator.clone(),
                            value: condition.value,
                        })
                        .collect(),
                })
                .collect();
            TagSnapshot {
                id,
                order: (index + 1) as u16,
                title: tag.title.clone(),
                description: tag.description.clone(),
                visibility: tag.visibility,
                conflict_group_id,
                conflict_priority: tag.conflict_priority,
                rules,
            }
        })
        .collect();
    let tasks = normalize_tasks(&response.initial_tasks, session_id, request_id)?;
    to_vec(&OpeningContent {
        version: 1,
        locale: response.locale,
        goal: GoalSnapshot {
            text: response.goal.text,
            achieved_tag_id: achieved_id.clone(),
            unmet_tag_id: unmet_id.clone(),
        },
        tags,
        initial_tasks: tasks,
        guide,
    })
    .map_err(|_| invalid_output())
}

fn normalize_guide(
    input: &crate::game::assets::CycleInputManifest,
    generated: Option<GeneratedGuideResponse>,
    session_id: &str,
    request_id: &str,
) -> Result<GuideSnapshot, Fault> {
    let mut fixed_guides = input
        .characters
        .iter()
        .filter(|character| character.is_guide);
    match (fixed_guides.next(), fixed_guides.next(), generated) {
        (Some(character), None, None) => Ok(GuideSnapshot::Fixed {
            character_id: character.character_id.clone(),
            truth: character.truth.clone(),
            initial_knowledge: character.initial_knowledge.clone(),
            presentation: character.presentation.clone(),
            opening_role: character.opening_role.clone().ok_or_else(invalid_output)?,
            opening_relationship: character
                .opening_relationship
                .clone()
                .ok_or_else(invalid_output)?,
        }),
        (None, None, Some(generated)) => {
            validate_generated_guide(&generated)?;
            Ok(GuideSnapshot::Generated {
                character_id: stable_id("guide-character", session_id, request_id, "guide"),
                truth: generated.truth,
                initial_knowledge: generated.initial_knowledge,
                presentation: generated.presentation,
                opening_role: generated.opening_role,
                opening_relationship: generated.opening_relationship,
            })
        }
        _ => Err(invalid_output()),
    }
}

fn validate_generated_guide(guide: &GeneratedGuideResponse) -> Result<(), Fault> {
    for (value, max_bytes) in [
        (&guide.truth.personal_name, 128),
        (&guide.truth.background, MAX_DESCRIPTION_BYTES),
        (&guide.truth.identity, MAX_TITLE_BYTES),
        (&guide.truth.race, 128),
        (&guide.truth.age, 128),
        (&guide.truth.gender, 128),
        (&guide.presentation.display_name, 128),
        (&guide.presentation.identity, MAX_TITLE_BYTES),
        (&guide.presentation.race, 128),
        (&guide.presentation.age, 128),
        (&guide.presentation.gender, 128),
        (&guide.presentation.appearance, MAX_DESCRIPTION_BYTES),
        (&guide.opening_role, MAX_TITLE_BYTES),
        (&guide.opening_relationship, MAX_DESCRIPTION_BYTES),
    ] {
        validate_text(value, 1, max_bytes)?;
    }
    if guide.initial_knowledge.len() > 6
        || guide.initial_knowledge.iter().any(|field| {
            !matches!(
                field.as_str(),
                "personalName" | "background" | "identity" | "race" | "age" | "gender"
            )
        })
        || guide
            .initial_knowledge
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != guide.initial_knowledge.len()
    {
        return Err(invalid_output());
    }
    Ok(())
}

fn validate_rule(
    rule: &RuleResponse,
    fields: &BTreeMap<&str, &crate::game::assets::StateField>,
) -> Result<(), Fault> {
    let mut constraints: BTreeMap<&str, (i64, i64)> = BTreeMap::new();
    let mut unique = BTreeSet::new();
    for condition in &rule.conditions {
        let field = fields
            .get(condition.field_id.as_str())
            .ok_or_else(invalid_output)?;
        if condition.value < field.min
            || condition.value > field.max
            || !field
                .operators
                .iter()
                .any(|operator| operator == &condition.operator)
            || !matches!(condition.operator.as_str(), "equals" | "atLeast" | "atMost")
            || !unique.insert((condition.field_id.as_str(), condition.operator.as_str()))
        {
            return Err(invalid_output());
        }
        let range = constraints
            .entry(field.id.as_str())
            .or_insert((field.min, field.max));
        match condition.operator.as_str() {
            "equals" => {
                range.0 = range.0.max(condition.value);
                range.1 = range.1.min(condition.value);
            }
            "atLeast" => range.0 = range.0.max(condition.value),
            "atMost" => range.1 = range.1.min(condition.value),
            _ => return Err(invalid_output()),
        }
        if range.0 > range.1 {
            return Err(invalid_output());
        }
    }
    Ok(())
}

fn normalize_tasks(
    tasks: &[TaskResponse],
    session_id: &str,
    request_id: &str,
) -> Result<Vec<TaskSnapshot>, Fault> {
    let mut ids = BTreeMap::new();
    let mut chain_state: BTreeMap<&str, (LocalizedText, u16, &str, usize)> = BTreeMap::new();
    let mut chain_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut edge_count = 0usize;
    for task in tasks {
        validate_key(&task.key)?;
        validate_localized(&task.title, 1, MAX_TITLE_BYTES)?;
        validate_localized(&task.description, 1, MAX_DESCRIPTION_BYTES)?;
        if ids.contains_key(task.key.as_str())
            || task.depends_on.len() > MAX_TASK_DEPENDENCIES
            || task.depends_on.iter().collect::<BTreeSet<_>>().len() != task.depends_on.len()
            || task
                .depends_on
                .iter()
                .any(|dependency| !ids.contains_key(dependency.as_str()))
        {
            return Err(invalid_output());
        }
        edge_count += task.depends_on.len();
        match task.kind {
            TaskKind::Task
                if task.chain_key.is_none()
                    && task.chain_title.is_none()
                    && task.chain_step.is_none() => {}
            TaskKind::ChainStep => {
                let key = task.chain_key.as_deref().ok_or_else(invalid_output)?;
                let title = task.chain_title.as_ref().ok_or_else(invalid_output)?;
                let step = task.chain_step.ok_or_else(invalid_output)?;
                validate_key(key)?;
                validate_localized(title, 1, MAX_TITLE_BYTES)?;
                let count = chain_counts.entry(key).or_default();
                *count += 1;
                if *count > MAX_CHAIN_STEPS || step == 0 {
                    return Err(invalid_output());
                }
                if let Some((previous_title, previous_step, previous_key, _)) = chain_state.get(key)
                {
                    if previous_title != title
                        || previous_step.checked_add(1) != Some(step)
                        || task.depends_on.as_slice() != [*previous_key]
                    {
                        return Err(invalid_output());
                    }
                } else if step != 1 {
                    return Err(invalid_output());
                }
                chain_state.insert(key, (title.clone(), step, &task.key, *count));
            }
            _ => return Err(invalid_output()),
        }
        ids.insert(
            task.key.as_str(),
            stable_id("task", session_id, request_id, &task.key),
        );
    }
    if edge_count > MAX_TASKS * MAX_TASK_DEPENDENCIES
        || chain_counts.values().any(|count| *count < 2)
    {
        return Err(invalid_output());
    }
    tasks
        .iter()
        .enumerate()
        .map(|(index, task)| {
            let id = ids[task.key.as_str()].clone();
            let depends_on = task
                .depends_on
                .iter()
                .map(|key| ids.get(key.as_str()).cloned().ok_or_else(invalid_output))
                .collect::<Result<Vec<_>, _>>()?;
            let chain_id = task
                .chain_key
                .as_deref()
                .map(|key| stable_id("task-chain", session_id, request_id, key));
            let state = if depends_on.is_empty() {
                InitialTaskState::Available
            } else {
                InitialTaskState::Locked
            };
            Ok(TaskSnapshot {
                id,
                order: (index + 1) as u16,
                title: task.title.clone(),
                description: task.description.clone(),
                kind: task.kind,
                state,
                chain_id,
                chain_title: task.chain_title.clone(),
                chain_step: task.chain_step,
                depends_on,
            })
        })
        .collect()
}

fn validate_localized(text: &LocalizedText, min: usize, max_bytes: usize) -> Result<(), Fault> {
    for value in [&text.en, &text.zh_hans] {
        validate_text(value, min, max_bytes)?;
    }
    Ok(())
}

fn validate_key(value: &str) -> Result<(), Fault> {
    if value.is_empty()
        || value.len() > MAX_KEY_BYTES
        || value.starts_with('-')
        || value.ends_with('-')
        || value.contains("--")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(invalid_output());
    }
    Ok(())
}

fn validate_text(value: &str, min: usize, max_bytes: usize) -> Result<(), Fault> {
    if value.trim() != value
        || value.chars().count() < min
        || value.len() > max_bytes
        || value.chars().any(char::is_control)
    {
        return Err(invalid_output());
    }
    Ok(())
}

fn stable_id(kind: &str, session_id: &str, request_id: &str, key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"aoidos-cycle-id-v1\0");
    for part in [
        kind.as_bytes(),
        session_id.as_bytes(),
        request_id.as_bytes(),
        key.as_bytes(),
    ] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn invalid_output() -> Fault {
    Fault::new("engine.invalid-initialization", "开局生成结果未通过校验")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "00000000-0000-4000-8000-000000000001";
    const REQUEST: &str = "00000000-0000-4000-8000-000000000002";

    fn manifest() -> Vec<u8> {
        aoidos_json::to_vec(&crate::game::assets::cycle_input("mistbell").unwrap()).unwrap()
    }

    fn text(en: &str, zh: &str) -> serde_json::Value {
        serde_json::json!({"en":en,"zh-Hans":zh})
    }

    fn valid_response() -> serde_json::Value {
        serde_json::json!({
            "version":1, "locale":"zh-Hans",
            "goal":{"text":text("Uncover the old tower's secret","揭开旧塔钟声背后的秘密"),"achievedTagKey":"tower-truth","unmetTagKey":"tower-unresolved"},
            "tags":[
                {"key":"tower-truth","title":text("Tower's truth","旧塔真相"),"description":text("Discover the source of the bell","查明钟声来源"),"visibility":"public","rules":[{"conditions":[{"fieldId":"committed-round-count","operator":"atLeast","value":4}]}]},
                {"key":"tower-unresolved","title":text("Unresolved","疑问未解"),"description":text("The bell's source remains unknown","尚未查明钟声来源"),"visibility":"public","rules":[{"conditions":[{"fieldId":"committed-round-count","operator":"atMost","value":3}]}]}
            ],
            "initialTasks":[
                {"key":"reach-village","title":text("Reach the village","抵达苔泉村"),"description":text("Find the village entrance.","沿山路找到村口。"),"kind":"task","dependsOn":[]},
                {"key":"ask-luo-wei","title":text("Ask the lamplighter","询问守灯人"),"description":text("Learn about the old tower.","了解旧塔的传闻。"),"kind":"chain-step","chainKey":"tower-inquiry","chainTitle":text("Investigate the tower","追查旧塔"),"chainStep":1,"dependsOn":["reach-village"]},
                {"key":"inspect-bell","title":text("Inspect the bell","检查铜钟"),"description":text("Look for marks on the bell.","寻找钟体上的痕迹。"),"kind":"chain-step","chainKey":"tower-inquiry","chainTitle":text("Investigate the tower","追查旧塔"),"chainStep":2,"dependsOn":["ask-luo-wei"]}
            ],
            "guideNpc":null
        })
    }

    fn normalize(value: &serde_json::Value) -> Result<Vec<u8>, Fault> {
        validate_and_normalize(
            &serde_json::to_vec(value).unwrap(),
            SESSION,
            REQUEST,
            "zh-Hans",
            &manifest(),
        )
    }

    fn normalize_without_fixed_guide(value: &serde_json::Value) -> Result<Vec<u8>, Fault> {
        let mut input: crate::game::assets::CycleInputManifest =
            decode_bytes(&manifest(), MAX_OUTPUT_BYTES).unwrap();
        for character in &mut input.characters {
            character.is_guide = false;
            character.opening_role = None;
            character.opening_relationship = None;
        }
        validate_and_normalize(
            &serde_json::to_vec(value).unwrap(),
            SESSION,
            REQUEST,
            "zh-Hans",
            &aoidos_json::to_vec(&input).unwrap(),
        )
    }

    #[test]
    fn validates_and_stably_normalizes_whitelisted_content() {
        let first = normalize(&valid_response()).unwrap();
        assert_eq!(first, normalize(&valid_response()).unwrap());
        let snapshot: OpeningContent = decode_bytes(&first, MAX_OUTPUT_BYTES).unwrap();
        assert_eq!(snapshot.goal.text.zh_hans, "揭开旧塔钟声背后的秘密");
        assert_ne!(snapshot.goal.achieved_tag_id, snapshot.goal.unmet_tag_id);
        assert_eq!(snapshot.tags[0].order, 1);
        assert_eq!(
            snapshot.initial_tasks[2].depends_on,
            vec![snapshot.initial_tasks[1].id.clone()]
        );
        assert_eq!(snapshot.initial_tasks[0].state, InitialTaskState::Available);
        assert_eq!(snapshot.initial_tasks[1].state, InitialTaskState::Locked);
        assert_ne!(snapshot.initial_tasks[0].id, "reach-village");
        assert!(matches!(snapshot.guide, GuideSnapshot::Fixed { .. }));

        let second_request = validate_and_normalize(
            &serde_json::to_vec(&valid_response()).unwrap(),
            SESSION,
            "00000000-0000-4000-8000-000000000003",
            "zh-Hans",
            &manifest(),
        )
        .unwrap();
        let second: OpeningContent = decode_bytes(&second_request, MAX_OUTPUT_BYTES).unwrap();
        assert_ne!(snapshot.initial_tasks[0].id, second.initial_tasks[0].id);
    }

    #[test]
    fn generated_guide_is_required_only_without_a_fixed_guide_and_gets_engine_id() {
        let mut value = valid_response();
        value["guideNpc"] = serde_json::json!({
            "truth": {
                "personalName":"Mira", "background":"A village guide.",
                "identity":"Wayfinder", "race":"Human", "age":"Adult", "gender":"Female"
            },
            "initialKnowledge":["background"],
            "presentation": {
                "displayName":"Mira", "identity":"Local guide", "race":"Human",
                "age":"Adult", "gender":"Female", "appearance":"A travel-worn coat."
            },
            "openingRole":"Village guide",
            "openingRelationship":"Introduces the first lead."
        });
        assert!(normalize(&value).is_err());

        let first = normalize_without_fixed_guide(&value).unwrap();
        assert_eq!(first, normalize_without_fixed_guide(&value).unwrap());
        let snapshot: OpeningContent = decode_bytes(&first, MAX_OUTPUT_BYTES).unwrap();
        let GuideSnapshot::Generated {
            character_id,
            truth,
            opening_role,
            ..
        } = snapshot.guide
        else {
            panic!("expected generated guide")
        };
        assert_ne!(character_id, "Mira");
        assert_eq!(truth.personal_name, "Mira");
        assert_eq!(opening_role, "Village guide");

        let mut missing = valid_response();
        missing["guideNpc"] = serde_json::Value::Null;
        assert!(normalize_without_fixed_guide(&missing).is_err());
    }

    #[test]
    fn generated_guide_rejects_invalid_knowledge_and_incomplete_profile() {
        let mut value = valid_response();
        value["guideNpc"] = serde_json::json!({
            "truth": {
                "personalName":"Mira", "background":"A village guide.",
                "identity":"Wayfinder", "race":"Human", "age":"Adult", "gender":"Female"
            },
            "initialKnowledge":["hiddenSecret"],
            "presentation": {
                "displayName":"Mira", "identity":"Local guide", "race":"Human",
                "age":"Adult", "gender":"Female", "appearance":"A travel-worn coat."
            },
            "openingRole":"Village guide",
            "openingRelationship":"Introduces the first lead."
        });
        assert!(normalize_without_fixed_guide(&value).is_err());
        value["guideNpc"]["initialKnowledge"] = serde_json::json!([]);
        value["guideNpc"]["truth"]["race"] = serde_json::json!("");
        assert!(normalize_without_fixed_guide(&value).is_err());
    }

    #[test]
    fn rejects_duplicate_json_keys_and_malformed_trusted_input() {
        let duplicate_key = br#"{"version":1,"version":1}"#;
        assert!(
            validate_and_normalize(duplicate_key, SESSION, REQUEST, "zh-Hans", &manifest())
                .is_err()
        );

        let mut input: crate::game::assets::CycleInputManifest =
            decode_bytes(&manifest(), MAX_OUTPUT_BYTES).unwrap();
        input.state_fields[0].operators.push("atLeast".into());
        assert!(
            validate_and_normalize(
                &serde_json::to_vec(&valid_response()).unwrap(),
                SESSION,
                REQUEST,
                "zh-Hans",
                &aoidos_json::to_vec(&input).unwrap()
            )
            .is_err()
        );

        let mut missing_tasks = valid_response();
        missing_tasks
            .as_object_mut()
            .unwrap()
            .remove("initialTasks");
        assert!(normalize(&missing_tasks).is_err());
    }

    #[test]
    fn rejects_unknown_fields_unregistered_rules_and_private_goal_tags() {
        let mut value = valid_response();
        value["extra"] = serde_json::json!(true);
        assert!(normalize(&value).is_err());
        let mut value = valid_response();
        value["tags"][0]["rules"][0]["conditions"][0]["fieldId"] =
            serde_json::json!("hidden-secret");
        assert!(normalize(&value).is_err());
        let mut value = valid_response();
        value["tags"][0]["visibility"] = serde_json::json!("private");
        assert!(normalize(&value).is_err());
    }

    #[test]
    fn rejects_threshold_intersection_conflicts_and_invalid_task_edges() {
        let mut value = valid_response();
        value["tags"][0]["rules"][0]["conditions"] = serde_json::json!([
            {"fieldId":"committed-round-count","operator":"atLeast","value":10},
            {"fieldId":"committed-round-count","operator":"atMost","value":2}
        ]);
        assert!(normalize(&value).is_err());
        let mut value = valid_response();
        for index in 0..2 {
            value["tags"][index]["conflictGroupKey"] = serde_json::json!("ending-status");
            value["tags"][index]["conflictPriority"] = serde_json::json!(1);
        }
        assert!(normalize(&value).is_err());
        let mut value = valid_response();
        value["initialTasks"][0]["dependsOn"] = serde_json::json!(["ask-luo-wei"]);
        assert!(normalize(&value).is_err());
        let mut value = valid_response();
        value["initialTasks"][0]["dependsOn"] = serde_json::json!(["reach-village"]);
        assert!(normalize(&value).is_err());
        let mut value = valid_response();
        value["initialTasks"][2]["chainStep"] = serde_json::json!(3);
        assert!(normalize(&value).is_err());
    }

    #[test]
    fn empty_rules_are_never_match_and_locale_coverage_is_mandatory() {
        let mut input: crate::game::assets::CycleInputManifest =
            decode_bytes(&manifest(), MAX_OUTPUT_BYTES).unwrap();
        input.state_fields.clear();
        let mut value = valid_response();
        value["tags"][0]["rules"] = serde_json::json!([]);
        value["tags"][1]["rules"] = serde_json::json!([]);
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(
            validate_and_normalize(
                &bytes,
                SESSION,
                REQUEST,
                "zh-Hans",
                &aoidos_json::to_vec(&input).unwrap()
            )
            .is_ok()
        );
        value["goal"]["text"] = serde_json::json!({"zh-Hans":"缺少英文"});
        assert!(normalize(&value).is_err());
    }

    #[test]
    fn rejects_wrong_locale_untrimmed_text_and_oversize_output() {
        let mut value = valid_response();
        value["locale"] = serde_json::json!("ja");
        assert!(normalize(&value).is_err());
        let mut value = valid_response();
        value["goal"]["text"]["zh-Hans"] = serde_json::json!(" 前后空格 ");
        assert!(normalize(&value).is_err());
        let too_large = vec![b'x'; MAX_OUTPUT_BYTES + 1];
        assert!(
            validate_and_normalize(&too_large, SESSION, REQUEST, "zh-Hans", &manifest()).is_err()
        );
    }
}
