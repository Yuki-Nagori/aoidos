//! 将可信场景规则结果转换为记忆轮回模块所需的数据。

use super::catalog::{HintRule, SceneRuleMatch, SceneRuleView};
use crate::fault::Fault;
use aoidos_memory::model::{MAX_SCENE_RULE_EVIDENCE, SceneProgress, SceneRuleEvidence, hash};
use std::collections::{BTreeMap, BTreeSet};

/// 将已确认的规则匹配结果转换为稳定且有界的节点进度依据。
///
/// 仓储不负责求值剧本规则；调用方必须从当前 `SceneCatalog` 获得匹配结果，
/// 并确保所有结果使用相同的目录修订和历史修订。
/// # Errors
/// 过期、重复、越权、超限或不支持的依据都会被拒绝。
pub fn progress_evidence(
    scene: &SceneRuleView,
    matches: &[SceneRuleMatch],
    history_revision: u64,
    world_revision: &str,
) -> Result<Vec<SceneRuleEvidence>, Fault> {
    let allowed = scene
        .progress_rule_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    if matches.len() > MAX_SCENE_RULE_EVIDENCE {
        return Err(super::invalid_phase());
    }
    let mut evidence = Vec::new();
    for matched in matches {
        if matched.catalog_revision != scene.catalog_revision
            || matched.history_revision != history_revision
            || matched.world_revision != world_revision
            || !allowed.contains(matched.rule_id.as_str())
            || !seen.insert(matched.rule_id.as_str())
            || matched.evidence_refs.is_empty()
        {
            return Err(super::invalid_phase());
        }
        if matched.matched {
            let payload =
                aoidos_json::canonical_string(&(matched.rule_id.as_str(), &matched.evidence_refs))
                    .map_err(|_| super::invalid_phase())?;
            let evidence_json = payload.into_bytes();
            if evidence_json.len() > aoidos_memory::model::MAX_SCENE_EVIDENCE_BYTES {
                return Err(super::invalid_phase());
            }
            evidence.push(SceneRuleEvidence {
                rule_id: matched.rule_id.clone(),
                evidence_hash: hash(&evidence_json),
                evidence_json,
            });
        }
    }
    evidence.sort_by(|left, right| left.rule_id.as_bytes().cmp(right.rule_id.as_bytes()));
    Ok(evidence)
}

/// 按稳定优先级返回登记条件已满足的固定提示。
/// 提示领取及冷却校验由单独的仓储事务完成。
/// # Errors
/// 任何过期或重复的规则结果都会被拒绝，不会静默忽略。
pub fn eligible_hints(
    scene: &SceneRuleView,
    matches: &[SceneRuleMatch],
    progress: &SceneProgress,
    history_revision: u64,
    world_revision: &str,
) -> Result<Vec<HintRule>, Fault> {
    if progress.scene_id != scene.scene_id
        || progress.catalog_revision != scene.catalog_revision
        || progress.entry_history_revision != history_revision
    {
        return Err(super::invalid_phase());
    }
    if progress.stagnation_rounds < aoidos_memory::model::NODE_HINT_AFTER_STAGNANT_ROUNDS {
        return Ok(Vec::new());
    }
    if progress.hints_shown >= aoidos_memory::model::MAX_CYCLE_HINTS_PER_SCENE {
        return Ok(Vec::new());
    }
    let registered_rules = scene
        .progress_rule_ids
        .iter()
        .chain(&scene.advance_rule_ids)
        .chain(&scene.exit_rule_ids)
        .map(String::as_str)
        .chain(
            scene
                .hint_rules
                .iter()
                .map(|hint| hint.when_rule_id.as_str()),
        )
        .collect::<BTreeSet<_>>();
    let mut matched_rules = BTreeMap::new();
    for matched in matches {
        if matched.catalog_revision != scene.catalog_revision
            || matched.history_revision != history_revision
            || matched.world_revision != world_revision
            || !registered_rules.contains(matched.rule_id.as_str())
            || matched.evidence_refs.is_empty()
            || matched_rules
                .insert(matched.rule_id.as_str(), matched.matched)
                .is_some()
        {
            return Err(super::invalid_phase());
        }
    }
    let mut hints = scene
        .hint_rules
        .iter()
        .filter(|hint| matched_rules.get(hint.when_rule_id.as_str()) == Some(&true))
        .cloned()
        .collect::<Vec<_>>();
    hints.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.hint_id.as_bytes().cmp(right.hint_id.as_bytes()))
    });
    Ok(hints)
}

#[cfg(test)]
mod tests;
