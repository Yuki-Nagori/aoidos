//! 固定边界分页与 UTF-8 正文分段；不透明引用绑定会话和当前视图身份。

use super::{
    format::{self, Body},
    session::{Session, body_text, store_fault},
};
use crate::{fault::Fault, ports::Outcome};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;

const PAGE_BYTES: usize = 512 * 1024;
const BODY_BYTES: usize = 32 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordItem {
    pub record_seq: u64,
    pub kind: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InFlight {
    pub turn_id: String,
    pub record_seq: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub session_id: String,
    pub items: Vec<RecordItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub last_seq: u64,
    pub last_record_seq: u64,
    pub view_epoch: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    #[serde(flatten)]
    pub page: Page,
    pub needs_recovery: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_flight: Option<InFlight>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyPage {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cursor {
    version: u32,
    session: String,
    epoch: String,
    boundary: u64,
    event_seq: u64,
    before: u64,
    direction: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BodyRef {
    version: u32,
    session: String,
    epoch: String,
    seq: u64,
    hash: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BodyCursor {
    reference: String,
    offset: usize,
}
fn invalid() -> Fault {
    Fault::new("app.bad-request", "记录分页或正文引用不合法")
}
fn missing() -> Fault {
    Fault::new("app.not-found", "记录不存在")
}
fn encode<T: Serialize>(value: &T, key: &str) -> String {
    let bytes = aoidos_json::to_vec(value).expect("reference contains only finite scalar values");
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(&bytes);
    format!("{}.{}", hex(&bytes), hex(&mac.finalize().into_bytes()))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(value: &str) -> Result<Vec<u8>, Fault> {
    if !value.len().is_multiple_of(2) {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().as_chunks::<2>().0 {
        let hex = std::str::from_utf8(pair).map_err(ref_utf8)?;
        bytes.push(u8::from_str_radix(hex, 16).map_err(ref_hex)?);
    }
    Ok(bytes)
}
fn decode<T: serde::de::DeserializeOwned>(value: &str, key: &str) -> Result<T, Fault> {
    if value.len() > 4096 {
        return Err(invalid());
    }
    let (data, signature) = value.split_once('.').ok_or_else(invalid)?;
    let bytes = unhex(data)?;
    let signature = unhex(signature)?;
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(&bytes);
    mac.verify_slice(&signature).map_err(invalid_mac)?;
    aoidos_json::decode_bytes(&bytes, 2048).map_err(ref_json)
}
fn invalid_mac(_: hmac::digest::MacError) -> Fault {
    invalid()
}
fn ref_utf8(_: std::str::Utf8Error) -> Fault {
    invalid()
}
fn ref_hex(_: std::num::ParseIntError) -> Fault {
    invalid()
}
fn ref_json(_: aoidos_json::Error) -> Fault {
    invalid()
}

impl Session {
    /// 历史页的 cursor 保留读边界，后来追加不会混入后续页。
    /// # Errors
    /// limit / cursor 非法为 bad-request；磁盘校验失败为 store.*。
    pub fn page(&self, cursor: Option<&str>, limit: Option<u32>) -> Result<Page, Fault> {
        let limit = limit.unwrap_or(50);
        if limit == 0 || limit > 200 {
            return Err(invalid());
        }
        let highest = self.index.keys().next_back().copied().unwrap_or(0);
        let cursor = match cursor {
            None => Cursor {
                version: 1,
                session: self.header.session_id.clone(),
                epoch: self.view_epoch.clone(),
                boundary: highest,
                event_seq: self.last_event_seq,
                before: highest + 1,
                direction: "descending".into(),
            },
            Some(token) => decode(token, &self.reference_key)?,
        };
        if cursor.version != 1
            || cursor.session != self.header.session_id
            || cursor.epoch != self.view_epoch
            || cursor.direction != "descending"
            || cursor.boundary > highest
            || cursor.before > cursor.boundary + 1
            || cursor.before == 0
            || cursor.event_seq > self.last_event_seq
        {
            return Err(invalid());
        }
        let mut page = Page {
            session_id: self.header.session_id.clone(),
            items: Vec::new(),
            next_cursor: None,
            last_seq: cursor.event_seq,
            last_record_seq: cursor.boundary,
            view_epoch: self.view_epoch.clone(),
        };
        for (count, (&seq, index)) in self
            .index
            .range(..cursor.before)
            .rev()
            .filter(|(seq, index)| self.read_only || self.includes_record(**seq, index))
            .enumerate()
        {
            let token = || {
                encode(
                    &Cursor {
                        before: seq + 1,
                        ..Cursor {
                            version: cursor.version,
                            session: cursor.session.clone(),
                            epoch: cursor.epoch.clone(),
                            boundary: cursor.boundary,
                            event_seq: cursor.event_seq,
                            before: cursor.before,
                            direction: cursor.direction.clone(),
                        }
                    },
                    &self.reference_key,
                )
            };
            if count == limit as usize {
                page.next_cursor = Some(token());
                break;
            }
            let parsed = self.read(seq).map_err(store_fault)?;
            let outcome = parsed.body.as_ref().and_then(|r| match &r.body {
                Body::Narration { terminal, .. } | Body::CharacterSpeech { terminal, .. } => {
                    Some(terminal.outcome())
                }
                _ => None,
            });
            let mut body = format::json(&parsed.raw)
                .map_err(store_fault)?
                .as_object()
                .cloned()
                .ok_or_else(invalid)?;
            for field in ["seq", "kind", "createdAt", "branchSeq"] {
                body.remove(field);
            }
            let reference = encode(
                &BodyRef {
                    version: 1,
                    session: self.header.session_id.clone(),
                    epoch: self.view_epoch.clone(),
                    seq,
                    hash: index.hash.clone(),
                },
                &self.reference_key,
            );
            let large = aoidos_json::to_vec(&body).map_err(ref_json)?.len() > PAGE_BYTES / 2;
            let item = RecordItem {
                record_seq: seq,
                kind: index.kind.clone(),
                created_at: index.created_at.clone(),
                body: (!large && !index.read_only && !self.read_only)
                    .then_some(Value::Object(body)),
                body_ref: (large || index.read_only || self.read_only).then_some(reference),
                turn_id: index.turn_id.clone(),
                outcome,
            };
            page.items.push(item);
            if aoidos_json::to_vec(&page).map_err(ref_json)?.len() > PAGE_BYTES - 4096 {
                page.items.pop();
                if page.items.is_empty() {
                    return Err(Fault::new("store.corrupt", "单条记录摘要超过分页上限"));
                }
                page.next_cursor = Some(token());
                break;
            }
        }
        Ok(page)
    }
    /// 最新窗口只替换当前窗口，不能用其基线假装所有历史页已恢复。
    /// # Errors
    /// 同 [`Self::page`]。
    pub fn view(&self, limit: Option<u32>) -> Result<View, Fault> {
        Ok(View {
            page: self.page(None, limit)?,
            needs_recovery: self.needs_recovery(),
            in_flight: self.in_flight().map(|(turn_id, record_seq)| InFlight {
                turn_id,
                record_seq,
            }),
        })
    }
    /// 当前正文查看只返回一段；旧 epoch 引用立即失效。
    /// # Errors
    /// 身份 / hash / UTF-8 边界非法为 bad-request；有效引用的不存在 seq 为 not-found。
    pub fn body(&self, reference: &str, cursor: Option<&str>) -> Result<BodyPage, Fault> {
        let token: BodyRef = decode(reference, &self.reference_key)?;
        if token.version != 1
            || token.session != self.header.session_id
            || token.epoch != self.view_epoch
            || !format::valid_hash(&token.hash)
            || token.seq == 0
            || token.seq > format::MAX_SEQ
        {
            return Err(invalid());
        }
        let index = self.index.get(&token.seq).ok_or_else(missing)?;
        if token.hash != index.hash {
            return Err(invalid());
        }
        let parsed = self.read(token.seq).map_err(store_fault)?;
        let text = parsed
            .body
            .as_ref()
            .and_then(|r| body_text(&r.body))
            .unwrap_or(&parsed.raw);
        let offset = match cursor {
            None => 0,
            Some(value) => {
                let cursor: BodyCursor = decode(value, &self.reference_key)?;
                if cursor.reference != reference {
                    return Err(invalid());
                }
                cursor.offset
            }
        };
        if offset > text.len() || !text.is_char_boundary(offset) {
            return Err(invalid());
        }
        let mut end = (offset + BODY_BYTES).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Ok(BodyPage {
            text: text[offset..end].into(),
            next_cursor: (end < text.len()).then(|| {
                encode(
                    &BodyCursor {
                        reference: reference.into(),
                        offset: end,
                    },
                    &self.reference_key,
                )
            }),
        })
    }
}

#[cfg(test)]
mod tests;
