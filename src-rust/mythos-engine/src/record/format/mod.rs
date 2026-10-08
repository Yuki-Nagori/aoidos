//! v1 类型与严格读取；保留原行负责向前兼容，不经枚举重新导出。

use crate::ports::Terminal;
use mythos_store::error::{Result, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const MAX_SEQ: u64 = (1 << 53) - 1;
pub const MAX_LINE: usize = 2 * 1024 * 1024;
pub const MAX_TEXT: usize = 256 * 1024;

/// 冻结前缀包含完整 STATIC 标记和末尾 LF，hash 对原字节计算。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    pub kind: String,
    pub format_version: u32,
    pub grammar_version: u32,
    pub projection_version: u32,
    pub script_id: String,
    pub session_id: String,
    pub created_at: String,
    pub static_prefix: String,
    pub static_prefix_hash: String,
    pub script_revision: String,
}

/// 玩家输入字节区间；索引单位永远是 UTF-8 字节，不是 JS 字符偏移。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InputMode {
    InCharacter,
    OutOfCharacter,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub kind: String,
    pub id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Roll {
    pub sides: u32,
    pub value: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Modifier {
    pub value: i32,
    pub source: Source,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RngTrace {
    pub algorithm: String,
    pub mapping_version: u32,
    pub seed: String,
    pub start_counter: String,
    pub end_counter: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckResult {
    Success,
    CostlySuccess,
    Failure,
    CriticalSuccess,
    CriticalFailure,
}

/// 磁盘事实。预留 / 未知种类另保留原行，不用此枚举执行控制。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Body {
    PlayerSpeech {
        player_id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<InputMode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_range: Option<ContentRange>,
    },
    Narration {
        text: String,
        turn_id: String,
        #[serde(flatten)]
        terminal: Terminal,
    },
    CharacterSpeech {
        speaker_id: String,
        text: String,
        turn_id: String,
        #[serde(flatten)]
        terminal: Terminal,
    },
    Dice {
        expression: String,
        rolls: Vec<Roll>,
        total: i32,
        source: Source,
        plan_id: String,
        rng: RngTrace,
        modifiers: Vec<Modifier>,
    },
    Check {
        dice_seq: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        dc: Option<f64>,
        result: CheckResult,
        rule_id: String,
        plan_id: String,
    },
    System {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        related_seq: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        data: Value,
    },
    Recap {
        from_seq: u64,
        through_seq: u64,
        text: String,
        source_hash: String,
        estimator_version: u32,
        origin: RecapOrigin,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecapOrigin {
    Manual,
    Background,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub seq: u64,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_seq: Option<u64>,
    #[serde(flatten)]
    pub body: Body,
}

/// 已解析事实仍带原字节；未知可选字段与空白不丢失。
#[derive(Debug, Clone)]
pub struct Parsed {
    pub seq: u64,
    pub created_at: String,
    pub kind: String,
    pub body: Option<Record>,
    pub raw: String,
    pub read_only: bool,
}

pub fn corrupt() -> StoreError {
    StoreError::Corrupt("invalid record format or identity".into())
}
/// 返回带算法前缀的 SHA-256，不按重新序列化后的对象取 hash。
pub fn hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
pub fn valid_hash(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}
pub fn valid_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}
pub fn now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("UTC timestamp is representable")
}
fn valid_time(value: &str) -> bool {
    value.len() <= 30 && value.ends_with('Z') && OffsetDateTime::parse(value, &Rfc3339).is_ok()
}
fn positive(seq: u64) -> bool {
    seq > 0 && seq <= MAX_SEQ
}

impl Header {
    /// # Errors
    /// 版本、身份、冻结前缀和摘要不一致返回 corrupt。
    pub fn validate(&self) -> Result<()> {
        if self.kind != "header"
            || self.format_version != 1
            || self.grammar_version != 1
            || self.projection_version != 1
            || mythos_store::paths::script_id_from_name(&self.script_id) != self.script_id
            || !valid_uuid(&self.session_id)
            || !valid_time(&self.created_at)
            || !valid_hash(&self.script_revision)
            || self.static_prefix_hash != hash(self.static_prefix.as_bytes())
            || !self.static_prefix.starts_with("[MYTHOS:STATIC]\n")
            || !self.static_prefix.ends_with("\n[/MYTHOS:STATIC]\n")
        {
            return Err(corrupt());
        }
        Ok(())
    }
}
/// 读取时核验规范骰式，不执行 RNG；恢复沿用既定 rolls / total。
pub fn dice_expression(value: &str) -> Option<(u32, u32, i32)> {
    let (count, rest) = value.split_once('d')?;
    let signed = rest.find(['+', '-']);
    let (sides, modifier) = match signed {
        Some(at) => (&rest[..at], rest[at..].parse::<i32>().ok()?),
        None => (rest, 0),
    };
    let count = count.parse::<u32>().ok()?;
    let sides = sides.parse::<u32>().ok()?;
    if !(1..=20).contains(&count) || !(2..=1000).contains(&sides) || i64::from(modifier).abs() > 100
    {
        return None;
    }
    let canonical = if modifier == 0 {
        format!("{count}d{sides}")
    } else {
        format!("{count}d{sides}{modifier:+}")
    };
    (canonical == value).then_some((count, sides, modifier))
}
impl Record {
    /// 校验单块字段；引用和分支由会话有效路径再校验。
    /// # Errors
    /// 非法已知字段返回 corrupt，不能当作未知兼容内容继续写入。
    pub fn validate(&self) -> Result<()> {
        if !positive(self.seq)
            || !valid_time(&self.created_at)
            || self
                .branch_seq
                .is_some_and(|seq| !positive(seq) || seq >= self.seq)
        {
            return Err(corrupt());
        }
        match &self.body {
            Body::PlayerSpeech {
                player_id,
                text,
                content_range,
                ..
            } => {
                if !valid_id(player_id) || text.is_empty() || text.len() > MAX_TEXT {
                    return Err(corrupt());
                }
                if let Some(range) = content_range
                    && (range.start > range.end
                        || range.end > text.len()
                        || !text.is_char_boundary(range.start)
                        || !text.is_char_boundary(range.end))
                {
                    return Err(corrupt());
                }
            }
            Body::Narration {
                text,
                turn_id,
                terminal,
            }
            | Body::CharacterSpeech {
                text,
                turn_id,
                terminal,
                ..
            } => {
                if !valid_uuid(turn_id) || text.len() > MAX_TEXT || text.contains('\r') {
                    return Err(corrupt());
                }
                if let Body::CharacterSpeech { speaker_id, .. } = &self.body
                    && !valid_id(speaker_id)
                {
                    return Err(corrupt());
                }
                if let Terminal::Failed {
                    error,
                    finish_reason,
                } = terminal
                    && (error.code == "llm.empty-output" && finish_reason.is_none()
                        || error.code.is_empty()
                        || error.message.is_empty())
                {
                    return Err(corrupt());
                }
            }
            Body::Dice {
                rolls,
                total,
                plan_id,
                rng,
                modifiers,
                source,
                expression,
            } => {
                let Some((count, sides, modifier)) = dice_expression(expression) else {
                    return Err(corrupt());
                };
                if count as usize != rolls.len()
                    || rolls.iter().any(|roll| roll.sides != sides)
                    || modifiers.iter().map(|m| i64::from(m.value)).sum::<i64>()
                        != i64::from(modifier)
                {
                    return Err(corrupt());
                }
                if !valid_id(plan_id)
                    || !valid_id(&source.id)
                    || source.kind.is_empty()
                    || rolls.is_empty()
                    || rolls.len() > 20
                    || modifiers.len() > 32
                    || expression.len() > 64
                    || rolls
                        .iter()
                        .any(|r| r.sides < 2 || r.sides > 1000 || r.value == 0 || r.value > r.sides)
                    || modifiers.iter().any(|m| {
                        !valid_id(&m.source.id)
                            || m.source.kind.is_empty()
                            || i64::from(m.value).abs() > 100
                    })
                    || rolls.iter().map(|r| i64::from(r.value)).sum::<i64>()
                        + modifiers.iter().map(|m| i64::from(m.value)).sum::<i64>()
                        != i64::from(*total)
                    || rng.algorithm != "chacha20-v1"
                    || rng.mapping_version != 1
                    || rng.seed.len() != 64
                    || !rng
                        .seed
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                    || rng
                        .start_counter
                        .parse::<u64>()
                        .ok()
                        .zip(rng.end_counter.parse::<u64>().ok())
                        .is_none_or(|(start, end)| end < start || end - start < rolls.len() as u64)
                {
                    return Err(corrupt());
                }
            }
            Body::Check {
                dice_seq,
                dc,
                rule_id,
                plan_id,
                ..
            } => {
                if !positive(*dice_seq)
                    || *dice_seq >= self.seq
                    || !valid_id(rule_id)
                    || !valid_id(plan_id)
                    || dc.is_some_and(|value| !value.is_finite())
                {
                    return Err(corrupt());
                }
            }
            Body::System {
                message,
                related_seq,
                turn_id,
                data,
                ..
            } => {
                if message.len() > MAX_TEXT
                    || related_seq.is_some_and(|seq| !positive(seq) || seq >= self.seq)
                    || turn_id.as_ref().is_some_and(|id| !valid_uuid(id))
                    || !data.is_object()
                {
                    return Err(corrupt());
                }
            }
            Body::Recap {
                from_seq,
                through_seq,
                text,
                source_hash,
                estimator_version,
                ..
            } => {
                if !positive(*from_seq)
                    || *through_seq < *from_seq
                    || *through_seq >= self.seq
                    || text.trim().is_empty()
                    || text.len() > MAX_TEXT
                    || !valid_hash(source_hash)
                    || !matches!(*estimator_version, 1 | 2)
                {
                    return Err(corrupt());
                }
            }
        }
        Ok(())
    }
}

/// 严格 JSON 解码，包括嵌套对象重复键；不让 serde 默认的末键覆盖隐藏损坏。
/// # Errors
/// 语法、重复键或行上限错误返回 corrupt。
pub fn json(line: &str) -> Result<Value> {
    if line.len() > MAX_LINE || !line.ends_with('\n') {
        return Err(corrupt());
    }
    mythos_json::parse(line, MAX_LINE).map_err(decode_error)
}
fn decode_error(_: mythos_json::Error) -> StoreError {
    corrupt()
}
/// # Errors
/// 已知块的非法字段不得降级为未知；未知 kind 只读保存原行。
pub fn parse(line: &str) -> Result<Parsed> {
    let value = json(line)?;
    let seq = value["seq"]
        .as_u64()
        .filter(|seq| positive(*seq))
        .ok_or_else(corrupt)?;
    let created_at = value["createdAt"]
        .as_str()
        .filter(|time| valid_time(time))
        .ok_or_else(corrupt)?
        .to_owned();
    let kind = value["kind"]
        .as_str()
        .filter(|kind| !kind.is_empty() && kind.len() <= 64)
        .ok_or_else(corrupt)?
        .to_owned();
    let known = matches!(
        kind.as_str(),
        "playerSpeech" | "narration" | "characterSpeech" | "dice" | "check" | "system" | "recap"
    );
    for field in [
        "mode",
        "contentRange",
        "branchSeq",
        "dc",
        "relatedSeq",
        "finishReason",
        "error",
        "speakerId",
    ] {
        if known && value.get(field).is_some_and(Value::is_null) {
            return Err(corrupt());
        }
    }
    if matches!(kind.as_str(), "narration" | "characterSpeech") {
        let outcome = value["outcome"].as_str().ok_or_else(corrupt)?;
        if (outcome == "completed"
            && (value.get("error").is_some() || value.get("finishReason").is_none()))
            || (outcome == "cancelled"
                && (value.get("error").is_some() || value.get("finishReason").is_some()))
            || (outcome == "failed" && value.get("error").is_none())
        {
            return Err(corrupt());
        }
    }
    if kind == "system" {
        let code = value["code"].as_str().ok_or_else(corrupt)?;
        if registered_system(code) && value["data"]["version"] == 1 {
            super::facts::Fact::decode(code, &value["data"])?;
        }
    }
    let body = if known {
        let record: Record = mythos_json::from_value(value).map_err(decode_error)?;
        record.validate()?;
        Some(record)
    } else {
        None
    };
    let read_only=body.as_ref().is_none_or(|record| matches!(&record.body,Body::System{code,data,..} if !registered_system(code) || data["version"]!=1));
    Ok(Parsed {
        seq,
        created_at,
        kind,
        body,
        raw: line.to_owned(),
        read_only,
    })
}
pub fn registered_system(code: &str) -> bool {
    matches!(
        code,
        "orphan"
            | "roundAccepted"
            | "checkPlanned"
            | "checkSkipped"
            | "settlementPlanned"
            | "roundSettled"
            | "sceneAdvanced"
            | "sceneStayed"
            | "sessionEnded"
            | "roundEnded"
            | "abandonCheckpoint"
            | "historyFork"
    )
}
/// 序列化单条事实并校验 LF / 行上限；可选字段遵循省略语义。
/// # Errors
/// 序列化或容量不合法返回 corrupt。
pub fn line<T: Serialize>(value: &T) -> Result<String> {
    let mut line = mythos_json::to_string(value).map_err(decode_error)?;
    line.push('\n');
    if line.len() > MAX_LINE {
        return Err(corrupt());
    }
    Ok(line)
}

#[cfg(test)]
mod tests;
