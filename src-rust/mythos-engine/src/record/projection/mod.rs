//! 有界纯投影；不发网络请求、不写 recap，不修改事实或估算系数。

pub use super::estimator::{EstimatorRevision, estimate, estimate_input};
use super::{
    format::{Body, Header, InputMode, Parsed},
    grammar,
    session::Target,
};
use crate::ports::Terminal;
use mythos_llm::provider::{ChatInput, ChatMessage, ChatRole, CompletionInput, ProviderInput};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Completion,
    ChatPrefix,
    Chat,
}
/// context_limit 来自可信模型能力；未知窗口拒绝投影。
#[derive(Debug, Clone)]
pub struct Budget {
    pub estimator: EstimatorRevision,
    pub context_limit: Option<u32>,
    pub max_output_tokens: u32,
    pub input_hard_limit: u32,
    pub tail: u32,
    pub recap: u32,
    pub static_prefix: u32,
    pub world: u32,
    pub folding: u32,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            estimator: EstimatorRevision::ConservativeV2,
            context_limit: None,
            max_output_tokens: 4096,
            input_hard_limit: 32768,
            tail: 8192,
            recap: 4096,
            static_prefix: 8192,
            world: 2048,
            folding: 2048,
        }
    }
}
impl Budget {
    /// 从已核验能力与冻结 profile 输出额度建立投影预算。
    /// # Errors
    /// 未知窗口或输出额度非法拒绝，不发送请求。
    pub fn for_model(
        capabilities: &mythos_llm::provider::ProviderCapabilities,
        max_output_tokens: u32,
    ) -> Result<Self, ProjectionError> {
        let context_limit = capabilities
            .context_limit
            .ok_or(ProjectionError::InvalidConfig)?;
        if max_output_tokens == 0
            || max_output_tokens > capabilities.max_output_tokens
            || context_limit <= max_output_tokens.saturating_add(2048)
        {
            return Err(ProjectionError::InvalidConfig);
        }
        Ok(Self {
            context_limit: Some(context_limit),
            max_output_tokens,
            ..Self::default()
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionError {
    InvalidConfig,
    BudgetExceeded,
    ReadOnly,
    CorruptRecap,
    NeedsRecovery,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeqRange {
    pub from: u64,
    pub through: u64,
}
/// 调用方已经确认的有界工作集；未加载的旧物理区间必须显式传入。
pub struct RecordView<'a> {
    pub header: &'a Header,
    pub records: &'a [Parsed],
    pub needs_recovery: bool,
    pub omitted_ranges: &'a [SeqRange],
    pub(super) verified_recaps: &'a BTreeMap<u64, String>,
}
/// 世界字段仅来自已确认的只读引擎投影，不从任意外部数据库取值。
pub struct WorldView<'a> {
    pub context: &'a str,
    pub needs_recovery: bool,
}
impl ProjectionError {
    pub fn fault(self) -> crate::fault::Fault {
        match self {
            Self::ReadOnly | Self::NeedsRecovery => {
                crate::fault::Fault::new("engine.invalid-phase", "记录或世界状态尚不可用于生成")
            }
            Self::CorruptRecap => crate::fault::Fault::new("store.corrupt", "摘要与有效来源不一致"),
            Self::InvalidConfig | Self::BudgetExceeded => {
                crate::fault::Fault::new("app.bad-request", "投影配置或上下文预算不合法")
            }
        }
    }
}
/// 计划明确省略区间和压缩需求，不把裁剪称为完整记忆。
pub struct PromptPlan {
    pub(super) working_set_hash: String,
    pub input: ProviderInput,
    pub guard: mythos_llm::guard::GuardSpec,
    pub guard_spec_id: String,
    pub estimate: u32,
    pub estimator_version: u32,
    pub(super) folded: Vec<u64>,
    pub(super) included: Vec<u64>,
    pub omitted_ranges: Vec<SeqRange>,
    pub compression_needed: bool,
}
impl PromptPlan {
    pub fn included_sequences(&self) -> &[u64] {
        &self.included
    }
    pub fn folded_sequences(&self) -> &[u64] {
        &self.folded
    }
}

fn framed(tag: &str, text: &str) -> String {
    format!(
        "[MYTHOS:{tag}]\n{}\n[/MYTHOS:{}]\n",
        grammar::data(text),
        tag.split_whitespace().next().unwrap_or(tag)
    )
}
fn render(record: &Parsed) -> Option<String> {
    match &record.body.as_ref()?.body {
        Body::PlayerSpeech {
            player_id,
            text,
            mode,
            content_range,
        } => {
            let text = content_range.map_or(text.as_str(), |range| &text[range.start..range.end]);
            let mut out = String::new();
            if *mode == Some(InputMode::OutOfCharacter) {
                out.push_str(&framed("CONTEXT", "玩家输入模式：outOfCharacter"));
            }
            out.push_str(&framed(&format!("PLAYER id={player_id}"), text));
            Some(out)
        }
        Body::Narration {
            text,
            terminal: Terminal::Completed { .. },
            ..
        } => Some(framed("NARRATION", text)),
        Body::CharacterSpeech {
            speaker_id,
            text,
            terminal: Terminal::Completed { .. },
            ..
        } => Some(framed(&format!("CHARACTER id={speaker_id}"), text)),
        Body::Dice {
            expression, total, ..
        } => Some(framed(
            "CONTEXT",
            &format!("骰判 {expression}，合计 {total}。"),
        )),
        Body::Check { dc, result, .. } => Some(framed(
            "CONTEXT",
            &format!("检定：DC {dc:?}，结果 {result:?}。"),
        )),
        Body::System { code, message, .. } if code != "orphan" => Some(framed("CONTEXT", message)),
        _ => None,
    }
}
fn fold(text: &str, limit: u32, revision: &EstimatorRevision) -> String {
    let paragraphs = text.split("\n\n").collect::<Vec<_>>();
    let mut head = String::new();
    let mut tail = String::new();
    let mut used = 0;
    for paragraph in &paragraphs {
        let cost = revision.scale(estimate(paragraph, 0));
        if used + cost > limit / 3 {
            break;
        }
        head.push_str(paragraph);
        head.push_str("\n\n");
        used += cost;
    }
    used = 0;
    for paragraph in paragraphs.iter().rev() {
        let cost = revision.scale(estimate(paragraph, 0));
        if used + cost > limit / 3 {
            break;
        }
        tail.insert_str(0, &format!("{paragraph}\n\n"));
        used += cost;
    }
    format!("{head}（正文已折叠，省略中间内容；原记录完整保留）\n\n{tail}")
}
fn ranges(values: impl Iterator<Item = u64>) -> Vec<SeqRange> {
    let mut out: Vec<SeqRange> = Vec::new();
    for seq in values {
        if let Some(last) = out.last_mut()
            && seq == last.through + 1
        {
            last.through = seq;
            continue;
        }
        out.push(SeqRange {
            from: seq,
            through: seq,
        });
    }
    out
}
fn materialize(
    header: &Header,
    history: &str,
    world: &str,
    target: &Target,
    shape: Shape,
) -> ProviderInput {
    let context = format!("{history}{world}");
    match shape {
        Shape::Completion => ProviderInput::Completion(CompletionInput {
            prompt: format!("{}{context}{}", header.static_prefix, grammar::open(target)),
        }),
        Shape::Chat | Shape::ChatPrefix => {
            let content = if shape == Shape::Chat {
                format!(
                    "{context}生成 {} 选定块，仅返回正文，不返回结构标记。",
                    grammar::open(target).trim_end()
                )
            } else {
                context
            };
            ProviderInput::Chat(ChatInput {
                messages: vec![
                    ChatMessage {
                        role: ChatRole::System,
                        content: header.static_prefix.clone(),
                    },
                    ChatMessage {
                        role: ChatRole::User,
                        content,
                    },
                ],
                assistant_prefix: (shape == Shape::ChatPrefix).then(|| grammar::open(target)),
            })
        }
    }
}
fn scaled_estimate(revision: &EstimatorRevision, input: &ProviderInput) -> u32 {
    revision.scale(estimate_input(input))
}

fn require_budget(estimate: u32, limit: u32) -> Result<(), ProjectionError> {
    if estimate > limit {
        Err(ProjectionError::BudgetExceeded)
    } else {
        Ok(())
    }
}

/// 按冻结前缀、recap、逐字尾部、世界上下文顺序投影。
/// # Errors
/// 未知窗口、只读事实、非法 recap 或必须保留的最新块超预算时本地拒绝。
pub fn project(
    record_view: &RecordView<'_>,
    world_view: &WorldView<'_>,
    budget: &Budget,
    target: &Target,
    shape: Shape,
) -> Result<PromptPlan, ProjectionError> {
    if record_view.needs_recovery || world_view.needs_recovery {
        return Err(ProjectionError::NeedsRecovery);
    }
    let header = record_view.header;
    let records = record_view.records;
    let world = world_view.context;
    if records.windows(2).any(|pair| pair[0].seq >= pair[1].seq) {
        return Err(ProjectionError::InvalidConfig);
    }
    header
        .validate()
        .map_err(|_| ProjectionError::InvalidConfig)?;
    if target
        .speaker()
        .is_some_and(|id| !super::format::valid_id(id))
    {
        return Err(ProjectionError::InvalidConfig);
    }
    let limit = budget
        .context_limit
        .and_then(|n| n.checked_sub(budget.max_output_tokens)?.checked_sub(2048))
        .ok_or(ProjectionError::InvalidConfig)?
        .min(budget.input_hard_limit)
        .min(32768);
    if limit == 0
        || budget.max_output_tokens == 0
        || budget.tail == 0
        || budget.recap == 0
        || budget.folding == 0
        || budget.estimator.scale(estimate(&header.static_prefix, 0)) > budget.static_prefix
    {
        return Err(ProjectionError::InvalidConfig);
    }
    if records.iter().any(|r| r.read_only) {
        return Err(ProjectionError::ReadOnly);
    }
    let world = if world.is_empty() {
        String::new()
    } else {
        framed("CONTEXT", world)
    };
    if budget.estimator.scale(estimate(&world, 0)) > budget.world {
        return Err(ProjectionError::BudgetExceeded);
    }
    let mut fragments = BTreeMap::new();
    let mut recaps = Vec::new();
    let mut covered = BTreeSet::new();
    let mut recap_through = 0;
    for record in records {
        if let Some(Body::Recap {
            from_seq,
            through_seq,
            text,
            source_hash,
            ..
        }) = record.body.as_ref().map(|r| &r.body)
        {
            let verified = record_view
                .verified_recaps
                .get(&record.seq)
                .is_some_and(|hash| *hash == super::format::hash(record.raw.as_bytes()));
            let source = records
                .iter()
                .filter(|r| {
                    r.seq >= *from_seq
                        && r.seq <= *through_seq
                        && !matches!(r.body.as_ref().map(|r| &r.body), Some(Body::Recap { .. }))
                })
                .collect::<Vec<_>>();
            let raw = source.iter().map(|r| r.raw.as_str()).collect::<String>();
            if *from_seq <= recap_through
                || (!verified
                    && (source.first().is_none_or(|r| r.seq != *from_seq)
                        || source.last().is_none_or(|r| r.seq != *through_seq)
                        || super::format::hash(raw.as_bytes()) != *source_hash))
            {
                return Err(ProjectionError::CorruptRecap);
            }
            recap_through = *through_seq;
            recaps.push((
                record.seq,
                framed(
                    &format!("RECAP from={from_seq} through={through_seq}"),
                    text,
                ),
                records
                    .iter()
                    .filter(|r| r.seq >= *from_seq && r.seq <= *through_seq)
                    .map(|r| r.seq)
                    .collect::<Vec<_>>(),
                (*from_seq, *through_seq),
            ));
        } else if let Some(fragment) = render(record) {
            fragments.insert(record.seq, fragment);
        }
    }
    let latest = fragments.keys().next_back().copied();
    let player = records
        .iter()
        .rev()
        .find(|r| {
            matches!(
                r.body.as_ref().map(|r| &r.body),
                Some(Body::PlayerSpeech { .. })
            )
        })
        .map(|r| r.seq);
    let mut selected = BTreeMap::new();
    let mut tail: u32 = 0;
    // 必需块先占预算，避免可选历史使当前输入被错误拒绝。
    for seq in [latest, player]
        .into_iter()
        .flatten()
        .collect::<BTreeSet<_>>()
    {
        let fragment = &fragments[&seq];
        let cost = budget.estimator.scale(estimate(fragment, 0));
        if tail.saturating_add(cost) > budget.tail {
            return Err(ProjectionError::BudgetExceeded);
        }
        selected.insert(seq, fragment.clone());
        tail += cost;
    }
    let required_text = selected.values().map(String::as_str).collect::<String>();
    if scaled_estimate(
        &budget.estimator,
        &materialize(header, &required_text, &world, target, shape),
    ) > limit
    {
        return Err(ProjectionError::BudgetExceeded);
    }
    let mut recap_text = String::new();
    let mut recap_cost = 0;
    let mut recap_ids = Vec::new();
    let mut covered_ranges = Vec::new();
    for (seq, text, source, range) in recaps.iter().rev() {
        if source.iter().any(|seq| selected.contains_key(seq)) {
            continue;
        }
        let cost = budget.estimator.scale(estimate(text, 0));
        if recap_cost + cost > budget.recap {
            continue;
        }
        let candidate = format!(
            "{text}{recap_text}{}",
            selected.values().map(String::as_str).collect::<String>()
        );
        if scaled_estimate(
            &budget.estimator,
            &materialize(header, &candidate, &world, target, shape),
        ) > limit
        {
            continue;
        }
        recap_text.insert_str(0, text);
        recap_cost += cost;
        recap_ids.push(*seq);
        covered.extend(source.iter().copied());
        covered_ranges.push(*range);
    }
    for (&seq, fragment) in fragments.iter().rev() {
        if selected.contains_key(&seq) || covered.contains(&seq) {
            continue;
        }
        let cost = budget.estimator.scale(estimate(fragment, 0));
        if tail.saturating_add(cost) > budget.tail {
            continue;
        }
        selected.insert(seq, fragment.clone());
        let candidate = format!(
            "{recap_text}{}",
            selected.values().map(String::as_str).collect::<String>()
        );
        if scaled_estimate(
            &budget.estimator,
            &materialize(header, &candidate, &world, target, shape),
        ) > limit
        {
            selected.remove(&seq);
        } else {
            tail += cost;
        }
    }
    let mut folded = Vec::new();
    for record in records.iter().rev() {
        if selected.contains_key(&record.seq)
            || covered.contains(&record.seq)
            || Some(record.seq) == player
            || Some(record.seq) == latest
        {
            continue;
        }
        let Some(mut copy) = record.body.clone() else {
            continue;
        };
        let text = match &mut copy.body {
            Body::Narration {
                text,
                terminal: Terminal::Completed { .. },
                ..
            }
            | Body::CharacterSpeech {
                text,
                terminal: Terminal::Completed { .. },
                ..
            } => text,
            _ => continue,
        };
        if budget.estimator.scale(estimate(text, 0)) <= budget.folding {
            continue;
        }
        *text = fold(text, budget.folding, &budget.estimator);
        let mut parsed = record.clone();
        parsed.body = Some(copy);
        let fragment = render(&parsed).expect("folding only completed narrative records");
        let candidate = format!(
            "{recap_text}{}{}",
            selected.values().map(String::as_str).collect::<String>(),
            fragment
        );
        if scaled_estimate(
            &budget.estimator,
            &materialize(header, &candidate, &world, target, shape),
        ) <= limit
        {
            selected.insert(record.seq, fragment);
            folded.push(record.seq);
        }
    }
    let history = format!(
        "{recap_text}{}",
        selected.values().map(String::as_str).collect::<String>()
    );
    let input = materialize(header, &history, &world, target, shape);
    let count = scaled_estimate(&budget.estimator, &input);
    require_budget(count, limit)?;
    let untrimmed = fragments.values().map(String::as_str).collect::<String>();
    let compression_needed = scaled_estimate(
        &budget.estimator,
        &materialize(header, &untrimmed, &world, target, shape),
    ) > limit * 3 / 4;
    let mut omitted_ranges = ranges(
        fragments
            .keys()
            .filter(|seq| {
                (!selected.contains_key(seq) || folded.contains(seq)) && !covered.contains(seq)
            })
            .copied(),
    );
    covered_ranges.sort_unstable();
    for omitted in record_view.omitted_ranges {
        let mut from = omitted.from;
        for &(start, end) in &covered_ranges {
            if end < from || start > omitted.through {
                continue;
            }
            if start > from {
                omitted_ranges.push(SeqRange {
                    from,
                    through: start - 1,
                });
            }
            from = from.max(end.saturating_add(1));
        }
        if from <= omitted.through {
            omitted_ranges.push(SeqRange {
                from,
                through: omitted.through,
            });
        }
    }
    omitted_ranges.sort_by_key(|range| range.from);
    let mut included = selected.keys().copied().collect::<Vec<_>>();
    included.extend(recap_ids);
    included.sort_unstable();
    Ok(PromptPlan {
        working_set_hash: super::working_set::fingerprint(header, records),
        input,
        guard: grammar::guard(target),
        guard_spec_id: grammar::guard_id(target),
        estimate: count,
        estimator_version: budget.estimator.version(),
        folded,
        included,
        omitted_ranges,
        compression_needed,
    })
}

#[cfg(test)]
mod tests;
