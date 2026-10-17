//! 将冻结批次中的有效来源与剧本授权合并为刻痕专用上下文。

use super::SourcePort;
use crate::{
    fault::Fault,
    game::catalog::{EntityDirectory, SceneCatalog},
    record::session::Session,
};
use aoidos_memory::{
    model::{
        CausalPort, CycleMarkAuthorization, CycleSourceRef, MAX_CYCLE_SOURCES, ProposalContext, Run,
    },
    repository::BatchSnapshot,
};
use std::collections::{BTreeMap, BTreeSet};

/// 构造刻痕上下文时需要同时核验的可信输入。
pub struct CycleMarkContextInput<'a> {
    pub catalog: &'a SceneCatalog,
    pub run: &'a Run,
    pub session: &'a Session,
    pub directory: &'a EntityDirectory,
    /// 必须来自仓储的 `Repository::batch`，不能从请求体重新构造。
    pub batch: &'a BatchSnapshot,
    pub source_port: &'a SourcePort<'a>,
}

/// 可合并进完整提案上下文的刻痕授权与来源白名单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleMarkContext {
    source_ids: BTreeSet<String>,
    entity_ids: BTreeSet<String>,
    rules: BTreeMap<String, CycleMarkAuthorization>,
    sources: BTreeMap<String, CycleSourceRef>,
}

impl CycleMarkContext {
    /// 只补充刻痕所需白名单；已有同名映射必须完全一致。
    /// # Errors
    /// 已有规则 / 来源不属于当前批次，或同名映射指向不同值时拒绝合并。
    pub fn apply_to(self, context: &mut ProposalContext) -> Result<(), Fault> {
        if context
            .cycle_rules
            .keys()
            .any(|id| !self.rules.contains_key(id))
            || context
                .cycle_sources
                .keys()
                .any(|id| !self.sources.contains_key(id))
            || self.rules.iter().any(|(id, rule)| {
                context
                    .cycle_rules
                    .get(id)
                    .is_some_and(|existing| existing != rule)
            })
            || self.sources.iter().any(|(id, source)| {
                context
                    .cycle_sources
                    .get(id)
                    .is_some_and(|existing| existing != source)
            })
        {
            return Err(Fault::bad_request());
        }
        context.cycle_rules.extend(self.rules);
        context.cycle_sources.extend(self.sources);
        context.source_ids.extend(self.source_ids);
        context.entity_ids.extend(self.entity_ids);
        Ok(())
    }
}

/// 从当前有效批次构造模型可引用的刻痕来源和规则白名单。
///
/// 普通候选的主体、证据和目标快照仍由完整记忆请求构造器提供；本函数只负责
/// 合并刻痕所需字段，不构造 Prompt，也不提交模型响应。
/// # Errors
/// 批次边界过期、来源不在当前历史、目录不完整或任何批次身份不匹配时拒绝。
pub fn build_cycle_mark_context(
    input: CycleMarkContextInput<'_>,
) -> Result<CycleMarkContext, Fault> {
    let boundary = input.source_port.boundary().map_err(memory_fault)?;
    let batch = input.batch;
    if input.session.needs_recovery()
        || batch.version != 1
        || batch.run_id != input.run.run_id
        || batch.history_revision != boundary.history_revision
        || batch.entity_revision != boundary.entity_revision
        || batch.entity_revision != input.directory.revision()
        || batch.evidence_revision != boundary.evidence_revision
        || batch.policy_hash != input.run.policy_hash
        || boundary.script_id != input.run.script_id
        || boundary.session_id != input.run.session_id
        || batch.spans.len() > MAX_CYCLE_SOURCES
    {
        return Err(Fault::bad_request());
    }

    let rules =
        input
            .catalog
            .cycle_mark_authorizations(input.run, input.session, input.directory)?;
    let entity_ids = rules
        .values()
        .flat_map(|rule| rule.allowed_subject_ids.iter().cloned())
        .collect();
    let mut source_ids = BTreeSet::new();
    let mut sources = BTreeMap::new();
    for span in &batch.spans {
        let source_number = span
            .source_id
            .strip_prefix('s')
            .and_then(|number| number.parse::<usize>().ok());
        if !source_number.is_some_and(|number| {
            (1..=128).contains(&number) && span.source_id == format!("s{number}")
        }) || !source_ids.insert(span.source_id.clone())
            || span.source.run_id != input.run.run_id
            || !input
                .source_port
                .valid_source(&span.source)
                .map_err(memory_fault)?
            || input
                .source_port
                .source_text(&span.source)
                .map_err(memory_fault)?
                .as_deref()
                != Some(span.text.as_str())
        {
            return Err(Fault::bad_request());
        }
        sources.insert(
            span.source_id.clone(),
            CycleSourceRef {
                source: span.source.clone(),
                history_revision: batch.history_revision,
            },
        );
    }

    Ok(CycleMarkContext {
        source_ids,
        entity_ids,
        rules,
        sources,
    })
}

fn memory_fault(error: aoidos_memory::error::Error) -> Fault {
    Fault::new(error.code(), "记忆批次上下文已过期或不可用")
}
