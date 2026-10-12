//! 记忆来源适配：当前有效路径、原文 UTF-8 与角色获知证据均在会话锁内核验。

use super::{
    facts::{Fact, GenerationTarget, RoundIdentity},
    format::{Body, InputMode as RecordMode},
    session::Session,
};
use crate::game::catalog::EntityDirectory;
use crate::ports::{Outcome, Terminal};
use aoidos_memory::{
    error::{Error, Reason, Result},
    model::{
        Boundary, CausalPort, EntityAliasContext, EntityCategory, InputMode, KnowledgeEvidence,
        MAX_SCAN, MAX_SCAN_BYTES, SourceRef, hash,
    },
};

const DIRECT_DIALOGUE_SCOPE: &str = "directDialogue";
const DIRECT_DIALOGUE_RULE: &str = "directDialogueV1";

/// 引擎持有会话锁时借用；角色证据只认可完整记录的定向角色对话。
pub struct SourcePort<'a> {
    session: &'a Session,
    entity_directory: Option<&'a EntityDirectory>,
}
impl<'a> SourcePort<'a> {
    pub fn new(session: &'a Session) -> Self {
        Self {
            session,
            entity_directory: None,
        }
    }
    /// 将完整剧本目录绑定到当前记录，供记忆快照冻结实体 revision。
    /// # Errors
    /// 不完整目录或与存档剧本修订不匹配时拒绝建立记忆来源端口。
    pub fn with_entity_directory(
        session: &'a Session,
        entity_directory: &'a EntityDirectory,
    ) -> Result<Self> {
        if !entity_directory.is_complete()
            || entity_directory.script_revision() != session.header().script_revision
        {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        Ok(Self {
            session,
            entity_directory: Some(entity_directory),
        })
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

    fn direct_dialogue_recipient(&self, source: &SourceRef) -> Result<Option<(String, String)>> {
        if source.mode != Some(InputMode::InCharacter) || self.source_text(source)?.is_none() {
            return Ok(None);
        }
        let input = self.session.known(source.record_seq)?;
        let Body::PlayerSpeech {
            player_id,
            mode: Some(RecordMode::InCharacter),
            ..
        } = input.body
        else {
            return Ok(None);
        };

        let mut accepted: Option<(RoundIdentity, String)> = None;
        let mut scanned_bytes = 0usize;
        let mut scanned_path = vec![source.record_seq];
        for (&seq, index) in self
            .session
            .index
            .range((
                std::ops::Bound::Excluded(source.record_seq),
                std::ops::Bound::Unbounded,
            ))
            .take(MAX_SCAN)
        {
            scanned_bytes = scanned_bytes.saturating_add(index.len);
            if scanned_bytes > MAX_SCAN_BYTES {
                return Ok(None);
            }
            if !self.session.includes_record(seq, index) {
                continue;
            }
            scanned_path.push(seq);
            let record = self.session.known(seq)?;
            let Body::System { code, data, .. } = record.body else {
                continue;
            };
            match Fact::decode(&code, &data)? {
                Fact::RoundAccepted {
                    identity,
                    input_seq,
                    mode,
                    target,
                    source_round_id,
                    checkpoint_seq,
                    ..
                } if input_seq == source.record_seq => {
                    let GenerationTarget::CharacterSpeech { speaker_id } = target else {
                        return Ok(None);
                    };
                    if mode != RecordMode::InCharacter {
                        return Ok(None);
                    }
                    match (accepted.as_ref(), source_round_id, checkpoint_seq) {
                        (None, None, None) => accepted = Some((identity, speaker_id)),
                        (Some((previous, previous_speaker)), Some(source), Some(_))
                            if previous.round_id == source
                                && previous_speaker == &speaker_id
                                && !same_round(previous, &identity) =>
                        {
                            // 023 validates the checkpoint anchor and inherited target below.
                            accepted = Some((identity, speaker_id));
                        }
                        _ => return Ok(None),
                    }
                }
                Fact::RoundEnded {
                    identity,
                    outcome,
                    through_seq,
                    ..
                } if accepted
                    .as_ref()
                    .is_some_and(|(accepted, _)| same_round(accepted, &identity)) =>
                {
                    if matches!(outcome, Outcome::Failed | Outcome::Cancelled) {
                        // A later RoundAccepted may continue this input from a valid checkpoint.
                        // Keep the accepted identity so that continuation must link to this round;
                        // 023's recovery validator decides whether that checkpoint is resumable.
                        continue;
                    }
                    let recovered = crate::game::recovery::derive_evidence_path(
                        self.session,
                        scanned_path,
                        seq,
                    )
                    .map_err(memory_recovery_error)?;
                    if let Some(round) = recovered.round
                        && same_round(&round.identity, &identity)
                        && round.input_seq == source.record_seq
                        && round.mode == RecordMode::InCharacter
                        && round.advance == Some(through_seq)
                        && let Some(response_seq) = round.narrative
                    {
                        let response = self.session.known(response_seq)?;
                        // 共享恢复器已核验完整因果链和有效正文引用；这里只补角色身份门控。
                        if let (
                            Some((_, speaker_id)),
                            Body::CharacterSpeech {
                                speaker_id: response_speaker,
                                terminal: Terminal::Completed { .. },
                                ..
                            },
                        ) = (accepted, response.body)
                            && response_speaker == speaker_id
                        {
                            return Ok(Some((speaker_id, player_id)));
                        }
                    }
                    // 终态闭合本次证据，后续回合不扩大扫描预算，也不能补足本轮证据。
                    return Ok(None);
                }
                _ => {}
            }
        }

        Ok(None)
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
            entity_revision: self.entity_directory.map_or(0, EntityDirectory::revision),
            // 获知证据由有效记录路径派生；历史修订是保守的 CAS 版本。
            evidence_revision: self.session.history_revision(),
        })
    }
    fn valid_source(&self, source: &SourceRef) -> Result<bool> {
        Ok(self.source_text(source)?.is_some())
    }
    fn valid_evidence(&self, evidence: &KnowledgeEvidence) -> Result<bool> {
        if evidence.kind != aoidos_memory::model::KnowledgeKind::Told
            || evidence.scope != DIRECT_DIALOGUE_SCOPE
            || evidence.rule_id != DIRECT_DIALOGUE_RULE
        {
            return Ok(false);
        }
        let Some(directory) = self.entity_directory else {
            return Ok(false);
        };
        let Some((speaker_id, player_id)) = self.direct_dialogue_recipient(&evidence.source)?
        else {
            return Ok(false);
        };
        Ok(speaker_id == evidence.subject_id
            && evidence.informant_id.as_deref() == Some(player_id.as_str())
            && directory.contains_character(&speaker_id))
    }
    fn entity_alias_context(&self, entity_id: &str) -> Result<Option<EntityAliasContext>> {
        let Some(directory) = self.entity_directory else {
            return Ok(None);
        };
        let Some(entity) = directory
            .entity(entity_id)
            .map_err(|_| Error::Rejected(Reason::PolicyUnavailable))?
        else {
            return Ok(None);
        };
        Ok(Some(EntityAliasContext {
            entity_id: entity.id.clone(),
            category: match entity.kind {
                crate::game::catalog::EntityKind::Character => EntityCategory::Character,
                crate::game::catalog::EntityKind::Place => EntityCategory::Place,
                crate::game::catalog::EntityKind::Organization => EntityCategory::Organization,
                crate::game::catalog::EntityKind::Item => EntityCategory::Item,
                crate::game::catalog::EntityKind::Term => EntityCategory::Term,
            },
            scope: entity.scope.clone(),
            canonical_name: entity.canonical_name.clone(),
        }))
    }
    fn canonical_entity_id(
        &self,
        category: EntityCategory,
        scope: &str,
        name: &str,
    ) -> Result<Option<String>> {
        let Some(directory) = self.entity_directory else {
            return Ok(None);
        };
        let kind = match category {
            EntityCategory::Character => crate::game::catalog::EntityKind::Character,
            EntityCategory::Place => crate::game::catalog::EntityKind::Place,
            EntityCategory::Organization => crate::game::catalog::EntityKind::Organization,
            EntityCategory::Item => crate::game::catalog::EntityKind::Item,
            EntityCategory::Term => crate::game::catalog::EntityKind::Term,
        };
        directory
            .resolve_exact(kind, scope, name)
            .map(|entity| entity.map(|entity| entity.id.clone()))
            .map_err(|_| Error::Rejected(Reason::PolicyUnavailable))
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
fn same_round(left: &RoundIdentity, right: &RoundIdentity) -> bool {
    left.version == right.version
        && left.round_id == right.round_id
        && left.operation_id == right.operation_id
}
fn memory_recovery_error(_: crate::fault::Fault) -> Error {
    Error::Corrupt
}
fn json_error(_: aoidos_json::Error) -> Error {
    Error::Corrupt
}
#[cfg(test)]
mod tests;
