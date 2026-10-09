//! 最小 Markdown 原文格式；不把文章中的数值、规则或代码块解释为执行权。

pub const MAX_BYTES: usize = 64 * 1024;
/// 原文保留全部 UTF-8 字节语义；身份由注册资源目录提供，不从正文猜测。
#[derive(Debug, Clone)]
pub struct Scenario {
    pub script_id: String,
    pub title: String,
    pub body: String,
}
/// 静态错误不包含正文、路径或第三方解码诊断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    SizeLimit,
    InvalidUtf8,
    InvalidId,
    InvalidHeader,
    InvalidText,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SizeLimit => "scenario exceeds byte limit",
            Self::InvalidUtf8 => "scenario is not UTF-8",
            Self::InvalidId => "invalid scenario identity",
            Self::InvalidHeader => "scenario requires a nonempty H1 title",
            Self::InvalidText => "invalid scenario text",
        })
    }
}
impl std::error::Error for Error {}
/// 读取注册身份的有界原文；要求首行 H1，不执行或重写正文。
/// # Errors
/// 容量、UTF-8、身份、标题、控制字符及保留结构标记不合法拒绝。
pub fn decode(script_id: &str, bytes: &[u8]) -> Result<Scenario, Error> {
    if bytes.len() > MAX_BYTES {
        return Err(Error::SizeLimit);
    }
    if !valid_id(script_id) {
        return Err(Error::InvalidId);
    }
    let body = std::str::from_utf8(bytes).map_err(invalid_utf8)?;
    let title = body
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("# "))
        .ok_or(Error::InvalidHeader)?
        .trim();
    if title.is_empty() || title.len() > 256 || title.chars().any(char::is_control) {
        return Err(Error::InvalidHeader);
    }
    if body
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        || body.contains("[AOIDOS:")
        || body.contains("[/AOIDOS:")
    {
        return Err(Error::InvalidText);
    }
    Ok(Scenario {
        script_id: script_id.into(),
        title: title.into(),
        body: body.into(),
    })
}
fn invalid_utf8(_: std::str::Utf8Error) -> Error {
    Error::InvalidUtf8
}
/// 身份不消毒或近似匹配，跨平台设备名统一拒绝。
pub fn valid_id(id: &str) -> bool {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || id.starts_with('-')
        || id.ends_with('-')
    {
        return false;
    }
    if matches!(id, "con" | "prn" | "aux" | "nul") {
        return false;
    }
    !(id.len() == 4
        && (id.starts_with("com") || id.starts_with("lpt"))
        && matches!(id.as_bytes()[3], b'1'..=b'9'))
}
#[cfg(test)]
mod tests;
