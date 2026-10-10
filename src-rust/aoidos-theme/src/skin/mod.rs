//! CSS 子集校验和固定目录皮肤读取。

use cssparser::{
    AtRuleParser, BasicParseError, CowRcStr, DeclarationParser, ParseError, Parser, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser, Token,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const MAX_SKIN_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkinStatus {
    Missing,
    Valid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WarningCode {
    UnknownToken,
    ProtectedToken,
    InvalidDeclaration,
    InvalidValue,
    InvalidReference,
    CyclicReference,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkinWarning {
    pub code: WarningCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skin {
    pub script_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    pub status: SkinStatus,
    pub tokens: BTreeMap<String, String>,
    pub warnings: Vec<SkinWarning>,
    pub warnings_truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkinError {
    UnknownScript,
    InvalidSkin,
    Storage,
}

#[derive(Debug)]
struct Declaration {
    name: String,
    value: String,
    line: u32,
    important: bool,
}

#[derive(Debug)]
struct Rule {
    declarations: Vec<Declaration>,
}

struct Stylesheet {
    invalid: bool,
}

impl<'i> QualifiedRuleParser<'i> for Stylesheet {
    type Prelude = ();
    type QualifiedRule = Rule;
    type Error = ();

    fn parse_prelude(
        &mut self,
        input: &mut Parser<'i>,
    ) -> Result<Self::Prelude, ParseError<Self::Error>> {
        let parsed: Result<(), ParseError<()>> = (|| {
            input.expect_colon()?;
            expect_ident(input, "root")?;
            input.expect_exhausted()?;
            Ok(())
        })();
        parsed.map_err(|_| {
            self.invalid = true;
            input.new_error_for_next_token()
        })
    }

    fn parse_block(
        &mut self,
        _root: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<Self::QualifiedRule, ParseError<Self::Error>> {
        let (declarations, invalid_structure) = parse_declarations(input);
        self.invalid |= invalid_structure;
        Ok(Rule { declarations })
    }
}

impl<'i> AtRuleParser<'i> for Stylesheet {
    type Prelude = ();
    type AtRule = Rule;
    type Error = ();
}

struct Declarations {
    nested: bool,
    at_rule: bool,
}

impl<'i> DeclarationParser<'i> for Declarations {
    type Declaration = Declaration;
    type Error = ();

    fn parse_value(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i>,
        _start: &ParserState,
    ) -> Result<Self::Declaration, ParseError<Self::Error>> {
        let value_start = input.position();
        let line = input.current_source_location().line.saturating_add(1);
        input.expect_no_error_token()?;
        let value = input.slice(value_start..input.position()).trim().to_owned();
        let important = is_important(&value);
        Ok(Declaration {
            name: name.to_string(),
            value,
            line,
            important,
        })
    }
}

impl<'i> QualifiedRuleParser<'i> for Declarations {
    type Prelude = ();
    type QualifiedRule = Declaration;
    type Error = ();

    fn parse_prelude(
        &mut self,
        input: &mut Parser<'i>,
    ) -> Result<Self::Prelude, ParseError<Self::Error>> {
        self.nested = true;
        Err(input.new_error_for_next_token())
    }
}

impl<'i> AtRuleParser<'i> for Declarations {
    type Prelude = ();
    type AtRule = Declaration;
    type Error = ();

    fn parse_prelude(
        &mut self,
        _name: CowRcStr<'i>,
        input: &mut Parser<'i>,
    ) -> Result<Self::Prelude, ParseError<Self::Error>> {
        self.at_rule = true;
        Err(input.new_error_for_next_token())
    }
}

impl<'i> RuleBodyItemParser<'i, Declaration, ()> for Declarations {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        true
    }
}

fn expect_ident<'i>(input: &mut Parser<'i>, expected: &str) -> Result<(), BasicParseError> {
    match input.next()? {
        Token::Ident(name) if name.as_ref() == expected => Ok(()),
        _ => Err(BasicParseError::unexpected_token()),
    }
}

fn is_important(value: &str) -> bool {
    value
        .trim_end()
        .rsplit_once('!')
        .is_some_and(|(_, suffix)| suffix.trim().eq_ignore_ascii_case("important"))
}

fn parse_declarations<'i>(input: &mut Parser<'i>) -> (Vec<Declaration>, bool) {
    let mut parser = Declarations {
        nested: false,
        at_rule: false,
    };
    let mut declarations = Vec::new();
    let mut invalid = false;
    for declaration in RuleBodyParser::new(input, &mut parser) {
        match declaration {
            Ok(declaration) => declarations.push(declaration),
            Err(_) => invalid = true,
        }
    }
    (declarations, invalid || parser.nested || parser.at_rule)
}

fn has_balanced_blocks(source: &str) -> bool {
    let mut input = Parser::new(source);
    input.set_nested_block_limit(8);
    scan_blocks(&mut input, source, None)
}

fn scan_blocks(input: &mut Parser<'_>, source: &str, expected_closer: Option<char>) -> bool {
    loop {
        let token_start = input.position().byte_index();
        let Ok(token) = input.next_including_whitespace_and_comments().cloned() else {
            return expected_closer
                .is_none_or(|closer| next_non_comment_char(source, token_start) == Some(closer));
        };
        let token_end = input.position().byte_index();
        match token {
            Token::Function(_) | Token::ParenthesisBlock => {
                if !input
                    .parse_nested_block(|nested| {
                        Ok::<bool, ParseError<()>>(scan_blocks(nested, source, Some(')')))
                    })
                    .unwrap_or(false)
                {
                    return false;
                }
            }
            Token::SquareBracketBlock => {
                if !input
                    .parse_nested_block(|nested| {
                        Ok::<bool, ParseError<()>>(scan_blocks(nested, source, Some(']')))
                    })
                    .unwrap_or(false)
                {
                    return false;
                }
            }
            Token::CurlyBracketBlock => {
                if !input
                    .parse_nested_block(|nested| {
                        Ok::<bool, ParseError<()>>(scan_blocks(nested, source, Some('}')))
                    })
                    .unwrap_or(false)
                {
                    return false;
                }
            }
            Token::CloseParenthesis
            | Token::CloseSquareBracket
            | Token::CloseCurlyBracket
            | Token::BadString(_)
            | Token::BadUrl(_) => return false,
            Token::Comment(_) if !source[token_start..token_end].ends_with("*/") => {
                return false;
            }
            _ => {}
        }
    }
}

fn next_non_comment_char(source: &str, mut position: usize) -> Option<char> {
    loop {
        while source
            .as_bytes()
            .get(position)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c'))
        {
            position += 1;
        }
        if source[position..].starts_with("/*") {
            let comment_end = source[position + 2..].find("*/")?;
            position += comment_end + 4;
            continue;
        }
        return source[position..].chars().next();
    }
}

/// 校验并规范化剧本皮肤；传入原文只允许来自可信资源根。
///
/// # Errors
/// 字符编码、CSS 结构或硬预算越界时整份拒绝；单条语义错误成为有界 warning。
pub fn parse(script_id: &str, source: &str) -> Result<Skin, SkinError> {
    if script_id != "mistbell" {
        return Err(SkinError::UnknownScript);
    }
    if source.len() > MAX_SKIN_BYTES || source.contains('\0') {
        return Err(SkinError::InvalidSkin);
    }
    let digest = Sha256::digest(source.as_bytes());
    let source_hash = Some(digest.iter().map(|byte| format!("{byte:02x}")).collect());
    let source = source.trim_start_matches('\u{feff}');
    if !has_balanced_blocks(source) {
        return Err(SkinError::InvalidSkin);
    }
    let mut parser = Parser::new(source);
    parser.set_nested_block_limit(8);
    let mut rules = Vec::new();
    let mut styles = Stylesheet { invalid: false };
    let parsed_rules = StyleSheetParser::new(&mut parser, &mut styles).collect::<Vec<_>>();
    for rule in parsed_rules {
        match rule {
            Ok(rule) => rules.push(rule),
            Err(_) => styles.invalid = true,
        }
        if rules.len() > 8 {
            return Err(SkinError::InvalidSkin);
        }
    }
    if styles.invalid || rules.len() > 8 {
        return Err(SkinError::InvalidSkin);
    }
    let mut warnings = Vec::new();
    let mut warnings_truncated = false;
    let mut raw = BTreeMap::<String, Declaration>::new();
    let mut total = 0;
    for rule in rules {
        for declaration in rule.declarations {
            total += 1;
            if total > 128 || declaration.value.len() > 1024 {
                return Err(SkinError::InvalidSkin);
            }
            let code = match crate::catalog::find(&declaration.name) {
                None => Some(WarningCode::UnknownToken),
                Some(token) if !token.skin_writable => Some(WarningCode::ProtectedToken),
                Some(_) if declaration.important => Some(WarningCode::InvalidDeclaration),
                Some(_) => None,
            };
            if let Some(code) = code {
                let was_full = warnings.len() == 32;
                push_warning(
                    &mut warnings,
                    SkinWarning {
                        code,
                        token: safe_token_name(&declaration.name),
                        line: Some(declaration.line),
                    },
                );
                warnings_truncated |= was_full;
                continue;
            }
            let token = crate::catalog::find(&declaration.name).expect("checked above");
            let references = match whole_var_reference(&declaration.value) {
                Some(reference) => Some(vec![(reference, token.kind)]),
                None if token.kind == crate::catalog::ValueKind::GradientList
                    && normalize_gradient(&declaration.value).is_some() =>
                {
                    Some(
                        gradient_references(&declaration.value)
                            .into_iter()
                            .map(|reference| (reference, crate::catalog::ValueKind::Color))
                            .collect(),
                    )
                }
                None => Some(Vec::new()),
            };
            if references.as_ref().is_some_and(|references| {
                references.iter().any(|(reference, expected_kind)| {
                    crate::catalog::find(reference)
                        .is_none_or(|target| target.kind != *expected_kind)
                })
            }) {
                let was_full = warnings.len() == 32;
                push_warning(
                    &mut warnings,
                    SkinWarning {
                        code: WarningCode::InvalidReference,
                        token: safe_token_name(&declaration.name),
                        line: Some(declaration.line),
                    },
                );
                warnings_truncated |= was_full;
                continue;
            }
            if whole_var_reference(&declaration.value).is_none()
                && normalize_literal(token.kind, &declaration.value).is_none()
            {
                let was_full = warnings.len() == 32;
                push_warning(
                    &mut warnings,
                    SkinWarning {
                        code: WarningCode::InvalidValue,
                        token: safe_token_name(&declaration.name),
                        line: Some(declaration.line),
                    },
                );
                warnings_truncated |= was_full;
                continue;
            }
            raw.insert(declaration.name.clone(), declaration);
        }
    }
    let (tokens, resolve_warnings_truncated) = resolve_tokens(&raw, &mut warnings)?;
    let skin = Skin {
        script_id: script_id.into(),
        source_hash,
        status: SkinStatus::Valid,
        tokens,
        warnings,
        warnings_truncated: warnings_truncated || resolve_warnings_truncated,
    };
    Ok(skin)
}

/// 从只读资源目录加载已编译登记的剧本皮肤文件。
///
/// # Errors
/// 未登记 id、符号链接、非普通文件、超限或文件错误返回静态错误类别。
pub fn load(resource_root: &std::path::Path, script_id: &str) -> Result<Skin, SkinError> {
    if script_id != "mistbell" {
        return Err(SkinError::UnknownScript);
    }
    let root = resource_root.join(script_id);
    let metadata = std::fs::symlink_metadata(&root).map_err(|_| SkinError::Storage)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(SkinError::Storage);
    }
    let path = root.join("theme.css");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => return missing_or_storage_error(error, script_id),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(SkinError::Storage);
    }
    // 目录类型和符号链接由本模块校验；有界读取与 UTF-8 解码复用存储基建。
    let source = aoidos_store::read::read_text_bounded(&path, MAX_SKIN_BYTES as u32)
        .map_err(map_read_error)?
        .ok_or(SkinError::Storage)?;
    parse(script_id, &source)
}

fn map_read_error(error: aoidos_store::error::StoreError) -> SkinError {
    match error.code() {
        "corrupt" => SkinError::InvalidSkin,
        _ => SkinError::Storage,
    }
}

fn missing_or_storage_error(error: std::io::Error, script_id: &str) -> Result<Skin, SkinError> {
    if error.kind() == std::io::ErrorKind::NotFound {
        Ok(empty_skin(script_id))
    } else {
        Err(SkinError::Storage)
    }
}

fn empty_skin(script_id: &str) -> Skin {
    Skin {
        script_id: script_id.into(),
        source_hash: None,
        status: SkinStatus::Missing,
        tokens: BTreeMap::new(),
        warnings: Vec::new(),
        warnings_truncated: false,
    }
}

fn resolve_tokens(
    declarations: &BTreeMap<String, Declaration>,
    warnings: &mut Vec<SkinWarning>,
) -> Result<(BTreeMap<String, String>, bool), SkinError> {
    let mut resolved = BTreeMap::new();
    let mut visiting = Vec::new();
    let mut truncated = false;
    for token in crate::catalog::TOKENS
        .iter()
        .filter(|token| token.skin_writable)
    {
        if !declarations.contains_key(token.name) {
            continue;
        }
        match resolve_token(token.name, declarations, &mut visiting, &mut resolved) {
            Ok(value) => {
                resolved.insert(token.name.to_owned(), value);
            }
            Err(kind) => {
                let declaration = &declarations[token.name];
                let code = match kind {
                    ResolveError::Cycle => WarningCode::CyclicReference,
                    ResolveError::Reference => WarningCode::InvalidReference,
                    ResolveError::Value => WarningCode::InvalidValue,
                };
                let before = warnings.len();
                push_warning(
                    warnings,
                    SkinWarning {
                        code,
                        token: Some(token.name.to_owned()),
                        line: Some(declaration.line),
                    },
                );
                truncated |= warnings.len() == before;
            }
        }
    }
    Ok((resolved, truncated))
}

#[derive(Clone, Copy)]
enum ResolveError {
    Cycle,
    Reference,
    Value,
}

fn resolve_token(
    name: &str,
    declarations: &BTreeMap<String, Declaration>,
    visiting: &mut Vec<String>,
    resolved: &mut BTreeMap<String, String>,
) -> Result<String, ResolveError> {
    if let Some(value) = resolved.get(name) {
        return Ok(value.clone());
    }
    if visiting.iter().any(|entry| entry == name) {
        return Err(ResolveError::Cycle);
    }
    let declaration = declarations.get(name).ok_or(ResolveError::Reference)?;
    let token = crate::catalog::find(name).ok_or(ResolveError::Reference)?;
    visiting.push(name.to_owned());
    let result = if let Some(reference) = whole_var_reference(&declaration.value) {
        resolve_reference(reference, token.kind, declarations, visiting, resolved)
    } else if token.kind == crate::catalog::ValueKind::GradientList {
        resolve_gradient_references(&declaration.value, declarations, visiting, resolved)
    } else {
        normalize_literal(token.kind, &declaration.value).ok_or(ResolveError::Value)
    };
    visiting.pop();
    result
}

fn resolve_reference(
    reference: &str,
    expected_kind: crate::catalog::ValueKind,
    declarations: &BTreeMap<String, Declaration>,
    visiting: &mut Vec<String>,
    resolved: &mut BTreeMap<String, String>,
) -> Result<String, ResolveError> {
    let target = crate::catalog::find(reference).ok_or(ResolveError::Reference)?;
    if target.kind != expected_kind {
        return Err(ResolveError::Reference);
    }
    if declarations.contains_key(reference) {
        resolve_token(reference, declarations, visiting, resolved)
    } else {
        Ok(format!("var({reference})"))
    }
}

fn resolve_gradient_references(
    value: &str,
    declarations: &BTreeMap<String, Declaration>,
    visiting: &mut Vec<String>,
    resolved: &mut BTreeMap<String, String>,
) -> Result<String, ResolveError> {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start) = remaining.find("var(") {
        output.push_str(&remaining[..start]);
        let reference_start = start + "var(".len();
        let end = remaining[reference_start..]
            .find(')')
            .map(|offset| reference_start + offset)
            .ok_or(ResolveError::Value)?;
        let reference = remaining[reference_start..end].trim();
        output.push_str(&resolve_reference(
            reference,
            crate::catalog::ValueKind::Color,
            declarations,
            visiting,
            resolved,
        )?);
        remaining = &remaining[end + 1..];
    }
    output.push_str(remaining);
    Ok(output)
}

fn gradient_references(value: &str) -> Vec<&str> {
    let mut references = Vec::new();
    let mut remaining = value;
    while let Some(start) = remaining.find("var(") {
        let reference_start = start + "var(".len();
        // `normalize_gradient` has already verified each variable stop before this scan.
        let tail = &remaining[reference_start..];
        let end = reference_start + tail.find(')').unwrap_or(tail.len());
        references.push(remaining[reference_start..end].trim());
        remaining = remaining.get(end.saturating_add(1)..).unwrap_or_default();
    }
    references
}

fn whole_var_reference(value: &str) -> Option<&str> {
    let value = value.trim();
    let name = value.strip_prefix("var(")?.strip_suffix(')')?.trim();
    name.starts_with("--").then_some(name)
}

fn normalize_literal(kind: crate::catalog::ValueKind, value: &str) -> Option<String> {
    use crate::catalog::ValueKind;
    match kind {
        ValueKind::Color => normalize_color(value),
        ValueKind::PixelLength => bounded_unit(value, "px", 4.0, 24.0),
        ValueKind::Duration => bounded_unit(value, "ms", 80.0, 800.0).or_else(|| {
            let seconds_text = value.trim().strip_suffix('s')?;
            if seconds_text.ends_with(char::is_whitespace) {
                return None;
            }
            let seconds = seconds_text.parse::<f64>().ok()?;
            if !seconds.is_finite() || !(0.08..=0.8).contains(&seconds) {
                return None;
            }
            Some(format!("{}ms", seconds * 1000.0))
        }),
        ValueKind::Easing => normalize_easing(value),
        ValueKind::GradientList => normalize_gradient(value),
    }
}

pub(crate) fn is_valid_theme_value(kind: crate::catalog::ValueKind, value: &str) -> bool {
    normalize_literal(kind, value).is_some()
}

fn normalize_color(value: &str) -> Option<String> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("transparent") {
        return Some("#00000000".into());
    }
    if let Some(hex) = value.strip_prefix('#') {
        let full = match hex.len() {
            3 => hex.chars().flat_map(|c| [c, c]).collect::<String>() + "ff",
            4 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
            6 => format!("{hex}ff"),
            8 => hex.to_owned(),
            _ => return None,
        };
        if !full.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        return Some(format!("#{}", full.to_ascii_lowercase()));
    }
    let (function, body) = value.split_once('(')?;
    let body = body.strip_suffix(')')?;
    if body.contains(['(', ')']) {
        return None;
    }
    let args = body.split(',').map(str::trim).collect::<Vec<_>>();
    if function.eq_ignore_ascii_case("rgb") || function.eq_ignore_ascii_case("rgba") {
        let rgba = function.eq_ignore_ascii_case("rgba");
        if (rgba && args.len() != 4) || (!rgba && args.len() != 3) {
            return None;
        }
        let channels = args[..3]
            .iter()
            .map(|arg| arg.parse::<u8>().ok())
            .collect::<Option<Vec<_>>>()?;
        let alpha = if rgba { parse_alpha(args[3])? } else { 255 };
        return Some(format!(
            "#{:02x}{:02x}{:02x}{alpha:02x}",
            channels[0], channels[1], channels[2]
        ));
    }
    if function.eq_ignore_ascii_case("hsl") || function.eq_ignore_ascii_case("hsla") {
        let hsla = function.eq_ignore_ascii_case("hsla");
        if (hsla && args.len() != 4) || (!hsla && args.len() != 3) {
            return None;
        }
        let hue = args[0].parse::<f64>().ok()?;
        let saturation = args[1].strip_suffix('%')?.parse::<f64>().ok()? / 100.0;
        let lightness = args[2].strip_suffix('%')?.parse::<f64>().ok()? / 100.0;
        if !hue.is_finite()
            || !(0.0..=360.0).contains(&hue)
            || !(0.0..=1.0).contains(&saturation)
            || !(0.0..=1.0).contains(&lightness)
        {
            return None;
        }
        let alpha = if hsla { parse_alpha(args[3])? } else { 255 };
        let h = if hue == 360.0 { 0.0 } else { hue } / 60.0;
        let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
        let x = chroma * (1.0 - (h % 2.0 - 1.0).abs());
        let (r, g, b) = match h as u8 {
            0 => (chroma, x, 0.0),
            1 => (x, chroma, 0.0),
            2 => (0.0, chroma, x),
            3 => (0.0, x, chroma),
            4 => (x, 0.0, chroma),
            _ => (chroma, 0.0, x),
        };
        let m = lightness - chroma / 2.0;
        let channels = [r, g, b].map(|channel| ((channel + m) * 255.0 + 0.5).floor() as u8);
        return Some(format!(
            "#{:02x}{:02x}{:02x}{alpha:02x}",
            channels[0], channels[1], channels[2]
        ));
    }
    None
}

fn parse_alpha(value: &str) -> Option<u8> {
    let alpha = value.parse::<f64>().ok()?;
    if !alpha.is_finite() || !(0.0..=1.0).contains(&alpha) {
        return None;
    }
    Some((alpha * 255.0 + 0.5).floor() as u8)
}

fn bounded_unit(value: &str, unit: &str, min: f64, max: f64) -> Option<String> {
    let number_text = value.trim().strip_suffix(unit)?;
    if number_text.ends_with(char::is_whitespace) {
        return None;
    }
    let number = number_text.parse::<f64>().ok()?;
    if !number.is_finite() || !(min..=max).contains(&number) {
        return None;
    }
    let formatted = format!("{number}");
    Some(format!("{formatted}{unit}"))
}

fn normalize_easing(value: &str) -> Option<String> {
    let value = value.trim();
    if ["linear", "ease", "ease-in", "ease-out", "ease-in-out"].contains(&value) {
        return Some(value.to_owned());
    }
    let body = value.strip_prefix("cubic-bezier(")?.strip_suffix(')')?;
    let points = body
        .split(',')
        .map(|point| point.trim().parse::<f64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if points.len() != 4
        || points
            .iter()
            .any(|point| !point.is_finite() || !(0.0..=1.0).contains(point))
    {
        return None;
    }
    Some(format!(
        "cubic-bezier({},{},{},{})",
        points[0], points[1], points[2], points[3]
    ))
}

fn normalize_gradient(value: &str) -> Option<String> {
    let value = value.trim();
    if value.contains([';', '{', '}', '\\', '\"', '\'']) {
        return None;
    }
    let layers = split_top_level(value, ',')?;
    if layers.is_empty() || layers.len() > 4 {
        return None;
    }
    for layer in layers {
        let open = layer.find('(')?;
        if !layer.ends_with(')') {
            return None;
        }
        let function = layer[..open].trim();
        if !function.eq_ignore_ascii_case("linear-gradient")
            && !function.eq_ignore_ascii_case("radial-gradient")
        {
            return None;
        }
        let arguments = split_top_level(&layer[open + 1..layer.len() - 1], ',')?;
        let mut stops = arguments.as_slice();
        if function.eq_ignore_ascii_case("linear-gradient")
            && arguments.first()?.trim().ends_with("deg")
        {
            let angle = arguments[0]
                .trim()
                .strip_suffix("deg")?
                .parse::<f64>()
                .ok()?;
            if !angle.is_finite() || !(0.0..=360.0).contains(&angle) {
                return None;
            }
            stops = &arguments[1..];
        } else if function.eq_ignore_ascii_case("radial-gradient")
            && arguments.first()?.trim().starts_with("circle at ")
        {
            let position = arguments[0].trim().strip_prefix("circle at ")?;
            let coordinates = position.split_whitespace().collect::<Vec<_>>();
            if coordinates.len() != 2
                || coordinates
                    .iter()
                    .any(|coordinate| percent(coordinate).is_none())
            {
                return None;
            }
            stops = &arguments[1..];
        }
        if !(2..=8).contains(&stops.len()) {
            return None;
        }
        for stop in stops {
            validate_gradient_stop(stop)?;
        }
    }
    Some(value.to_owned())
}

fn split_top_level(value: &str, separator: char) -> Option<Vec<&str>> {
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut items = Vec::new();
    for (index, character) in value.char_indices() {
        match character {
            '(' => depth = depth.checked_add(1)?,
            ')' => depth = depth.checked_sub(1)?,
            c if c == separator && depth == 0 => {
                items.push(value[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    items.push(value[start..].trim());
    Some(items)
}

fn validate_gradient_stop(stop: &str) -> Option<()> {
    let stop = stop.trim();
    let color_end = if stop.starts_with('#') {
        stop.find(char::is_whitespace).unwrap_or(stop.len())
    } else if let Some(reference) = stop.strip_prefix("var(") {
        let close = reference.find(')')? + "var(".len();
        let name = &stop["var(".len()..close];
        if !name.starts_with("--") || name.contains(['(', ')']) {
            return None;
        }
        close + 1
    } else if stop
        .get(.."transparent".len())
        .is_some_and(|keyword| keyword.eq_ignore_ascii_case("transparent"))
        && stop["transparent".len()..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
    {
        "transparent".len()
    } else {
        let open = stop.find('(')?;
        let function = &stop[..open];
        if !["rgb", "rgba", "hsl", "hsla"]
            .iter()
            .any(|candidate| function.eq_ignore_ascii_case(candidate))
        {
            return None;
        }
        let mut depth = 0usize;
        stop.char_indices()
            .skip(open)
            .find_map(|(index, character)| match character {
                '(' => {
                    depth += 1;
                    None
                }
                ')' => {
                    depth = depth.checked_sub(1)?;
                    (depth == 0).then_some(index + 1)
                }
                _ => None,
            })?
    };
    let color = &stop[..color_end];
    if whole_var_reference(color).is_none() {
        normalize_color(color)?;
    }
    let positions = stop[color_end..].split_whitespace().collect::<Vec<_>>();
    if positions.len() > 2 {
        return None;
    }
    let mut prior = -1.0f64;
    for position in positions {
        let current = percent(position)?;
        if current < prior {
            return None;
        }
        prior = current;
    }
    Some(())
}

fn percent(value: &str) -> Option<f64> {
    let value = value.trim().strip_suffix('%')?.parse::<f64>().ok()?;
    (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value)
}

fn push_warning(warnings: &mut Vec<SkinWarning>, warning: SkinWarning) {
    if warnings.len() < 32 {
        warnings.push(warning);
    }
}

fn safe_token_name(name: &str) -> Option<String> {
    (name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte)))
    .then(|| name.to_owned())
}

#[cfg(test)]
mod tests;
