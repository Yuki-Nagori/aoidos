//! 有界增量 SSE 解析：按字符串分片喂入，产出 `data` 载荷事件。
//!
//! 自建解析器（探测结论：eventsource-stream 0.2 无事件大小上限，事件边界无法从外部观测，
//! 不能在输入侧可靠施加上限）。单个未完成 event 的内存占用以 [`MAX_EVENT_BYTES`] 为上界，
//! 超限报 [`SseError::EventTooLarge`]，由上层归入 `llm.bad-response` 并停止上游。
//! 换行接受 LF / CRLF / CR，跨分片的 CRLF 拆开也能正确合并；流首 BOM 按规范剥除。

/// 单个未完成 SSE event 的字节上界（通信契约 v1 初值）。
pub const MAX_EVENT_BYTES: usize = 1024 * 1024;

/// 一条已派发的 SSE 事件；本项目只消费 `data` 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub data: String,
}

/// SSE 流损坏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseError {
    /// 未派发的 event 缓冲超过 [`MAX_EVENT_BYTES`]。
    EventTooLarge { buffered: usize },
}

impl std::fmt::Display for SseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EventTooLarge { buffered } => {
                write!(f, "sse event exceeds {MAX_EVENT_BYTES} bytes: {buffered}")
            }
        }
    }
}

impl std::error::Error for SseError {}

/// 增量 SSE 解析器状态机。
#[derive(Debug)]
pub struct IncrementalSse {
    line: String,
    data: String,
    has_data: bool,
    pending_cr: bool,
    started: bool,
}

impl Default for IncrementalSse {
    fn default() -> Self {
        Self::new()
    }
}

impl IncrementalSse {
    pub fn new() -> Self {
        Self {
            line: String::new(),
            data: String::new(),
            has_data: false,
            pending_cr: false,
            started: false,
        }
    }

    /// 喂入一段已通过 UTF-8 解码的文本，返回本轮派发的事件（可为空）。
    ///
    /// # Errors
    /// 未派发 event 的缓冲超过 [`MAX_EVENT_BYTES`] 时返回 [`SseError::EventTooLarge`]。
    pub fn feed(&mut self, chunk: &str) -> Result<Vec<SseEvent>, SseError> {
        let mut events = Vec::new();
        let mut chars = chunk.chars().peekable();
        if !self.started {
            self.started = true;
            if chars.peek() == Some(&'\u{feff}') {
                chars.next();
            }
        }
        for ch in chars {
            if self.pending_cr {
                // 上一字符是 CR：后随 LF 合并为同一行终止符，否则 CR 独立终止上一行。
                self.pending_cr = false;
                if ch != '\n' {
                    self.terminate_line(&mut events);
                }
            }
            match ch {
                '\r' => self.pending_cr = true,
                '\n' => self.terminate_line(&mut events),
                other => self.push_char(other)?,
            }
        }
        Ok(events)
    }

    /// 流结束：按规范处理最后一条未换行的行并派发剩余事件（不会超出既有缓冲上界）。
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if self.pending_cr || !self.line.is_empty() {
            self.pending_cr = false;
            self.terminate_line(&mut events);
        }
        if self.has_data {
            events.push(self.dispatch());
        }
        events
    }

    fn push_char(&mut self, ch: char) -> Result<(), SseError> {
        self.line.push(ch);
        self.check_bound()
    }

    fn check_bound(&self) -> Result<(), SseError> {
        let buffered = self.line.len() + self.data.len();
        if buffered > MAX_EVENT_BYTES {
            Err(SseError::EventTooLarge { buffered })
        } else {
            Ok(())
        }
    }

    /// 行终止处理；缓冲上界由逐字符的 [`IncrementalSse::feed`] 检查保证
    /// （行内容并入 data 后的总和与检查时相同），本函数不再重复判定。
    fn terminate_line(&mut self, events: &mut Vec<SseEvent>) {
        let line = std::mem::take(&mut self.line);
        if line.is_empty() {
            if self.has_data {
                events.push(self.dispatch());
            }
        } else if let Some(rest) = line.strip_prefix(':') {
            // 注释行：不产出字段。
            let _ = rest;
        } else if let Some(value) = line.strip_prefix("data:") {
            let value = value.strip_prefix(' ').unwrap_or(value);
            // 规范：多条 data 行以 LF 连接，派发时剥掉最后一个换行。
            self.data.push_str(value);
            self.data.push('\n');
            // data 字段一旦出现即标志事件存在，值为空也派发空载荷。
            self.has_data = true;
        }
    }

    fn dispatch(&mut self) -> SseEvent {
        let mut data = std::mem::take(&mut self.data);
        if data.ends_with('\n') {
            data.pop();
        }
        self.has_data = false;
        SseEvent { data }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn datas(events: &[SseEvent]) -> Vec<String> {
        events.iter().map(|e| e.data.clone()).collect()
    }

    #[test]
    fn default_constructs_an_empty_parser() {
        let mut sse = IncrementalSse::default();
        let events = sse.feed("data: v\n\n").unwrap();
        assert_eq!(datas(&events), ["v"]);
    }

    #[test]
    fn single_data_event() {
        let mut sse = IncrementalSse::new();
        let events = sse.feed("data: hello\n\n").unwrap();
        assert_eq!(datas(&events), ["hello"]);
        assert!(sse.finish().is_empty());
    }

    #[test]
    fn colon_without_space_is_data() {
        let mut sse = IncrementalSse::new();
        let events = sse.feed("data:x\n\n").unwrap();
        assert_eq!(datas(&events), ["x"]);
    }

    #[test]
    fn multi_line_data_joins_with_lf() {
        let mut sse = IncrementalSse::new();
        let events = sse.feed("data: a\ndata: b\n\n").unwrap();
        assert_eq!(datas(&events), ["a\nb"]);
    }

    #[test]
    fn crlf_and_lone_cr_are_terminators() {
        let mut sse = IncrementalSse::new();
        // CR / CRLF 都终止行；无空行时多行 data 合并为一个事件（规范行为）。
        let events = sse.feed("data: a\r\ndata: b\r\r\n").unwrap();
        assert_eq!(datas(&events), ["a\nb"]);
    }

    #[test]
    fn crlf_split_across_chunks_is_one_terminator() {
        let mut sse = IncrementalSse::new();
        let first = sse.feed("data: a\r").unwrap();
        assert!(first.is_empty());
        let second = sse.feed("\ndata: b\n\n").unwrap();
        assert_eq!(datas(&second), ["a\nb"]);
    }

    #[test]
    fn lone_cr_at_chunk_end_then_lf() {
        let mut sse = IncrementalSse::new();
        assert!(sse.feed("data: x\r").unwrap().is_empty());
        // CRLF 终止首行，随后空行派发。
        let events = sse.feed("\n\n").unwrap();
        assert_eq!(datas(&events), ["x"]);
        assert!(sse.finish().is_empty());
    }

    #[test]
    fn lone_cr_followed_by_text_terminates_line() {
        let mut sse = IncrementalSse::new();
        // 两个 CR 各自终止行；EOF 冲刷时合并派发剩余 data。
        assert!(sse.feed("data: a\rdata: b\r\r").unwrap().is_empty());
        let events = sse.finish();
        assert_eq!(datas(&events), ["a\nb"]);
    }

    #[test]
    fn comments_are_ignored() {
        let mut sse = IncrementalSse::new();
        let events = sse.feed(": keep-alive\ndata: v\n\n").unwrap();
        assert_eq!(datas(&events), ["v"]);
    }

    #[test]
    fn empty_lines_without_data_do_not_dispatch() {
        let mut sse = IncrementalSse::new();
        let events = sse.feed("\n\n\n: c\n\n").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn other_fields_do_not_dispatch_alone() {
        let mut sse = IncrementalSse::new();
        let events = sse.feed("event: x\nid: 1\n\n").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn empty_data_field_dispatches_empty_payload() {
        let mut sse = IncrementalSse::new();
        let events = sse.feed("data:\n\n").unwrap();
        assert_eq!(datas(&events), [""]);
    }

    #[test]
    fn bom_at_stream_start_is_stripped() {
        let mut sse = IncrementalSse::new();
        let events = sse.feed("\u{feff}data: v\n\n").unwrap();
        assert_eq!(datas(&events), ["v"]);
    }

    #[test]
    fn bom_mid_stream_is_content() {
        let mut sse = IncrementalSse::new();
        let events = sse.feed("data: a\u{feff}b\n\n").unwrap();
        assert_eq!(datas(&events), ["a\u{feff}b"]);
    }

    #[test]
    fn finish_dispatches_trailing_partial_line() {
        let mut sse = IncrementalSse::new();
        assert!(sse.feed("data: tail").unwrap().is_empty());
        let events = sse.finish();
        assert_eq!(datas(&events), ["tail"]);
    }

    #[test]
    fn finish_with_pending_cr_terminates() {
        let mut sse = IncrementalSse::new();
        assert!(sse.feed("data: t\r").unwrap().is_empty());
        let events = sse.finish();
        assert_eq!(datas(&events), ["t"]);
    }

    #[test]
    fn oversized_event_is_rejected() {
        let mut sse = IncrementalSse::new();
        let big = "x".repeat(MAX_EVENT_BYTES + 2);
        // 逐字符入缓冲，跨过上限即报错；buffered 是当时的行缓冲长度。
        let error = sse.feed(&format!("data: {big}\n\n")).unwrap_err();
        assert_eq!(
            error,
            SseError::EventTooLarge {
                buffered: MAX_EVENT_BYTES + 1
            }
        );
    }

    #[test]
    fn bound_resets_after_dispatch() {
        let mut sse = IncrementalSse::new();
        let big = "x".repeat(MAX_EVENT_BYTES - 8);
        assert!(!sse.feed(&format!("data: {big}\n\n")).unwrap().is_empty());
        // 派发后缓冲归零，后续事件不受历史影响。
        let events = sse.feed("data: next\n\n").unwrap();
        assert_eq!(datas(&events), ["next"]);
    }

    #[test]
    fn char_boundary_replay_yields_same_events() {
        let input = "data: 风渡🌉\ndata: b\r\n\ndata: c\ndata:d\n\n";
        let mut whole = IncrementalSse::new();
        let expected = datas(&whole.feed(input).unwrap());
        for cut in input.char_indices().map(|(i, _)| i).chain([input.len()]) {
            let mut split = IncrementalSse::new();
            let mut got = datas(&split.feed(&input[..cut]).unwrap());
            got.extend(datas(&split.feed(&input[cut..]).unwrap()));
            got.extend(datas(&split.finish()));
            assert_eq!(got, expected, "cut at {cut}");
        }
    }

    #[test]
    fn display_is_implemented() {
        let error = SseError::EventTooLarge { buffered: 7 };
        assert!(!error.to_string().is_empty());
    }
}
