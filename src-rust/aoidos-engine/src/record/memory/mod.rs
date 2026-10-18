//! 记忆来源适配：当前有效路径、原文 UTF-8 与角色获知证据均在会话锁内核验。

mod cycle_context;
pub mod processing;
pub mod prompt;
mod proposal_context;

pub use cycle_context::{CycleMarkContext, CycleMarkContextInput, build_cycle_mark_context};
pub use proposal_context::{
    ProposalContextInput, ProposalSourceContextInput, build_proposal_authority_context,
    build_proposal_context, build_proposal_source_context,
};

use std::{cell::RefCell, collections::BTreeMap, sync::Arc};

use super::{
    facts::{Fact, GenerationTarget, RoundIdentity},
    format::{self, Body, InputMode as RecordMode},
    session::Session,
};
use crate::game::catalog::EntityDirectory;
use crate::game::{
    catalog::SceneCatalog,
    presence::{EvidenceKind, SceneEvidence},
};
use crate::ports::{Outcome, Terminal};
use aoidos_memory::{
    error::{Error, Reason, Result},
    model::{
        Boundary, CausalPort, EntityAliasContext, EntityCategory, Hash, InputMode,
        KnowledgeEvidence, MAX_MATERIAL_SPAN_BYTES, MAX_SCAN, MAX_SCAN_BYTES, SourceRef, hash,
    },
};

const DIRECT_DIALOGUE_SCOPE: &str = "directDialogue";
const DIRECT_DIALOGUE_RULE: &str = "directDialogueV1";

/// 引擎持有会话锁时借用；角色证据只认可完整记录的定向角色对话。
pub struct SourcePort<'a> {
    session: &'a Session,
    entity_directory: Option<&'a EntityDirectory>,
    scene_catalog: Option<&'a SceneCatalog>,
    text_cache: RefCell<TextCache>,
}

#[derive(Default)]
struct TextCache {
    records: BTreeMap<u64, Option<CachedText>>,
    bytes: usize,
}

#[derive(Clone)]
struct CachedText {
    text: Arc<str>,
    mode: Option<InputMode>,
    body_hash: Hash,
}

/// 当前历史页中可供记忆处理引用的稳定 UTF-8 素材单元。
pub(crate) struct MaterialSource {
    pub source: SourceRef,
    pub ordinal: u64,
}

pub(crate) struct MaterialSourcePage {
    pub sources: Vec<MaterialSource>,
    pub next_record_seq: Option<u64>,
}

/// 按当前有效路径查找某场景在记录边界前最近一次进入。
/// # Errors
/// 扫描超限、历史待恢复或场景不属于该路径时失败关闭。
pub(crate) fn scene_entry_seq_before(
    session: &Session,
    scene_catalog: &SceneCatalog,
    scene_id: &str,
    before_seq: u64,
) -> Result<u64> {
    let mut scanned = 0usize;
    let mut bytes = 0usize;
    for (&seq, index) in session.index.range(..before_seq).rev().take(MAX_SCAN) {
        bytes = bytes.saturating_add(index.len);
        if bytes > MAX_SCAN_BYTES {
            return Err(Error::Rejected(Reason::RecoveryRequired));
        }
        if !session.includes_record(seq, index) {
            continue;
        }
        scanned += 1;
        let record = session.known(seq)?;
        if let Body::System { code, data, .. } = record.body
            && let Fact::SceneAdvanced { position, .. } = Fact::decode(&code, &data)?
        {
            return if position.scene_id == scene_id {
                Ok(seq)
            } else {
                Err(Error::Rejected(Reason::InvalidSource))
            };
        }
    }
    if scanned == MAX_SCAN {
        return Err(Error::Rejected(Reason::RecoveryRequired));
    }
    if scene_catalog
        .presence_policy()
        .is_some_and(|policy| policy.initial_scene_id() == scene_id)
    {
        Ok(0)
    } else {
        Err(Error::Rejected(Reason::InvalidSource))
    }
}

impl<'a> SourcePort<'a> {
    pub fn new(session: &'a Session) -> Self {
        Self {
            session,
            entity_directory: None,
            scene_catalog: None,
            text_cache: RefCell::default(),
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
            scene_catalog: None,
            text_cache: RefCell::default(),
        })
    }

    /// 绑定校验 observed / rumor mutation 所需的引擎上下文。
    /// # Errors
    /// 实体目录或场景目录不完整、过期或互不匹配时，在查询证据前拒绝。
    pub fn with_evidence_context(
        session: &'a Session,
        entity_directory: &'a EntityDirectory,
        scene_catalog: &'a SceneCatalog,
    ) -> Result<Self> {
        if !entity_directory.is_complete()
            || entity_directory.script_revision() != session.header().script_revision
            || scene_catalog.script_revision() != session.header().script_revision
            || !scene_catalog
                .entity_directory()
                .is_some_and(EntityDirectory::is_complete)
            || scene_catalog.presence_policy().is_none()
        {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        Ok(Self {
            session,
            entity_directory: Some(entity_directory),
            scene_catalog: Some(scene_catalog),
            text_cache: RefCell::default(),
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

    /// 按记录序号扫描当前因果分支，并将长正文切为可持久复用的 UTF-8 片段。
    /// 单页最多检查 256 条 / 4 MiB；游标只前进到已检查的记录，调用方可继续读取后续页。
    /// # Errors
    /// 历史待恢复、页面超过字节预算或来源身份无法核验时失败关闭。
    pub(crate) fn material_source_page(
        &self,
        run_id: &str,
        after_record_seq: Option<u64>,
        through_record_seq: u64,
    ) -> Result<MaterialSourcePage> {
        let boundary = self.boundary()?;
        if run_id.is_empty()
            || boundary.session_id != self.session.header().session_id
            || after_record_seq.is_some_and(|after| after > through_record_seq)
        {
            return Err(Error::Rejected(Reason::InvalidSource));
        }

        let mut sources = Vec::new();
        let mut bytes = 0usize;
        let mut last_scanned = after_record_seq.unwrap_or(0);
        let mut scanned = 0usize;
        let mut stopped_for_bytes = false;
        for (&seq, index) in self
            .session
            .index
            .range((
                std::ops::Bound::Excluded(last_scanned),
                std::ops::Bound::Unbounded,
            ))
            .take_while(|(seq, _)| **seq <= through_record_seq)
            .take(MAX_SCAN)
        {
            let included = self.session.includes_record(seq, index);
            if included {
                let next_bytes = bytes.saturating_add(index.len);
                if next_bytes > MAX_SCAN_BYTES {
                    if bytes == 0 {
                        return Err(Error::Rejected(Reason::CapacityBlocked));
                    }
                    stopped_for_bytes = true;
                    break;
                }
                bytes = next_bytes;
            }
            last_scanned = seq;
            scanned += 1;
            if !included {
                continue;
            }
            let Some((text, mode)) = self.text(seq)? else {
                continue;
            };
            if text.is_empty() {
                continue;
            }
            sources.extend(split_material_source(self, run_id, seq, &text, mode)?);
        }
        let has_more = stopped_for_bytes
            || (scanned == MAX_SCAN
                && self
                    .session
                    .index
                    .range((
                        std::ops::Bound::Excluded(last_scanned),
                        std::ops::Bound::Unbounded,
                    ))
                    .take_while(|(seq, _)| **seq <= through_record_seq)
                    .next()
                    .is_some());
        Ok(MaterialSourcePage {
            sources,
            next_record_seq: has_more.then_some(last_scanned),
        })
    }

    /// 按当前完整实体快照解析稳定 ID 的展示名称。
    /// # Errors
    /// 实体目录未绑定或快照不完整时拒绝读取名称。
    pub fn canonical_entity_name(&self, entity_id: &str) -> Result<Option<String>> {
        let directory = self
            .entity_directory
            .ok_or(Error::Rejected(Reason::StaleVersion))?;
        Ok(directory
            .entity(entity_id)
            .map_err(|_| Error::Rejected(Reason::StaleVersion))?
            .map(|entity| entity.canonical_name.clone()))
    }

    /// 返回目标场景在指定记录边界前最近一次有效进入序号。
    /// # Errors
    /// 历史待恢复、扫描超限或当前场景没有可信进入记录时失败关闭。
    pub(crate) fn scene_entry_seq_before(&self, scene_id: &str, before_seq: u64) -> Result<u64> {
        self.boundary()?;
        let catalog = self
            .scene_catalog
            .ok_or(Error::Rejected(Reason::StaleVersion))?;
        scene_entry_seq_before(self.session, catalog, scene_id, before_seq)
    }

    /// 判断来源序号是否仍在已确认的当前路径上。
    pub(crate) fn has_current_record(&self, record_seq: u64) -> bool {
        self.session
            .index
            .get(&record_seq)
            .is_some_and(|index| self.session.includes_record(record_seq, index))
    }

    /// 当前下一个记录序号，用于清理预写后未能提交来源记录的孤立队列项。
    pub(crate) fn next_sequence(&self) -> u64 {
        self.session.next_sequence()
    }

    fn text(&self, seq: u64) -> Result<Option<(String, Option<InputMode>)>> {
        Ok(self
            .text_arc(seq)?
            .map(|value| (value.text.to_string(), value.mode)))
    }

    fn text_arc(&self, seq: u64) -> Result<Option<CachedText>> {
        self.boundary()?;
        if let Some(cached) = self.text_cache.borrow().records.get(&seq) {
            return Ok(cached.clone());
        }
        let Some(index) = self.session.index.get(&seq) else {
            return Ok(None);
        };
        if !self.session.includes_record(seq, index) {
            return Ok(None);
        }
        let record = self.session.known(seq)?;
        let value = match record.body {
            Body::PlayerSpeech { text, mode, .. } => Some((
                Arc::<str>::from(text),
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
            } => Some((Arc::<str>::from(text), None)),
            Body::Dice { .. } | Body::Check { .. } => Some((
                Arc::<str>::from(aoidos_json::canonical_string(&record.body).map_err(json_error)?),
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
                    Arc::<str>::from(aoidos_json::canonical_string(&data).map_err(json_error)?),
                    None,
                ))
            }
            _ => None,
        };
        let value = value.map(|(text, mode)| CachedText {
            body_hash: hash(text.as_bytes()),
            text,
            mode,
        });
        let mut cache = self.text_cache.borrow_mut();
        let size = value.as_ref().map_or(0, |value| value.text.len());
        if cache.records.len() < MAX_SCAN && cache.bytes.saturating_add(size) <= MAX_SCAN_BYTES {
            cache.bytes += size;
            cache.records.insert(seq, value.clone());
        }
        Ok(value)
    }

    pub(crate) fn direct_dialogue_recipient(
        &self,
        source: &SourceRef,
    ) -> Result<Option<(String, String)>> {
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
                            // 023 随后会校验 checkpoint 锚点及继承的接收目标。
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
                        // 后续 RoundAccepted 可能从有效 checkpoint 继续处理该输入。
                        // 保留已接纳身份以约束继承关系；是否可恢复由 023 校验。
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

    fn valid_scene_evidence(
        &self,
        evidence: &KnowledgeEvidence,
        connection: &rusqlite::Connection,
    ) -> Result<bool> {
        let (Some(directory), Some(catalog)) = (self.entity_directory, self.scene_catalog) else {
            return Ok(false);
        };
        let trusted = SceneEvidence::get(
            connection,
            self.session,
            catalog,
            directory,
            &evidence.evidence_id,
        )
        .map_err(|_| Error::Rejected(Reason::PolicyUnavailable))?;
        let Some(trusted) = trusted else {
            return Ok(false);
        };
        let Some((source_text, _)) = self.text(evidence.source.record_seq)? else {
            return Ok(false);
        };
        let expected_kind = match trusted.kind {
            EvidenceKind::Observed => aoidos_memory::model::KnowledgeKind::Observed,
            EvidenceKind::Rumor => aoidos_memory::model::KnowledgeKind::Rumor,
        };
        Ok(evidence.kind == expected_kind
            && evidence.subject_id == trusted.subject_id
            && evidence.informant_id == trusted.informant_id
            && evidence.scope == trusted.scope_id
            && evidence.rule_id == trusted.rule_id
            && evidence.source.record_seq == trusted.source.record_seq
            && evidence.source.body_hash == hash(source_text.as_bytes())
            && format::hash(source_text.as_bytes()) == trusted.source.body_hash
            && evidence.source.start_byte == trusted.source.start_byte
            && evidence.source.end_byte == trusted.source.end_byte
            && self.source_text(&evidence.source)?.is_some())
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
    fn valid_evidence_with_connection(
        &self,
        evidence: &KnowledgeEvidence,
        connection: &rusqlite::Connection,
    ) -> Result<bool> {
        if evidence.kind == aoidos_memory::model::KnowledgeKind::Told
            && evidence.scope == DIRECT_DIALOGUE_SCOPE
            && evidence.rule_id == DIRECT_DIALOGUE_RULE
        {
            return self.valid_evidence(evidence);
        }
        self.valid_scene_evidence(evidence, connection)
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
        let Some(value) = self.text_arc(source.record_seq)? else {
            return Ok(None);
        };
        if source.body_hash != value.body_hash || source.mode != value.mode {
            return Ok(None);
        }
        if source.end_byte > value.text.len() as u64 {
            return Ok(None);
        }
        // start < end <= 字符串长度，两个偏移在当前平台均可表示。
        let (start, end) = (source.start_byte as usize, source.end_byte as usize);
        Ok(value.text.get(start..end).map(str::to_owned))
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

    fn valid_scene_entry(&self, scene_id: &str, record_seq: u64) -> Result<bool> {
        if record_seq == 0 {
            return Ok(self
                .scene_catalog
                .and_then(SceneCatalog::presence_policy)
                .is_some_and(|policy| policy.initial_scene_id() == scene_id));
        }
        let Some(index) = self.session.index.get(&record_seq) else {
            return Ok(false);
        };
        if !self.session.includes_record(record_seq, index) {
            return Ok(false);
        }
        let record = self.session.known(record_seq)?;
        let Body::System { code, data, .. } = record.body else {
            return Ok(false);
        };
        Ok(matches!(
            Fact::decode(&code, &data)?,
            Fact::SceneAdvanced { position, .. } if position.scene_id == scene_id
        ))
    }

    fn valid_completed_round(
        &self,
        round_id: &str,
        scene_id: &str,
        entry_record_seq: u64,
        record_seq: u64,
    ) -> Result<bool> {
        if record_seq <= entry_record_seq || !self.valid_scene_entry(scene_id, entry_record_seq)? {
            return Ok(false);
        }
        let Some(index) = self.session.index.get(&record_seq) else {
            return Ok(false);
        };
        if !self.session.includes_record(record_seq, index) {
            return Ok(false);
        }
        let record = self.session.known(record_seq)?;
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

        // 完成事实必须能对应到当前有效路径中同场景的场内接纳事实。
        let mut bytes = 0_usize;
        for (&seq, index) in self.session.index.range(..record_seq).rev().take(MAX_SCAN) {
            bytes = bytes.saturating_add(index.len);
            if bytes > MAX_SCAN_BYTES {
                return Ok(false);
            }
            if !self.session.includes_record(seq, index) {
                continue;
            }
            let accepted = self.session.known(seq)?;
            if let Body::System { code, data, .. } = accepted.body
                && let Fact::RoundAccepted {
                    identity: accepted_identity,
                    mode,
                    scene_id: accepted_scene,
                    ..
                } = Fact::decode(&code, &data)?
                && accepted_identity.round_id == round_id
            {
                if seq <= entry_record_seq
                    || mode != RecordMode::InCharacter
                    || accepted_scene != scene_id
                {
                    return Ok(false);
                }
                // 边界内再次发生场景切换，说明该回合不属于传入的进入实例。
                for (&between_seq, between_index) in self.session.index.range((
                    std::ops::Bound::Excluded(entry_record_seq),
                    std::ops::Bound::Excluded(seq),
                )) {
                    if !self.session.includes_record(between_seq, between_index) {
                        continue;
                    }
                    let between = self.session.known(between_seq)?;
                    if let Body::System { code, data, .. } = between.body
                        && matches!(Fact::decode(&code, &data)?, Fact::SceneAdvanced { .. })
                    {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn split_material_source(
    port: &SourcePort<'_>,
    run_id: &str,
    record_seq: u64,
    text: &str,
    mode: Option<InputMode>,
) -> Result<Vec<MaterialSource>> {
    let body_hash = hash(text.as_bytes());
    let mut result = Vec::new();
    let mut start = 0usize;
    while start < text.len() {
        let mut end = (start + MAX_MATERIAL_SPAN_BYTES).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        let source = SourceRef {
            script_id: port.session.header().script_id.clone(),
            run_id: run_id.into(),
            session_id: port.session.header().session_id.clone(),
            record_seq,
            body_hash,
            start_byte: start as u64,
            end_byte: end as u64,
            mode: mode.clone(),
        };
        let part = text[start..end].to_owned();
        if port.source_text(&source)?.as_deref() != Some(&part) {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        let ordinal = result.len() as u64;
        result.push(MaterialSource { source, ordinal });
        start = end;
    }
    Ok(result)
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
