//! 输入模式只从原文首部解析；保存原文，投影借用 UTF-8 内容范围。

use crate::{
    fault::Fault,
    record::format::{ContentRange, InputMode},
};

pub const MAX_INPUT_BYTES: usize = 32 * 1024;

/// 只读解析结果，索引始终按 UTF-8 字节计。
#[derive(Debug, Clone, Copy)]
pub struct ParsedInput<'a> {
    pub raw: &'a str,
    pub mode: InputMode,
    pub content_range: ContentRange,
}
impl<'a> ParsedInput<'a> {
    pub fn content(&self) -> &'a str {
        &self.raw[self.content_range.start..self.content_range.end]
    }
}

/// 识别 /ooc、整条双括号及字面 slash；不 trim 或改写事实原文。
/// # Errors
/// 超限、空白输入、空场外内容或未知 / 未完成 slash 返回 bad-request。
pub fn parse_input(raw: &str) -> Result<ParsedInput<'_>, Fault> {
    if raw.len() > MAX_INPUT_BYTES || raw.trim().is_empty() {
        return Err(Fault::bad_request());
    }
    let (mode, start, end) = if raw.starts_with("\\/") {
        (InputMode::InCharacter, 1, raw.len())
    } else if raw.starts_with('/') {
        let rest = raw.strip_prefix("/ooc").ok_or_else(Fault::bad_request)?;
        if !rest.starts_with(char::is_whitespace) {
            return Err(Fault::bad_request());
        }
        let content = rest.trim_start_matches(char::is_whitespace);
        if content.trim().is_empty() {
            return Err(Fault::bad_request());
        }
        (
            InputMode::OutOfCharacter,
            raw.len() - content.len(),
            raw.len(),
        )
    } else if raw.starts_with("((") && raw.ends_with("))") {
        if raw[2..raw.len() - 2].trim().is_empty() {
            return Err(Fault::bad_request());
        }
        (InputMode::OutOfCharacter, 2, raw.len() - 2)
    } else {
        (InputMode::InCharacter, 0, raw.len())
    };
    Ok(ParsedInput {
        raw,
        mode,
        content_range: ContentRange { start, end },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_modes_keep_original_bytes_and_only_strip_confirmed_syntax() {
        for (raw, mode, content) in [
            ("走向门口。", InputMode::InCharacter, "走向门口。"),
            ("  /ooc 内容", InputMode::InCharacter, "  /ooc 内容"),
            ("/ooc \u{2003}说明 \n", InputMode::OutOfCharacter, "说明 \n"),
            (
                "(( 世界是什么？ ))",
                InputMode::OutOfCharacter,
                " 世界是什么？ ",
            ),
            ("我说((你好))", InputMode::InCharacter, "我说((你好))"),
            ("((局部)) 后文", InputMode::InCharacter, "((局部)) 后文"),
            ("\\/ooc 字面命令", InputMode::InCharacter, "/ooc 字面命令"),
            ("\\/", InputMode::InCharacter, "/"),
        ] {
            let parsed = parse_input(raw).unwrap();
            assert_eq!(parsed.raw, raw);
            assert_eq!(parsed.mode, mode);
            assert_eq!(parsed.content(), content);
            assert!(raw.is_char_boundary(parsed.content_range.start));
            assert!(raw.is_char_boundary(parsed.content_range.end));
        }
        for raw in [
            "",
            " \n",
            "/",
            "/ooc",
            "/oocx 内容",
            "/other 内容",
            "/ooc \n",
            "(())",
            "(( \n ))",
        ] {
            assert_eq!(parse_input(raw).unwrap_err().code, "app.bad-request");
        }
        assert!(parse_input(&"中".repeat(MAX_INPUT_BYTES / 3 + 1)).is_err());
        assert!(parse_input(&"a".repeat(MAX_INPUT_BYTES)).is_ok());
    }
}
