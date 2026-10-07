//! 增量护栏：GuardSpec 编译与跨分片安全交付。
//!
//! 算法（设计见 ai-docs/architecture/llm.md「跨分片算法与黄金样例」）：
//! Text 先做 CRLF / CR → LF 规范化（尾部待判定 CR 扣留），与未交付尾部拼接后
//! 找最早**结束位置**的完整命中（同结束位置取最长规则，再按稳定优先级），在命中**起点**截断；
//! 无完整命中时只交付确定安全的前缀，保留是有效规则前缀的最长后缀及待判定 CR。
//! 正常结束时单独 CR 转 LF、疑似规则前缀保守丢弃并按 guard 收尾；取消 / 错误直接丢弃尾部。

/// 单条规则 pattern 的字节上限（v1 本地资源上限）。
pub const MAX_RULE_BYTES: usize = 512;
/// 规则条数上限（含全部变体）。
pub const MAX_RULES: usize = 32;

/// pattern 的匹配锚定：`Anywhere` 任意位置；`LineStart` 仅输出起点或 LF 后第一位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Anywhere,
    LineStart,
}

/// 一条护栏规则。pattern 为记录语法定义的固定字符串，不承载用户正则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardRule {
    pub id: String,
    pub pattern: String,
    pub anchor: Anchor,
    /// 数值越小优先级越高；同 pattern 同锚定合并时保留最小值。
    pub priority: u16,
    /// 是否允许进入服务端 stop；行首语义仅客户端执行，服务端只能按子串匹配。
    pub server_eligible: bool,
}

/// `GuardSpec` 编译失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardSpecError {
    EmptyPattern { id: String },
    CrInPattern { id: String },
    PatternTooLong { id: String, bytes: usize },
    TooManyRules { count: usize },
    DuplicateId { id: String },
}

impl std::fmt::Display for GuardSpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyPattern { id } => write!(f, "guard rule {id:?} has an empty pattern"),
            Self::CrInPattern { id } => write!(f, "guard rule {id:?} pattern contains CR"),
            Self::PatternTooLong { id, bytes } => {
                write!(
                    f,
                    "guard rule {id:?} pattern is {bytes} bytes, limit {MAX_RULE_BYTES}"
                )
            }
            Self::TooManyRules { count } => {
                write!(f, "guard spec has {count} rules, limit {MAX_RULES}")
            }
            Self::DuplicateId { id } => write!(f, "duplicate guard rule id {id:?}"),
        }
    }
}

impl std::error::Error for GuardSpecError {}

/// 编译后的规则：同锚定同 pattern 合并，按（优先级、声明顺序）稳定排序。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledRule {
    pub pattern: String,
    pub anchor: Anchor,
    pub priority: u16,
    pub server_eligible: bool,
}

/// 编译产物，回合内可复用；运行态（待判定尾部）在 [`Guard`]，不进 spec。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuardSpec {
    rules: Vec<CompiledRule>,
}

impl GuardSpec {
    /// 编译：校验上限与 id 唯一性，合并同锚定同 pattern 的规则。
    ///
    /// # Errors
    /// 空 pattern、含 CR、超长、超量或重复 id 时返回对应 [`GuardSpecError`]。
    pub fn compile(rules: Vec<GuardRule>) -> Result<Self, GuardSpecError> {
        let mut seen_ids = std::collections::HashSet::new();
        for rule in &rules {
            if !seen_ids.insert(rule.id.as_str()) {
                return Err(GuardSpecError::DuplicateId {
                    id: rule.id.clone(),
                });
            }
            if rule.pattern.is_empty() {
                return Err(GuardSpecError::EmptyPattern {
                    id: rule.id.clone(),
                });
            }
            if rule.pattern.contains('\r') {
                return Err(GuardSpecError::CrInPattern {
                    id: rule.id.clone(),
                });
            }
            let bytes = rule.pattern.len();
            if bytes > MAX_RULE_BYTES {
                return Err(GuardSpecError::PatternTooLong {
                    id: rule.id.clone(),
                    bytes,
                });
            }
        }
        if rules.len() > MAX_RULES {
            return Err(GuardSpecError::TooManyRules { count: rules.len() });
        }
        let mut compiled: Vec<CompiledRule> = Vec::new();
        for rule in rules {
            let merged = compiled
                .iter_mut()
                .find(|c| c.anchor == rule.anchor && c.pattern == rule.pattern);
            match merged {
                // 同锚定同 pattern：保留最高优先级；server 资格不因合并放宽。
                Some(existing) => {
                    existing.priority = existing.priority.min(rule.priority);
                    existing.server_eligible &= rule.server_eligible;
                }
                None => compiled.push(CompiledRule {
                    pattern: rule.pattern,
                    anchor: rule.anchor,
                    priority: rule.priority,
                    server_eligible: rule.server_eligible,
                }),
            }
        }
        // 稳定排序：优先级升序，同优先级保持声明顺序。
        compiled.sort_by_key(|r| r.priority);
        Ok(Self { rules: compiled })
    }

    #[must_use]
    pub fn rules(&self) -> &[CompiledRule] {
        &self.rules
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// 最长规则的字节长度；扣留上界由此决定。
    #[must_use]
    pub fn max_pattern_len(&self) -> usize {
        self.rules
            .iter()
            .map(|r| r.pattern.len())
            .max()
            .unwrap_or(0)
    }

    /// 服务端 stop `子集：server_eligible` 规则按优先级取不重复 pattern 的前 `limit` 项。
    /// 锚定语义不进服务端，调用方须已知服务端只按子串 stop。
    #[must_use]
    pub fn server_stops(&self, limit: usize) -> Vec<String> {
        let mut stops: Vec<String> = Vec::new();
        for rule in &self.rules {
            if !rule.server_eligible {
                continue;
            }
            if stops.iter().any(|s| s == &rule.pattern) {
                continue;
            }
            if stops.len() == limit {
                break;
            }
            stops.push(rule.pattern.clone());
        }
        stops
    }
}

/// 单次推送的处理结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardStep {
    /// 无完整命中：可立即交付的安全增量（可为空），护栏继续。
    Delta(String),
    /// 命中完整 stop：`delivered` 为命中点之前的正文；此后必须停止上游，收尾记 guard。
    Stopped { delivered: String },
}

/// 正常结束（收到合法 finish）时冲刷护栏的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardFinish {
    /// EOF 冲刷出的最后一段安全正文（可为空）。
    pub tail: String,
    /// 是否因丢弃疑似 stop 前缀而按 guard 收尾。
    pub truncated: bool,
}

/// 护栏运行态：未交付尾部 + 尾部起点的行状态。回合内单线程使用，不跨 attempt 复用。
#[derive(Debug, Clone)]
pub struct Guard {
    spec: GuardSpec,
    pending: String,
    /// `pending` 起始位置是否处于行首（输出起点或 LF 之后）。
    at_line_start: bool,
}

impl Guard {
    #[must_use]
    pub fn new(spec: GuardSpec) -> Self {
        Self {
            spec,
            pending: String::new(),
            at_line_start: true,
        }
    }

    #[must_use]
    pub fn spec(&self) -> &GuardSpec {
        &self.spec
    }

    /// 测试观测：未交付尾部的字节数。护栏只扣留可能匹配的尾部，不缓存整行。
    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// 喂入一段模型正文（已由传输层解码为完整字符串）。
    pub fn push(&mut self, text: &str) -> GuardStep {
        let mut source = std::mem::take(&mut self.pending);
        source.push_str(text);
        let (combined, trailing_cr) = normalize_lf(&source);
        if let Some(hit) = self.find_complete_hit(&combined) {
            self.pending.clear();
            let delivered = combined[..hit.start].to_owned();
            return GuardStep::Stopped { delivered };
        }
        let retain_from = self.find_retention(&combined, trailing_cr);
        let delivered = combined[..retain_from].to_owned();
        self.pending = combined[retain_from..].to_owned();
        if retain_from > 0 {
            self.at_line_start = combined.as_bytes()[retain_from - 1] == b'\n';
        }
        GuardStep::Delta(delivered)
    }

    /// 正常结束：单独 CR 转 LF，重查命中后丢弃疑似规则前缀。
    pub fn finish(&mut self) -> GuardFinish {
        let mut source = std::mem::take(&mut self.pending);
        if source.ends_with('\r') {
            source.pop();
            source.push('\n');
        }
        let (combined, _) = normalize_lf(&source);
        if let Some(hit) = self.find_complete_hit(&combined) {
            return GuardFinish {
                tail: combined[..hit.start].to_owned(),
                truncated: true,
            };
        }
        let retain_from = self.find_retention(&combined, false);
        GuardFinish {
            tail: combined[..retain_from].to_owned(),
            truncated: retain_from < combined.len(),
        }
    }

    /// 取消 / 错误：未交付尾部直接丢弃，不冲刷。
    pub fn discard(&mut self) {
        self.pending.clear();
    }

    /// 在缓冲中找最优完整命中：比较（结束位置, 规则长度降序, 优先级, 声明顺序）。
    fn find_complete_hit(&self, combined: &str) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        for (order, rule) in self.spec.rules.iter().enumerate() {
            let mut from = 0;
            while let Some(found) = combined[from..].find(&rule.pattern) {
                let start = from + found;
                if self.anchor_ok(rule, combined, start) {
                    let hit = Hit {
                        end: start + rule.pattern.len(),
                        start,
                        len: rule.pattern.len(),
                        priority: rule.priority,
                        order,
                    };
                    if best.is_none_or(|b| hit.beats(b)) {
                        best = Some(hit);
                    }
                    break;
                }
                // 跳过一个字符继续找同规则的后续出现，覆盖自重叠 pattern。
                from = combined[start..]
                    .char_indices()
                    .nth(1)
                    .map_or(combined.len(), |(offset, _)| start + offset);
            }
        }
        best
    }

    /// 找最长扣留后缀的起点：后缀须是某适用规则的真前缀（锚定符合真实位置）。
    /// 返回字节起点；找不到时返回 `combined.len()`（全部可交付）。
    fn find_retention(&self, combined: &str, trailing_cr: bool) -> usize {
        let max_len = self.spec.max_pattern_len();
        let mut best = combined.len();
        if trailing_cr {
            // 待判定 CR 永远扣留；规则不含 CR，扣留起点不可能晚于 CR。
            best = combined.len() - '\r'.len_utf8();
        }
        for (pos, _) in combined.char_indices() {
            if combined.len() - pos > max_len {
                continue;
            }
            let suffix = &combined[pos..];
            for rule in &self.spec.rules {
                if suffix.len() >= rule.pattern.len() {
                    continue;
                }
                if !rule.pattern.starts_with(suffix) {
                    continue;
                }
                if !self.anchor_ok(rule, combined, pos) {
                    continue;
                }
                return pos.min(best);
            }
        }
        best
    }

    /// 锚定校验：LineStart 只在输出起点或 LF 后第一位置成立。
    fn anchor_ok(&self, rule: &CompiledRule, combined: &str, start: usize) -> bool {
        match rule.anchor {
            Anchor::Anywhere => true,
            Anchor::LineStart => {
                start == 0 && self.at_line_start
                    || start > 0 && combined.as_bytes()[start - 1] == b'\n'
            }
        }
    }
}

/// 一次完整命中的比较键。
#[derive(Debug, Clone, Copy)]
struct Hit {
    end: usize,
    start: usize,
    len: usize,
    priority: u16,
    order: usize,
}

impl Hit {
    /// 结束位置最早优先；同结束位置规则最长优先；再比优先级与声明顺序。
    fn beats(&self, other: Hit) -> bool {
        self.end < other.end
            || self.end == other.end && self.len > other.len
            || self.end == other.end && self.len == other.len && self.priority < other.priority
            || self.end == other.end
                && self.len == other.len
                && self.priority == other.priority
                && self.order < other.order
    }
}

/// CRLF / CR → LF 规范化；返回（规范化文本, 是否在尾部扣留了待判定 CR）。
fn normalize_lf(source: &str) -> (String, bool) {
    if !source.contains('\r') {
        return (source.to_owned(), false);
    }
    let mut out = String::with_capacity(source.len());
    let mut pending_cr = false;
    for ch in source.chars() {
        if pending_cr {
            pending_cr = false;
            if ch != '\n' {
                out.push('\n');
            }
        }
        if ch == '\r' {
            pending_cr = true;
        } else {
            out.push(ch);
        }
    }
    if pending_cr {
        out.push('\r');
        (out, true)
    } else {
        (out, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(rules: &[(&str, &str, Anchor, u16, bool)]) -> GuardSpec {
        GuardSpec::compile(
            rules
                .iter()
                .map(|(id, pattern, anchor, priority, server)| GuardRule {
                    id: (*id).to_owned(),
                    pattern: (*pattern).to_owned(),
                    anchor: *anchor,
                    priority: *priority,
                    server_eligible: *server,
                })
                .collect(),
        )
        .expect("fixture spec compiles")
    }

    fn fixture_spec() -> GuardSpec {
        spec(&[
            ("close", "</narration>", Anchor::Anywhere, 10, true),
            ("player", "[PLAYER]", Anchor::LineStart, 20, false),
            ("system", "### SYSTEM", Anchor::LineStart, 30, false),
        ])
    }

    #[test]
    fn compile_rejects_empty_pattern() {
        let error = GuardSpec::compile(vec![GuardRule {
            id: "a".into(),
            pattern: String::new(),
            anchor: Anchor::Anywhere,
            priority: 1,
            server_eligible: false,
        }])
        .unwrap_err();
        assert_eq!(error, GuardSpecError::EmptyPattern { id: "a".into() });
    }

    #[test]
    fn compile_rejects_cr_in_pattern() {
        let error = GuardSpec::compile(vec![GuardRule {
            id: "a".into(),
            pattern: "x\r\ny".into(),
            anchor: Anchor::Anywhere,
            priority: 1,
            server_eligible: false,
        }])
        .unwrap_err();
        assert_eq!(error, GuardSpecError::CrInPattern { id: "a".into() });
    }

    #[test]
    fn compile_rejects_overlong_pattern() {
        let error = GuardSpec::compile(vec![GuardRule {
            id: "a".into(),
            pattern: "x".repeat(MAX_RULE_BYTES + 1),
            anchor: Anchor::Anywhere,
            priority: 1,
            server_eligible: false,
        }])
        .unwrap_err();
        assert_eq!(
            error,
            GuardSpecError::PatternTooLong {
                id: "a".into(),
                bytes: MAX_RULE_BYTES + 1
            }
        );
    }

    #[test]
    fn compile_rejects_too_many_rules() {
        let rules: Vec<GuardRule> = (0..=MAX_RULES)
            .map(|i| GuardRule {
                id: format!("r{i}"),
                pattern: format!("p{i}"),
                anchor: Anchor::Anywhere,
                priority: 1,
                server_eligible: false,
            })
            .collect();
        let error = GuardSpec::compile(rules).unwrap_err();
        assert_eq!(
            error,
            GuardSpecError::TooManyRules {
                count: MAX_RULES + 1
            }
        );
    }

    #[test]
    fn compile_rejects_duplicate_id() {
        let rules = vec![
            GuardRule {
                id: "a".into(),
                pattern: "x".into(),
                anchor: Anchor::Anywhere,
                priority: 1,
                server_eligible: false,
            },
            GuardRule {
                id: "a".into(),
                pattern: "y".into(),
                anchor: Anchor::Anywhere,
                priority: 2,
                server_eligible: false,
            },
        ];
        assert_eq!(
            GuardSpec::compile(rules).unwrap_err(),
            GuardSpecError::DuplicateId { id: "a".into() }
        );
    }

    #[test]
    fn merge_keeps_highest_priority_without_widening_server() {
        let spec = GuardSpec::compile(vec![
            GuardRule {
                id: "a".into(),
                pattern: "STOP".into(),
                anchor: Anchor::Anywhere,
                priority: 20,
                server_eligible: true,
            },
            GuardRule {
                id: "b".into(),
                pattern: "STOP".into(),
                anchor: Anchor::Anywhere,
                priority: 5,
                server_eligible: true,
            },
            GuardRule {
                id: "c".into(),
                pattern: "STOP".into(),
                anchor: Anchor::Anywhere,
                priority: 9,
                server_eligible: false,
            },
        ])
        .unwrap();
        assert_eq!(spec.rules().len(), 1);
        assert_eq!(spec.rules()[0].priority, 5);
        assert!(!spec.rules()[0].server_eligible);
    }

    #[test]
    fn merge_is_scoped_to_anchor_and_pattern() {
        let spec = spec(&[
            ("a", "X", Anchor::Anywhere, 1, false),
            ("b", "X", Anchor::LineStart, 1, false),
        ]);
        assert_eq!(spec.rules().len(), 2);
    }

    /// 两种步骤的交付文本；命中时取命中前前缀，正常路径取安全增量。
    fn delivered_text(step: GuardStep) -> String {
        match step {
            GuardStep::Delta(text) => text,
            GuardStep::Stopped { delivered } => delivered,
        }
    }

    fn compile_rules(rules: Vec<GuardRule>) -> GuardSpec {
        GuardSpec::compile(rules).expect("fixture spec compiles")
    }

    fn rule(
        id: &str,
        pattern: &str,
        anchor: Anchor,
        priority: u16,
        server_eligible: bool,
    ) -> GuardRule {
        GuardRule {
            id: id.to_owned(),
            pattern: pattern.to_owned(),
            anchor,
            priority,
            server_eligible,
        }
    }

    #[test]
    fn server_stops_filter_limit_and_dedupe() {
        let spec = spec(&[
            ("a", "S1", Anchor::Anywhere, 30, true),
            ("b", "S1", Anchor::Anywhere, 1, true),
            ("c", "S2", Anchor::LineStart, 5, true),
            ("d", "S3", Anchor::Anywhere, 2, false),
            ("e", "S4", Anchor::Anywhere, 3, true),
        ]);
        assert_eq!(spec.server_stops(16), ["S1", "S4", "S2"]);
        assert_eq!(spec.server_stops(2), ["S1", "S4"]);
        assert!(spec.server_stops(0).is_empty());
        // 同 pattern 不同锚定是两条规则，服务端只取一次。
        let dup = compile_rules(vec![
            rule("f", "S5", Anchor::Anywhere, 1, true),
            rule("g", "S5", Anchor::LineStart, 2, true),
        ]);
        assert_eq!(dup.server_stops(16), ["S5"]);
    }

    #[test]
    fn empty_spec_delivers_everything() {
        let spec = GuardSpec::default();
        assert!(spec.is_empty());
        let mut guard = Guard::new(spec);
        assert!(guard.spec().is_empty());
        assert_eq!(
            guard.push("任意\r\n文本"),
            GuardStep::Delta("任意\n文本".into())
        );
        let finish = guard.finish();
        assert_eq!(finish.tail, "");
        assert!(!finish.truncated);
    }

    #[test]
    fn golden_close_tag_stops_and_drops_rest() {
        let mut guard = Guard::new(fixture_spec());
        assert_eq!(guard.push("风吹过。"), GuardStep::Delta("风吹过。".into()));
        assert_eq!(guard.push("</nar"), GuardStep::Delta(String::new()));
        assert_eq!(
            guard.push("ration>后文"),
            GuardStep::Stopped {
                delivered: String::new()
            }
        );
    }

    #[test]
    fn golden_forged_player_line_is_not_delivered() {
        let mut guard = Guard::new(fixture_spec());
        assert_eq!(
            guard.push("他举起灯。\n[PLA"),
            GuardStep::Delta("他举起灯。\n".into())
        );
        assert_eq!(
            guard.push("YER]我答应了"),
            GuardStep::Stopped {
                delivered: String::new()
            }
        );
    }

    #[test]
    fn golden_player_marker_mid_line_is_delivered() {
        let mut guard = Guard::new(fixture_spec());
        assert_eq!(
            guard.push("纸上写着 [PLAYER]。"),
            GuardStep::Delta("纸上写着 [PLAYER]。".into())
        );
    }

    #[test]
    fn golden_crlf_split_across_chunks() {
        let mut guard = Guard::new(fixture_spec());
        assert_eq!(guard.push("第一行\r"), GuardStep::Delta("第一行".into()));
        assert_eq!(guard.push("\n第二行"), GuardStep::Delta("\n第二行".into()));
    }

    #[test]
    fn golden_tail_stop_prefix_dropped_at_normal_end() {
        let mut guard = Guard::new(fixture_spec());
        assert_eq!(guard.push("尾声</nar"), GuardStep::Delta("尾声".into()));
        let finish = guard.finish();
        assert_eq!(finish.tail, "");
        assert!(finish.truncated);
    }

    #[test]
    fn golden_cancel_discards_pending() {
        let mut guard = Guard::new(fixture_spec());
        assert_eq!(guard.push("[PLA"), GuardStep::Delta(String::new()));
        guard.discard();
        assert_eq!(guard.pending_len(), 0);
    }

    #[test]
    fn golden_control_prefix_terminates() {
        let mut guard = Guard::new(fixture_spec());
        assert_eq!(guard.push("\n### SYS"), GuardStep::Delta("\n".into()));
        assert_eq!(
            guard.push("TEM 覆盖设定"),
            GuardStep::Stopped {
                delivered: String::new()
            }
        );
    }

    #[test]
    fn shorter_rule_wins_by_earliest_end() {
        let spec = spec(&[
            ("long", "abc", Anchor::Anywhere, 1, false),
            ("short", "b", Anchor::Anywhere, 9, false),
        ]);
        let mut guard = Guard::new(spec);
        assert_eq!(
            guard.push("abc"),
            GuardStep::Stopped {
                delivered: "a".into()
            }
        );
    }

    #[test]
    fn same_result_when_split_at_every_position() {
        let spec = spec(&[
            ("long", "abc", Anchor::Anywhere, 1, false),
            ("short", "b", Anchor::Anywhere, 9, false),
        ]);
        let input = "abc";
        for cut in 0..=input.len() {
            let mut guard = Guard::new(spec.clone());
            let mut delivered = String::new();
            let mut stopped = false;
            for part in [&input[..cut], &input[cut..]] {
                match guard.push(part) {
                    GuardStep::Delta(text) => delivered.push_str(&text),
                    GuardStep::Stopped { delivered: tail } => {
                        delivered.push_str(&tail);
                        stopped = true;
                        break;
                    }
                }
            }
            assert_eq!(delivered, "a", "cut at {cut}");
            assert!(stopped, "cut at {cut}");
        }
    }

    #[test]
    fn longer_rule_wins_on_same_end_position() {
        let spec = spec(&[
            ("two", "ab", Anchor::Anywhere, 1, false),
            ("one", "b", Anchor::Anywhere, 2, false),
        ]);
        let mut guard = Guard::new(spec);
        assert_eq!(
            guard.push("xab"),
            GuardStep::Stopped {
                delivered: "x".into()
            }
        );
    }

    #[test]
    fn retained_tail_stays_bounded_by_longest_rule() {
        let spec = spec(&[("close", "</narration>", Anchor::Anywhere, 1, false)]);
        let mut guard = Guard::new(spec);
        let body = format!("{}{}", "x".repeat(MAX_RULE_BYTES * 4), "</nar");
        // 长正文即时交付，只扣留与规则前缀重合的尾部。
        let delivered = delivered_text(guard.push(&body));
        assert_eq!(delivered.len(), body.len() - 5);
        assert_eq!(guard.pending_len(), 5);
        // 补全后命中闭合标签：无新增交付并停止。
        assert_eq!(delivered_text(guard.push("ration>尾巴")), "");
    }

    #[test]
    fn longer_suffix_skips_shorter_rule_before_matching_longer() {
        let spec = spec(&[
            ("short", "BZ", Anchor::Anywhere, 1, false),
            ("long", "ABCD", Anchor::Anywhere, 2, false),
        ]);
        let mut guard = Guard::new(spec);
        // 后缀 "ABC" 比 "BZ" 长且不含其完整匹配：跳过短规则，作为 "ABCD" 前缀扣留。
        assert_eq!(guard.push("xxABC"), GuardStep::Delta("xx".into()));
        assert_eq!(guard.pending_len(), 3);
    }

    #[test]
    fn line_start_prefix_at_non_line_start_is_not_retained() {
        let spec = spec(&[("player", "[PLAYER]", Anchor::LineStart, 1, false)]);
        let mut guard = Guard::new(spec);
        // "[PLA" 处于非行首：锚定不成立，不扣留不截断。
        assert_eq!(guard.push("x[PLA"), GuardStep::Delta("x[PLA".into()));
        assert_eq!(guard.pending_len(), 0);
    }

    #[test]
    fn same_end_same_length_resolved_by_priority_then_order() {
        // 同 pattern 不同锚定：结束位置与长度一致，按（优先级, 声明顺序）裁决。
        let spec = spec(&[
            ("low", "STOP", Anchor::Anywhere, 9, false),
            ("high", "STOP", Anchor::LineStart, 1, false),
        ]);
        let mut guard = Guard::new(spec);
        assert_eq!(
            guard.push("s\nSTOP"),
            GuardStep::Stopped {
                delivered: "s\n".into()
            }
        );
        let tie = compile_rules(vec![
            rule("first", "STOP", Anchor::Anywhere, 5, false),
            rule("second", "STOP", Anchor::LineStart, 5, false),
        ]);
        let mut guard = Guard::new(tie);
        assert_eq!(
            guard.push("\nSTOP"),
            GuardStep::Stopped {
                delivered: "\n".into()
            }
        );
    }

    #[test]
    fn line_start_rule_only_matches_at_line_starts() {
        let spec = spec(&[("player", "P", Anchor::LineStart, 1, false)]);
        let mut guard = Guard::new(spec);
        assert_eq!(guard.push("aP"), GuardStep::Delta("aP".into()));
        assert_eq!(
            guard.push("\nP"),
            GuardStep::Stopped {
                delivered: "\n".into()
            }
        );
    }

    #[test]
    fn line_start_at_output_start_matches() {
        let spec = spec(&[("player", "P", Anchor::LineStart, 1, false)]);
        let mut guard = Guard::new(spec);
        assert_eq!(
            guard.push("P"),
            GuardStep::Stopped {
                delivered: String::new()
            }
        );
    }

    #[test]
    fn detained_cr_does_not_match_until_next_char_arrives() {
        let spec = spec(&[("n", "\nY", Anchor::Anywhere, 1, false)]);
        let mut guard = Guard::new(spec);
        assert_eq!(guard.push("A\r"), GuardStep::Delta("A".into()));
        assert_eq!(guard.pending_len(), 1);
        // CR 后随非 LF：规范化为 "\n" 后按正文继续匹配与交付。
        assert_eq!(guard.push("X"), GuardStep::Delta("\nX".into()));
    }

    #[test]
    fn cr_followed_by_text_normalizes_then_matches() {
        let spec = spec(&[("n", "\nY", Anchor::Anywhere, 1, false)]);
        let mut guard = Guard::new(spec);
        assert_eq!(guard.push("A\r"), GuardStep::Delta("A".into()));
        assert_eq!(
            guard.push("Y"),
            GuardStep::Stopped {
                delivered: String::new()
            }
        );
    }

    #[test]
    fn lone_cr_becomes_lf_at_normal_finish_and_can_stop() {
        let spec = spec(&[("n", "\n", Anchor::Anywhere, 1, false)]);
        let mut guard = Guard::new(spec);
        assert_eq!(guard.push("A\r"), GuardStep::Delta("A".into()));
        let finish = guard.finish();
        assert_eq!(finish.tail, "");
        assert!(finish.truncated);
    }

    #[test]
    fn lone_cr_at_normal_finish_is_delivered_when_safe() {
        let spec = spec(&[("x", "X", Anchor::Anywhere, 1, false)]);
        let mut guard = Guard::new(spec);
        assert_eq!(guard.push("A\r"), GuardStep::Delta("A".into()));
        let finish = guard.finish();
        assert_eq!(finish.tail, "\n");
        assert!(!finish.truncated);
    }

    #[test]
    fn char_boundary_replay_preserves_result() {
        // 两组输入：A 含完整闭合标签（部分切割点在首段即命中）；B 从不命中。
        let cases = [
            ("他举起灯。\n[PLA</narration>尾", "他举起灯。\n[PLA"),
            ("他举起灯。\n[PLA风渡尾", "他举起灯。\n[PLA风渡尾"),
        ];
        for (input, expected) in cases {
            for cut in input.char_indices().map(|(i, _)| i).chain([input.len()]) {
                let mut guard = Guard::new(fixture_spec());
                let mut stopped = false;
                let mut delivered = match guard.push(&input[..cut]) {
                    GuardStep::Delta(text) => text,
                    GuardStep::Stopped { delivered } => {
                        stopped = true;
                        delivered
                    }
                };
                if !stopped {
                    match guard.push(&input[cut..]) {
                        GuardStep::Delta(text) => delivered.push_str(&text),
                        GuardStep::Stopped { delivered: tail } => delivered.push_str(&tail),
                    }
                }
                assert_eq!(delivered, expected, "cut at {cut}");
            }
        }
    }

    #[test]
    fn display_covers_all_variants() {
        let errors = [
            GuardSpecError::EmptyPattern { id: "a".into() },
            GuardSpecError::CrInPattern { id: "a".into() },
            GuardSpecError::PatternTooLong {
                id: "a".into(),
                bytes: 2,
            },
            GuardSpecError::TooManyRules { count: 2 },
            GuardSpecError::DuplicateId { id: "a".into() },
        ];
        for e in &errors {
            assert!(!e.to_string().is_empty());
        }
    }
}
