//! 会话单写者；索引只保存位置和身份，正文按行读取。

use super::format::{self, Body, Header, MAX_LINE, MAX_SEQ, MAX_TEXT, Parsed, Record};
use crate::{
    fault::Fault,
    ports::{OutputWriter, Terminal},
};
use futures::future::BoxFuture;
use mythos_store::{error::Result, journal::Journal};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 记录事件只带身份，正文始终由有界读取返回。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Appended {
    pub session_id: String,
    pub view_epoch: String,
    pub record_seq: u64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}

/// 平台预留与投递分离，不把投递失败当作文件回滚。
pub trait RecordEvents: Send + Sync {
    /// # Errors
    /// 事件无法准备时不得开始追加正式行。
    fn prepare(&self, event: &Appended) -> std::result::Result<u64, Fault>;
    /// # Errors
    /// 已提交后投递失败保留事实与基线。
    fn deliver(&self, seq: u64, event: Appended) -> std::result::Result<(), Fault>;
    /// 流写者退出后清退平台序号；无序号状态的纯夹具可忽略。
    fn retire(&self, _session_id: &str) {}
}

#[derive(Clone)]
pub(super) struct Entry {
    pub offset: u64,
    pub len: usize,
    pub hash: String,
    pub kind: String,
    pub created_at: String,
    pub turn_id: Option<String>,
    pub(super) read_only: bool,
    pub branch_seq: Option<u64>,
    pub control: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialMeta {
    session_id: String,
    turn_id: String,
    record_seq: u64,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    speaker_id: Option<String>,
    created_at: String,
    grammar_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch_seq: Option<u64>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Part {
    part_seq: u64,
    delta: String,
    chunk_seq: u64,
}
struct Partial {
    meta: PartialMeta,
    path: PathBuf,
    log: Journal,
    text: String,
    part_seq: u64,
    chunk_seq: u64,
}

/// 输出目标由引擎登记，模型不能选 kind / speakerId。
#[derive(Debug, Clone)]
pub enum Target {
    Narration,
    Character(String),
}
impl Target {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Narration => "narration",
            Self::Character(_) => "characterSpeech",
        }
    }
    pub fn speaker(&self) -> Option<&str> {
        match self {
            Self::Narration => None,
            Self::Character(id) => Some(id),
        }
    }
}

pub struct Session {
    pub(super) reference_key: String,
    pub(super) effective: Option<std::collections::BTreeSet<u64>>,
    pub(super) pending_world: bool,
    pub(super) pending_history: bool,
    pub(super) header: Header,
    pub(super) path: PathBuf,
    pub(super) index: BTreeMap<u64, Entry>,
    pub(super) view_epoch: String,
    pub(super) last_event_seq: u64,
    pub(super) history_revision: u64,
    pub(super) needs_recovery: bool,
    pub(super) read_only: bool,
    log: Journal,
    pub(super) next_seq: u64,
    pub(super) frozen: bool,
    active: Option<Partial>,
    events: Arc<dyn RecordEvents>,
    pending_events: BTreeMap<u64, (u64, Appended)>,
}
impl Session {
    #[cfg(test)]
    pub(crate) fn fail_sync(&mut self) {
        self.log
            .inject_fault(mythos_store::journal::FaultStage::Sync);
    }
    /// 新建会话只允许冻结且校验过的 header，不覆盖历史。
    /// # Errors
    /// 格式或排他创建失败返回存储错误。
    pub fn create(path: PathBuf, header: Header, events: Arc<dyn RecordEvents>) -> Result<Self> {
        header.validate()?;
        let log = Journal::create(&path, format::line(&header)?.as_bytes())?;
        Ok(Self::empty(path, header, log, events))
    }
    fn empty(path: PathBuf, header: Header, log: Journal, events: Arc<dyn RecordEvents>) -> Self {
        Self {
            reference_key: format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()),
            header,
            path,
            index: BTreeMap::new(),
            view_epoch: uuid::Uuid::new_v4().to_string(),
            last_event_seq: 0,
            history_revision: 0,
            needs_recovery: false,
            read_only: false,
            log,
            next_seq: 1,
            frozen: false,
            active: None,
            events,
            pending_events: BTreeMap::new(),
            effective: None,
            pending_world: false,
            pending_history: false,
        }
    }
    /// 扫描完整行后恢复唯一 sidecar；旧窗口 / LLM 序号不跨进程复活。
    /// # Errors
    /// 身份冲突、中间损坏、多活跃 sidecar 或恢复写入失败均禁止继续写入。
    pub fn open(path: PathBuf, events: Arc<dyn RecordEvents>) -> std::result::Result<Self, Fault> {
        repair_tail(&path)?;
        let mut header = None;
        let mut entries = BTreeMap::new();
        let mut next = 1;
        let mut read_only = false;
        mythos_store::journal::scan(&path, MAX_LINE, &mut |offset, line| {
            if offset == 0 {
                let parsed: Header =
                    mythos_json::from_value(format::json(line)?).map_err(decode_error)?;
                parsed.validate()?;
                header = Some(parsed);
                return Ok(());
            }
            let parsed = format::parse(line)?;
            if parsed.seq < next {
                return Err(format::corrupt());
            }
            next = parsed.seq.checked_add(1).ok_or_else(format::corrupt)?;
            read_only |= parsed.read_only;
            entries.insert(parsed.seq, entry(offset, &parsed));
            Ok(())
        })?;
        let header = header.ok_or_else(format::corrupt)?;
        let log = Journal::open(&path)?;
        let mut session = Self::empty(path, header, log, events);
        session.index = entries;
        session.next_seq = next;
        session.read_only = read_only;
        session.validate_references()?;
        for &seq in session.index.keys() {
            if let Some(Body::System { code, .. }) =
                session.read(seq)?.body.as_ref().map(|r| &r.body)
            {
                session.pending_world |= matches!(
                    code.as_str(),
                    "settlementPlanned" | "sceneAdvanced" | "sessionEnded"
                );
                session.pending_history |= code == "historyFork";
            }
        }
        session.needs_recovery = session.pending_world || session.pending_history;
        let sidecars = sidecars(&session.path, &session.header.session_id)?;
        if sidecars.len() > 1 {
            return Err(format::corrupt().into());
        }
        if let Some(path) = sidecars.first() {
            if session.read_only {
                session.needs_recovery = true;
            } else {
                session.recover_partial(path)?;
            }
        }
        Ok(session)
    }
    /// 冻结 header 只借出不可变引用，不允许消费方改写身份或前缀。
    pub fn header(&self) -> &Header {
        &self.header
    }
    pub fn view_epoch(&self) -> &str {
        &self.view_epoch
    }
    pub fn last_event_seq(&self) -> u64 {
        self.last_event_seq
    }
    pub fn history_revision(&self) -> u64 {
        self.history_revision
    }
    /// 单写者内预备关联状态时使用；调用方必须持有会话锁直到追加完成。
    pub(crate) fn next_sequence(&self) -> u64 {
        self.next_seq
    }
    pub fn needs_recovery(&self) -> bool {
        self.needs_recovery || self.pending_world || self.pending_history || self.read_only
    }
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }
    /// 索引和原行 hash 再核验，避免缓存指向外部修改后的文件。
    /// # Errors
    /// 未知 seq 或行内容改变返回存储错误。
    pub fn read(&self, seq: u64) -> Result<Parsed> {
        let index = self.index.get(&seq).ok_or_else(format::corrupt)?;
        let raw = mythos_store::journal::read_at(&self.path, index.offset, index.len, MAX_LINE)?;
        if format::hash(raw.as_bytes()) != index.hash {
            return Err(format::corrupt());
        }
        format::parse(&raw)
    }
    pub(super) fn known(&self, seq: u64) -> Result<Record> {
        self.read(seq)?.body.ok_or_else(format::corrupt)
    }
    fn validate_references(&self) -> Result<()> {
        if self.read_only {
            return Ok(());
        }
        for &seq in self.index.keys() {
            self.check_references(&self.known(seq)?)?;
        }
        Ok(())
    }
    pub(super) fn check_references(&self, record: &Record) -> Result<()> {
        if let Body::Narration { turn_id, .. } | Body::CharacterSpeech { turn_id, .. } =
            &record.body
            && self.index.iter().any(|(&seq, index)| {
                seq < record.seq
                    && matches!(index.kind.as_str(), "narration" | "characterSpeech")
                    && index.turn_id.as_deref() == Some(turn_id)
            })
        {
            return Err(format::corrupt());
        }
        let path = self
            .path_at(record.branch_seq.unwrap_or(0))
            .map_err(recovery_store_error)?;
        let before = |seq: u64| seq < record.seq && path.contains(&seq);
        let find_plan = |id: &str| -> Result<super::facts::CheckPlan> {
            for &seq in path.iter().rev().filter(|seq| **seq < record.seq) {
                if let Some(Body::System { code, data, .. }) = self.read(seq)?.body.map(|r| r.body)
                    && code == "checkPlanned"
                    && let super::facts::Fact::CheckPlanned { plan, .. } =
                        super::facts::Fact::decode(&code, &data)?
                    && plan.plan_id == id
                {
                    return Ok(plan);
                }
            }
            Err(format::corrupt())
        };
        match &record.body {
            Body::Dice {
                expression,
                modifiers,
                plan_id,
                ..
            } => {
                let plan = find_plan(plan_id)?;
                if &plan.expression != expression
                    || format::line(&plan.modifiers)? != format::line(modifiers)?
                {
                    return Err(format::corrupt());
                }
            }
            Body::Check {
                dice_seq,
                plan_id,
                rule_id,
                dc,
                ..
            } => {
                let plan = find_plan(plan_id)?;
                if !before(*dice_seq)
                    || &plan.rule_id != rule_id
                    || plan.result_policy.dc != *dc
                    || !matches!(self.read(*dice_seq)?.body.map(|r|r.body),Some(Body::Dice{plan_id:previous,..}) if previous==*plan_id)
                {
                    return Err(format::corrupt());
                }
            }
            Body::System { code, data, .. }
                if format::registered_system(code) && data["version"] == 1 =>
            {
                let fact = super::facts::Fact::decode(code, data)?;
                match fact {
                    super::facts::Fact::RoundAccepted { input_seq, .. } => {
                        if !before(input_seq)
                            || !matches!(
                                self.read(input_seq)?.body.map(|r| r.body),
                                Some(Body::PlayerSpeech { .. })
                            )
                        {
                            return Err(format::corrupt());
                        }
                    }
                    super::facts::Fact::SettlementPlanned { narrative_seq, .. } => {
                        if !before(narrative_seq)
                            || !matches!(
                                self.read(narrative_seq)?.body.map(|r| r.body),
                                Some(
                                    Body::Narration {
                                        terminal: Terminal::Completed { .. },
                                        ..
                                    } | Body::CharacterSpeech {
                                        terminal: Terminal::Completed { .. },
                                        ..
                                    }
                                )
                            )
                        {
                            return Err(format::corrupt());
                        }
                    }
                    super::facts::Fact::RoundSettled {
                        narrative_seq,
                        settlement_seq,
                        ..
                    } => {
                        if !before(narrative_seq) || !before(settlement_seq) {
                            return Err(format::corrupt());
                        }
                    }
                    super::facts::Fact::RoundEnded { through_seq, .. }
                    | super::facts::Fact::AbandonCheckpoint {
                        checkpoint_seq: through_seq,
                        ..
                    } if !before(through_seq) => {
                        return Err(format::corrupt());
                    }
                    _ => {}
                }
            }
            Body::Recap {
                from_seq,
                through_seq,
                source_hash,
                ..
            } => {
                let mut digest = sha2::Sha256::new();
                use sha2::Digest;
                let mut source = Vec::new();
                for &seq in path
                    .iter()
                    .filter(|seq| **seq < record.seq && self.index[seq].kind == "recap")
                {
                    if let Some(Body::Recap {
                        through_seq: previous,
                        ..
                    }) = self.read(seq)?.body.map(|r| r.body)
                        && *from_seq <= previous
                    {
                        return Err(format::corrupt());
                    }
                }
                for &seq in path
                    .iter()
                    .filter(|seq| **seq >= *from_seq && **seq <= *through_seq)
                {
                    let parsed = self.read(seq)?;
                    if matches!(
                        parsed.body.as_ref().map(|r| &r.body),
                        Some(
                            Body::Narration {
                                terminal: Terminal::Cancelled | Terminal::Failed { .. },
                                ..
                            } | Body::CharacterSpeech {
                                terminal: Terminal::Cancelled | Terminal::Failed { .. },
                                ..
                            }
                        )
                    ) || matches!(parsed.body.as_ref().map(|r|&r.body),Some(Body::System{code,..}) if code=="orphan")
                    {
                        return Err(format::corrupt());
                    }
                    if !matches!(
                        parsed.body.as_ref().map(|r| &r.body),
                        Some(Body::Recap { .. })
                    ) {
                        source.push(seq);
                        digest.update(parsed.raw.as_bytes());
                    }
                }
                if source.first() != Some(from_seq)
                    || source.last() != Some(through_seq)
                    || format!("sha256:{:x}", digest.finalize()) != *source_hash
                {
                    return Err(format::corrupt());
                }
            }
            _ => {}
        }
        Ok(())
    }
    /// 单写者分配不可复用物理身份；生成 partial 期间不接纳其他追加。
    /// # Errors
    /// 只读 / pending / 冻结 / 活跃生成及容量越界拒绝。
    pub fn append(&mut self, body: Body) -> std::result::Result<u64, Fault> {
        if matches!(&body, Body::Recap { .. }) {
            return Err(Fault::new("engine.invalid-phase", "摘要必须经冻结候选提交"));
        }
        if matches!(&body,Body::System{code,..} if matches!(code.as_str(),"settlementPlanned"|"sceneAdvanced"|"sessionEnded"|"historyFork"))
        {
            return Err(Fault::new(
                "engine.invalid-phase",
                "世界意图必须经登记解释器提交",
            ));
        }
        self.writable()?;
        if self.active.is_some() {
            return Err(Fault::busy());
        }
        let seq = self.reserve()?;
        self.commit(Record {
            seq,
            created_at: format::now(),
            branch_seq: (self.history_revision > 0).then_some(self.history_revision),
            body,
        })?;
        Ok(seq)
    }
    pub(super) fn writable(&self) -> std::result::Result<(), Fault> {
        if self.frozen
            || self.read_only
            || self.needs_recovery
            || self.pending_world
            || self.pending_history
        {
            return Err(Fault::new("engine.invalid-phase", "记录需要恢复或只读"));
        }
        Ok(())
    }
    pub(super) fn reserve(&mut self) -> std::result::Result<u64, Fault> {
        if self.next_seq > MAX_SEQ {
            return Err(Fault::new("store.corrupt", "记录身份容量耗尽"));
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        Ok(seq)
    }
    pub(super) fn commit(&mut self, record: Record) -> std::result::Result<(), Fault> {
        let seq = record.seq;
        self.persist_intent(record)?;
        self.confirm_intent(seq)
    }
    pub(super) fn append_intent(&mut self, body: Body) -> std::result::Result<u64, Fault> {
        self.writable()?;
        if self.active.is_some() {
            return Err(Fault::busy());
        }
        let seq = self.reserve()?;
        self.persist_intent(Record {
            seq,
            created_at: format::now(),
            branch_seq: (self.history_revision > 0).then_some(self.history_revision),
            body,
        })?;
        Ok(seq)
    }
    fn persist_intent(&mut self, record: Record) -> std::result::Result<(), Fault> {
        record.validate().map_err(store_fault)?;
        self.check_references(&record).map_err(store_fault)?;
        let raw = format::line(&record).map_err(store_fault)?;
        let parsed = format::parse(&raw).map_err(store_fault)?;
        if parsed.read_only {
            return Err(Fault::new("engine.invalid-phase", "未知记录不允许写入"));
        }
        let notification = Appended {
            session_id: self.header.session_id.clone(),
            view_epoch: self.view_epoch.clone(),
            record_seq: record.seq,
            kind: parsed.kind.clone(),
            turn_id: turn_id(&record.body),
        };
        let seq = self.events.prepare(&notification)?;
        if seq <= self.last_event_seq || seq > MAX_SEQ {
            return Err(Fault::new("app.event-failed", "记录事件序号不合法"));
        }
        let offset = match self.log.append(raw.as_bytes(), true) {
            Ok(offset) => offset,
            Err(error) => {
                self.frozen = true;
                return Err(store_fault(error));
            }
        };
        self.index.insert(record.seq, entry(offset, &parsed));
        self.pending_events.insert(record.seq, (seq, notification));
        Ok(())
    }
    pub(super) fn confirm_intent(&mut self, record_seq: u64) -> std::result::Result<(), Fault> {
        let mut prepared = if let Some(prepared) = self.pending_events.remove(&record_seq) {
            prepared
        } else {
            let index = self.index.get(&record_seq).ok_or_else(Fault::not_found)?;
            let event = Appended {
                session_id: self.header.session_id.clone(),
                view_epoch: self.view_epoch.clone(),
                record_seq,
                kind: index.kind.clone(),
                turn_id: index.turn_id.clone(),
            };
            (self.events.prepare(&event)?, event)
        };
        if prepared.0 <= self.last_event_seq || prepared.0 > MAX_SEQ {
            return Err(Fault::event());
        }
        // 世界 / 分支确认可能切换代际，通知绑定确认后的视图。
        prepared.1.view_epoch = self.view_epoch.clone();
        self.last_event_seq = prepared.0;
        let _ = self.events.deliver(prepared.0, prepared.1);
        Ok(())
    }
    pub(super) fn pending_confirmations(&self) -> Vec<u64> {
        self.pending_events.keys().copied().collect()
    }
    fn ensure_partial(&mut self, turn: &str, target: &Target) -> std::result::Result<(), Fault> {
        self.writable()?;
        if self.index.values().any(|entry| {
            entry.turn_id.as_deref() == Some(turn)
                && matches!(entry.kind.as_str(), "narration" | "characterSpeech")
        }) {
            return Err(Fault::new("engine.invalid-phase", "生成块已经封口"));
        }
        if let Some(active) = &self.active {
            if active.meta.turn_id == turn
                && active.meta.kind == target.kind()
                && active.meta.speaker_id.as_deref() == target.speaker()
            {
                return Ok(());
            }
            return Err(Fault::busy());
        }
        if !format::valid_uuid(turn) || target.speaker().is_some_and(|id| !format::valid_id(id)) {
            return Err(Fault::bad_request());
        }
        let seq = self.reserve()?;
        let meta = PartialMeta {
            session_id: self.header.session_id.clone(),
            turn_id: turn.into(),
            record_seq: seq,
            kind: target.kind().into(),
            speaker_id: target.speaker().map(str::to_owned),
            created_at: format::now(),
            grammar_version: 1,
            branch_seq: (self.history_revision > 0).then_some(self.history_revision),
        };
        let path = self
            .path
            .with_file_name(format!("{}.{}.partial.jsonl", meta.session_id, turn));
        let log = match Journal::create(&path, format::line(&meta).map_err(store_fault)?.as_bytes())
        {
            Ok(log) => log,
            Err(error) => {
                self.frozen = true;
                return Err(store_fault(error));
            }
        };
        self.active = Some(Partial {
            meta,
            path,
            log,
            text: String::new(),
            part_seq: 0,
            chunk_seq: 0,
        });
        Ok(())
    }
    pub(super) fn delta(
        &mut self,
        turn: &str,
        target: &Target,
        chunk_seq: u64,
        text: &str,
        high: bool,
    ) -> std::result::Result<(), Fault> {
        self.ensure_partial(turn, target)?;
        let active = self.active.as_mut().expect("partial initialized");
        if text.is_empty()
            || text.len() > 8192
            || active.text.len() + text.len() > MAX_TEXT
            || chunk_seq <= active.chunk_seq
            || chunk_seq > MAX_SEQ
            || text.contains('\r')
        {
            return Err(Fault::bad_request());
        }
        let part = Part {
            part_seq: active.part_seq + 1,
            delta: text.into(),
            chunk_seq,
        };
        let raw = format::line(&part).map_err(store_fault)?;
        if let Err(error) = active.log.append(raw.as_bytes(), high) {
            self.frozen = active.log.is_frozen();
            return Err(store_fault(error));
        }
        active.text.push_str(text);
        active.part_seq = part.part_seq;
        active.chunk_seq = chunk_seq;
        Ok(())
    }
    pub(crate) fn seal(
        &mut self,
        turn: &str,
        target: &Target,
        terminal: &Terminal,
    ) -> std::result::Result<(), Fault> {
        self.ensure_partial(turn, target)?;
        let active = self.active.as_ref().expect("partial initialized");
        let record = generated(
            &active.meta,
            active.text.clone(),
            terminal.clone(),
            self.history_revision,
        );
        let path = active.path.clone();
        self.commit(record)?;
        self.active = None;
        // 清理失败不撤销已封口事实，也不让协调器改写已经提交的终态。
        let _ = mythos_store::journal::remove(&path);
        Ok(())
    }
    fn recover_partial(&mut self, path: &Path) -> std::result::Result<(), Fault> {
        repair_tail(path)?;
        let mut meta = None;
        let mut text = String::new();
        let mut part_seq = 0;
        let mut chunk_seq = 0;
        mythos_store::journal::scan(path, 64 * 1024, &mut |offset, line| {
            if offset == 0 {
                meta = Some(
                    mythos_json::from_value::<PartialMeta>(format::json(line)?)
                        .map_err(decode_error)?,
                );
                return Ok(());
            }
            let part: Part = mythos_json::from_value(format::json(line)?).map_err(decode_error)?;
            if part.part_seq != part_seq + 1
                || part.chunk_seq <= chunk_seq
                || part.chunk_seq > MAX_SEQ
                || part.delta.is_empty()
                || part.delta.len() > 8192
                || text.len() + part.delta.len() > MAX_TEXT
                || part.delta.contains('\r')
            {
                return Err(format::corrupt());
            }
            text.push_str(&part.delta);
            part_seq = part.part_seq;
            chunk_seq = part.chunk_seq;
            Ok(())
        })?;
        let meta = meta.ok_or_else(format::corrupt)?;
        if meta.session_id != self.header.session_id
            || !format::valid_uuid(&meta.turn_id)
            || meta.grammar_version != 1
            || meta.record_seq == 0
            || meta.record_seq > MAX_SEQ
            || meta.branch_seq.is_some_and(|seq| {
                seq == 0 || seq >= meta.record_seq || !self.index.contains_key(&seq)
            })
            || !matches!(meta.kind.as_str(), "narration" | "characterSpeech")
            || (meta.kind == "characterSpeech") != meta.speaker_id.is_some()
            || path.file_name()
                != Some(std::ffi::OsStr::new(&format!(
                    "{}.{}.partial.jsonl",
                    meta.session_id, meta.turn_id
                )))
        {
            return Err(format::corrupt().into());
        }
        let terminal =
            Terminal::failed(Fault::new("engine.interrupted", "进程中断，已提交正文保留"));
        let target = meta
            .speaker_id
            .as_ref()
            .map_or(Target::Narration, |id| Target::Character(id.clone()));
        let mut guard = mythos_llm::guard::Guard::new(super::grammar::guard(&target));
        if matches!(
            guard.push(&text),
            mythos_llm::guard::GuardStep::Stopped { .. }
        ) || guard.finish().truncated
        {
            return Err(format::corrupt().into());
        }
        let generated = generated(&meta, text.clone(), terminal, self.history_revision);
        generated.validate()?;
        let recovery_id = format::hash(
            format!("{}:{}:{}", meta.session_id, meta.turn_id, meta.record_seq).as_bytes(),
        );
        if let Some(index) = self.index.get(&meta.record_seq) {
            let existing = self
                .read(meta.record_seq)?
                .body
                .ok_or_else(format::corrupt)?;
            if index.turn_id.as_deref() != Some(&meta.turn_id)
                || index.kind != meta.kind
                || index.branch_seq != meta.branch_seq
                || matches!(&existing.body,Body::CharacterSpeech{speaker_id,..} if Some(speaker_id.as_str())!=meta.speaker_id.as_deref())
                || body_text(&existing.body) != Some(text.as_str())
            {
                return Err(format::corrupt().into());
            }
        } else if !text.is_empty() {
            if meta.record_seq < self.next_seq {
                return Err(format::corrupt().into());
            }
            self.commit(generated)?;
        }
        self.next_seq = self.next_seq.max(meta.record_seq + 1);
        let mut marked = false;
        for &seq in self.index.keys() {
            if let Some(existing) = self.read(seq)?.body
                && let Body::System {
                    code,
                    data,
                    related_seq,
                    turn_id,
                    ..
                } = &existing.body
                && code == "orphan"
                && data["recoveryId"] == recovery_id
            {
                if *related_seq != (!text.is_empty()).then_some(meta.record_seq)
                    || turn_id.as_deref() != Some(meta.turn_id.as_str())
                    || existing.branch_seq != meta.branch_seq
                {
                    return Err(format::corrupt().into());
                }
                marked = true;
            }
        }
        if !marked {
            let body = Body::System {
                code: "orphan".into(),
                message: "进程中断后的记录已核验，未提交尾文已丢弃".into(),
                related_seq: (!text.is_empty()).then_some(meta.record_seq),
                turn_id: Some(meta.turn_id),
                data: serde_json::json!({"version":1,"recoveryId":recovery_id}),
            };
            let seq = self.reserve()?;
            self.commit(Record {
                seq,
                created_at: format::now(),
                branch_seq: meta.branch_seq,
                body,
            })?;
        }
        mythos_store::journal::remove(path).map_err(Fault::from)
    }
    /// 正常关闭同步所有已开始边界，禁止持有旧 Arc 的调用方继续写入。
    /// # Errors
    /// 同步失败保留原文件并返回 store.*。
    pub fn close(&mut self) -> std::result::Result<(), Fault> {
        self.frozen = true;
        if let Some(active) = self.active.as_mut() {
            active.log.sync().map_err(store_fault)?;
        }
        self.log.sync().map_err(store_fault)
    }
    /// 只返回 partial 身份；正文由 LLM 快照提供。
    pub fn in_flight(&self) -> Option<(String, u64)> {
        self.active
            .as_ref()
            .map(|p| (p.meta.turn_id.clone(), p.meta.record_seq))
    }
}
fn generated(meta: &PartialMeta, text: String, terminal: Terminal, branch: u64) -> Record {
    let body = match &meta.speaker_id {
        Some(id) => Body::CharacterSpeech {
            speaker_id: id.clone(),
            text,
            turn_id: meta.turn_id.clone(),
            terminal,
        },
        None => Body::Narration {
            text,
            turn_id: meta.turn_id.clone(),
            terminal,
        },
    };
    Record {
        seq: meta.record_seq,
        created_at: meta.created_at.clone(),
        branch_seq: meta.branch_seq.or((branch > 0).then_some(branch)),
        body,
    }
}
pub fn body_text(body: &Body) -> Option<&str> {
    match body {
        Body::Narration { text, .. }
        | Body::CharacterSpeech { text, .. }
        | Body::PlayerSpeech { text, .. }
        | Body::Recap { text, .. } => Some(text),
        _ => None,
    }
}
fn turn_id(body: &Body) -> Option<String> {
    match body {
        Body::Narration { turn_id, .. } | Body::CharacterSpeech { turn_id, .. } => {
            Some(turn_id.clone())
        }
        Body::System { turn_id, .. } => turn_id.clone(),
        _ => None,
    }
}
fn entry(offset: u64, parsed: &Parsed) -> Entry {
    Entry {
        offset,
        len: parsed.raw.len(),
        hash: format::hash(parsed.raw.as_bytes()),
        kind: parsed.kind.clone(),
        created_at: parsed.created_at.clone(),
        turn_id: parsed.body.as_ref().and_then(|r| turn_id(&r.body)),
        read_only: parsed.read_only,
        control: matches!(parsed.body.as_ref().map(|r|&r.body),Some(Body::System{code,..}) if code=="historyFork"),
        branch_seq: parsed.body.as_ref().and_then(|r| r.branch_seq),
    }
}
fn decode_error(_: mythos_json::Error) -> mythos_store::error::StoreError {
    format::corrupt()
}
fn recovery_store_error(error: Fault) -> mythos_store::error::StoreError {
    let code = match error.code.as_str() {
        "store.io" => "io",
        "store.disk-full" => "disk-full",
        "store.permission" => "permission",
        "store.not-found" => "not-found",
        "store.locked" => "locked",
        _ => return format::corrupt(),
    };
    mythos_store::error::StoreError::Io {
        code,
        source: std::io::Error::other("record reference read failed"),
    }
}
pub fn store_fault(error: mythos_store::error::StoreError) -> Fault {
    Fault::new(format!("store.{}", error.code()), "记录存储操作失败")
}
fn sidecars(path: &Path, session: &str) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(path.parent().ok_or_else(format::corrupt)?)
        .map_err(mythos_store::error::StoreError::from_io)?
    {
        let entry = entry.map_err(mythos_store::error::StoreError::from_io)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&format!("{session}.")) && name.ends_with(".partial.jsonl") {
            found.push(entry.path());
        }
    }
    Ok(found)
}
fn repair_tail(path: &Path) -> Result<()> {
    let backup = path.with_extension(format!("recovery-{}.bak", uuid::Uuid::new_v4()));
    mythos_store::journal::preserve_and_repair(path, &backup).map(|_| ())
}

/// 输出提交适配只捕获会话与可信目标；阻塞文件 I/O 放到 blocking pool。
pub struct PersistentWriter {
    pub session: Arc<Mutex<Session>>,
    pub target: Target,
    pub high: bool,
}
impl OutputWriter for PersistentWriter {
    fn begin<'a>(&'a self, turn_id: &'a str) -> BoxFuture<'a, std::result::Result<(), Fault>> {
        let session = self.session.clone();
        let target = self.target.clone();
        let turn = turn_id.to_owned();
        Box::pin(async move {
            crate::blocking::run(move || {
                session
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .ensure_partial(&turn, &target)
            })
            .await
        })
    }
    fn append<'a>(
        &'a self,
        turn_id: &'a str,
        chunk_seq: u64,
        text: &'a str,
    ) -> BoxFuture<'a, std::result::Result<(), Fault>> {
        let session = self.session.clone();
        let target = self.target.clone();
        let turn = turn_id.to_owned();
        let text = text.to_owned();
        let high = self.high;
        Box::pin(async move {
            crate::blocking::run(move || {
                session
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .delta(&turn, &target, chunk_seq, &text, high)
            })
            .await
        })
    }
    fn finish<'a>(
        &'a self,
        turn_id: &'a str,
        terminal: &'a Terminal,
    ) -> BoxFuture<'a, std::result::Result<(), Fault>> {
        let session = self.session.clone();
        let target = self.target.clone();
        let turn = turn_id.to_owned();
        let terminal = terminal.clone();
        Box::pin(async move {
            crate::blocking::run(move || {
                session
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .seal(&turn, &target, &terminal)
            })
            .await
        })
    }
}

#[cfg(test)]
mod tests;
