//! 语义 token 的唯一白名单及前后端共用标识。

use serde::Serialize;
use std::collections::BTreeMap;

include!(concat!(env!("OUT_DIR"), "/theme_css_files.rs"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Color,
    GradientList,
    PixelLength,
    Duration,
    Easing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: ValueKind,
    pub skin_writable: bool,
    pub alias: Option<&'static str>,
}

pub const TOKENS: [Token; 18] = [
    Token {
        id: "C01",
        name: "--ink",
        kind: ValueKind::Color,
        skin_writable: true,
        alias: Some("--color-ink"),
    },
    Token {
        id: "C02",
        name: "--muted",
        kind: ValueKind::Color,
        skin_writable: true,
        alias: Some("--color-muted"),
    },
    Token {
        id: "C03",
        name: "--accent",
        kind: ValueKind::Color,
        skin_writable: true,
        alias: Some("--color-accent"),
    },
    Token {
        id: "C04",
        name: "--accent-violet",
        kind: ValueKind::Color,
        skin_writable: true,
        alias: Some("--color-accent-violet"),
    },
    Token {
        id: "C05",
        name: "--accent-warm",
        kind: ValueKind::Color,
        skin_writable: true,
        alias: Some("--color-accent-warm"),
    },
    Token {
        id: "C06",
        name: "--panel",
        kind: ValueKind::Color,
        skin_writable: true,
        alias: Some("--color-panel"),
    },
    Token {
        id: "C07",
        name: "--bubble-user",
        kind: ValueKind::Color,
        skin_writable: true,
        alias: Some("--color-bubble-user"),
    },
    Token {
        id: "C08",
        name: "--bubble-persona",
        kind: ValueKind::Color,
        skin_writable: true,
        alias: Some("--color-bubble-persona"),
    },
    Token {
        id: "C09",
        name: "--hairline",
        kind: ValueKind::Color,
        skin_writable: true,
        alias: Some("--color-hairline"),
    },
    Token {
        id: "C10",
        name: "--app-bg",
        kind: ValueKind::GradientList,
        skin_writable: true,
        alias: None,
    },
    Token {
        id: "C11",
        name: "--led-active",
        kind: ValueKind::Color,
        skin_writable: false,
        alias: Some("--color-led-active"),
    },
    Token {
        id: "C12",
        name: "--diff-add",
        kind: ValueKind::Color,
        skin_writable: false,
        alias: Some("--color-diff-add"),
    },
    Token {
        id: "C13",
        name: "--diff-remove",
        kind: ValueKind::Color,
        skin_writable: false,
        alias: Some("--color-diff-remove"),
    },
    Token {
        id: "C14",
        name: "--warning",
        kind: ValueKind::Color,
        skin_writable: false,
        alias: Some("--color-warning"),
    },
    Token {
        id: "C15",
        name: "--danger",
        kind: ValueKind::Color,
        skin_writable: false,
        alias: Some("--color-danger"),
    },
    Token {
        id: "S01",
        name: "--record-left",
        kind: ValueKind::PixelLength,
        skin_writable: true,
        alias: Some("--spacing-record-left"),
    },
    Token {
        id: "M01",
        name: "--ease-signature",
        kind: ValueKind::Easing,
        skin_writable: true,
        alias: None,
    },
    Token {
        id: "M02",
        name: "--dur-micro",
        kind: ValueKind::Duration,
        skin_writable: true,
        alias: None,
    },
];

/// 按精确、大小写敏感的语义名称查找目录项。
#[must_use]
pub fn find(name: &str) -> Option<&'static Token> {
    TOKENS.iter().find(|token| token.name == name)
}

/// 列出构建时从主题目录发现的主题文件；目录中的每个 CSS 文件都是一个主题。
#[must_use]
pub fn theme_files() -> &'static [(&'static str, &'static str)] {
    THEME_CSS_FILES
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeInfo {
    pub id: String,
    pub name: String,
    pub color_scheme: String,
}

/// 返回从主题 CSS 目录发现且符合统一格式的主题选项。
#[must_use]
pub fn themes() -> Option<Vec<ThemeInfo>> {
    parse_themes(THEME_CSS_FILES)
}

fn parse_themes(files: &[(&str, &str)]) -> Option<Vec<ThemeInfo>> {
    let mut themes = Vec::with_capacity(files.len());
    for (id, source) in files {
        if !valid_theme_id(id) {
            return None;
        }
        let comment = source.strip_prefix("/*")?.split_once("*/")?.0.trim();
        let (color_scheme, _) = theme_values(source)?;
        themes.push(ThemeInfo {
            id: (*id).to_owned(),
            name: comment.to_owned(),
            color_scheme,
        });
    }
    Some(themes)
}

/// 验证主题 ID 使用稳定的小写 ASCII slug。
#[must_use]
pub fn valid_theme_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id.as_bytes()[id.len() - 1].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !id.contains("--")
}

/// 解析受信任的内置主题 CSS 根规则，返回 `color-scheme` 与语义 token。
#[must_use]
pub fn theme_values(source: &str) -> Option<(String, BTreeMap<String, String>)> {
    let source = source.replace(['\n', '\r'], " ");
    let source = source.trim();
    let source = if let Some(comment) = source.strip_prefix("/*") {
        source[comment.find("*/")? + 4..].trim()
    } else {
        source
    };
    let body = source.strip_prefix(":root {")?.strip_suffix('}')?;
    let mut scheme = None;
    let mut values = BTreeMap::new();
    for declaration in body
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let (name, value) = declaration.split_once(':')?;
        let name = name.trim();
        let value = value.trim();
        if name == "color-scheme" {
            if !matches!(value, "light" | "dark") || scheme.replace(value.to_owned()).is_some() {
                return None;
            }
            continue;
        }
        let token = find(name)?;
        if !crate::skin::is_valid_theme_value(token.kind, value)
            || values.insert(name.to_owned(), value.to_owned()).is_some()
        {
            return None;
        }
    }
    (scheme.is_some() && values.len() == TOKENS.len()).then(|| (scheme.unwrap(), values))
}

#[cfg(test)]
mod tests;
