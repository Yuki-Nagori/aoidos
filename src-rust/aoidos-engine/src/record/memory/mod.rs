//! 022 的记忆来源适配：当前有效路径、原文 UTF-8 与完成事实均在会话锁内核验。

use super::{
    facts::Fact,
    format::{Body, InputMode as RecordMode},
    session::Session,
};
use crate::ports::{Outcome, Terminal};
use aoidos_memory::{
    error::{Error, Reason, Result},
    model::{Boundary, CausalPort, InputMode, KnowledgeEvidence, SourceRef, hash},
};

/// 引擎持有会话锁时借用；030 尚未登记角色获知端口，因此不授予 NPC 证据。
pub struct SourcePort<'a> {
    session: &'a Session,
}
impl<'a> SourcePort<'a> {
    pub fn new(session: &'a Session) -> Self {
        Self { session }
    }
    /// # Errors
    /// 只从已提交且当前有效的来源构造引用，不能从模型偏移推定资格。
    pub fn reference(&self, run_id: &str, seq: u64) -> Result<SourceRef> {
        let (text, mode) = self
            .text(seq)?
            .ok_or(Error::Rejected(Reason::InvalidSource))?;
        if text.is_empty() {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        Ok(SourceRef {
            script_id: self.session.header().script_id.clone(),
            run_id: run_id.into(),
            session_id: self.session.header().session_id.clone(),
            record_seq: seq,
            body_hash: hash(text.as_bytes()),
            start_byte: 0,
            end_byte: text.len() as u64,
            mode,
        })
    }
    fn text(&self, seq: u64) -> Result<Option<(String, Option<InputMode>)>> {
        self.boundary()?;
        let Some(index) = self.session.index.get(&seq) else {
            return Ok(None);
        };
        if !self.session.includes_record(seq, index) {
            return Ok(None);
        }
        let record = self.session.known(seq)?;
        Ok(match record.body {
            Body::PlayerSpeech { text, mode, .. } => Some((
                text,
                Some(match mode.unwrap_or(RecordMode::InCharacter) {
                    RecordMode::InCharacter => InputMode::InCharacter,
                    RecordMode::OutOfCharacter => InputMode::OutOfCharacter,
                }),
            )),
            Body::Narration {
                text,
                terminal: Terminal::Completed { .. },
                ..
            }
            | Body::CharacterSpeech {
                text,
                terminal: Terminal::Completed { .. },
                ..
            } => Some((text, None)),
            Body::Dice { .. } | Body::Check { .. } => Some((
                aoidos_json::canonical_string(&record.body).map_err(json_error)?,
                None,
            )),
            Body::System { code, data, .. }
                if matches!(
                    Fact::decode(&code, &data)?,
                    Fact::RoundSettled { .. }
                        | Fact::SceneAdvanced { .. }
                        | Fact::SceneStayed { .. }
                        | Fact::SessionEnded { .. }
                        | Fact::RoundEnded {
                            outcome: Outcome::Completed,
                            ..
                        }
                ) =>
            {
                Some((
                    aoidos_json::canonical_string(&data).map_err(json_error)?,
                    None,
                ))
            }
            _ => None,
        })
    }
}
impl CausalPort for SourcePort<'_> {
    fn boundary(&self) -> Result<Boundary> {
        if self.session.needs_recovery() {
            return Err(Error::Rejected(Reason::RecoveryRequired));
        }
        Ok(Boundary {
            script_id: self.session.header().script_id.clone(),
            session_id: self.session.header().session_id.clone(),
            history_revision: self.session.history_revision(),
            entity_revision: 0,
            evidence_revision: 0,
        })
    }
    fn valid_source(&self, source: &SourceRef) -> Result<bool> {
        Ok(self.source_text(source)?.is_some())
    }
    fn valid_evidence(&self, _: &KnowledgeEvidence) -> Result<bool> {
        Ok(false)
    }
    fn source_text(&self, source: &SourceRef) -> Result<Option<String>> {
        if source.script_id != self.session.header().script_id
            || source.session_id != self.session.header().session_id
            || source.start_byte >= source.end_byte
        {
            return Ok(None);
        }
        let Some((text, mode)) = self.text(source.record_seq)? else {
            return Ok(None);
        };
        if source.body_hash != hash(text.as_bytes()) || source.mode != mode {
            return Ok(None);
        }
        if source.end_byte > text.len() as u64 {
            return Ok(None);
        }
        // start < end <= 字符串长度，两个偏移在当前平台均可表示。
        let (start, end) = (source.start_byte as usize, source.end_byte as usize);
        Ok(text.get(start..end).map(str::to_owned))
    }
    fn completed_round(&self, round_id: &str, source: &SourceRef) -> Result<bool> {
        if !self.valid_source(source)? {
            return Ok(false);
        }
        let record = self.session.known(source.record_seq)?;
        let Body::System { code, data, .. } = record.body else {
            return Ok(false);
        };
        let Fact::RoundEnded {
            identity,
            outcome: Outcome::Completed,
            ..
        } = Fact::decode(&code, &data)?
        else {
            return Ok(false);
        };
        if identity.round_id != round_id {
            return Ok(false);
        }
        let mut bytes = 0;
        // 只核验当前回合的接纳事实；超过预算不猜测场内身份。
        for (&seq, index) in self
            .session
            .index
            .range(..source.record_seq)
            .rev()
            .take(256)
        {
            bytes += index.len;
            if bytes > 4 * 1024 * 1024 {
                return Ok(false);
            }
            if !self.session.includes_record(seq, index) {
                continue;
            }
            let record = self.session.known(seq)?;
            if let Body::System { code, data, .. } = record.body
                && let Fact::RoundAccepted { identity, mode, .. } = Fact::decode(&code, &data)?
                && identity.round_id == round_id
            {
                return Ok(mode == RecordMode::InCharacter);
            }
        }
        Ok(false)
    }
}
fn json_error(_: aoidos_json::Error) -> Error {
    Error::Corrupt
}
#[cfg(test)]
mod tests;
