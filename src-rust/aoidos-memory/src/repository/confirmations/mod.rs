//! 待确认队列的稳定身份、版本 CAS 与来源复核；答案不直接改权威实体目录。

use super::*;

const MAX_QUESTION_BYTES: usize = 64 * 1024;
const MAX_QUESTION_ANSWER_BYTES: usize = 8 * 1024;
const MAX_QUESTION_SOURCES: usize = 32;
const MAX_QUESTION_CANDIDATES: usize = 32;
const MAX_PENDING_QUESTIONS_PER_RUN: u64 = 512;
const MAX_QUESTION_PAGE_SIZE: usize = 32;
const MAX_QUESTION_PAGE_SCAN: usize = 256;

/// 有界待确认队列的快照游标；绑定历史修订和队列上界，翻页期间新问题留给刷新读取。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionCursor {
    /// 读取上一页时观察到的世界历史版本。
    pub history_revision: u64,
    /// 快照读取时队列中最大的 SQLite 问题序号。
    pub snapshot_max_sequence: u64,
    /// 上一页最后扫描的 SQLite 问题序号。
    pub after_sequence: u64,
}

/// 当前有效的待确认问题分页；失效项目在返回前转入持久 `invalidated` 状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionPage {
    /// 当前仍有效且可供回答的问题。
    pub questions: Vec<MemoryQuestion>,
    /// 存在后续扫描项时返回；读取下一页前须原样传回。
    pub next_cursor: Option<QuestionCursor>,
    /// 本页复核时的世界历史修订。
    pub history_revision: u64,
}

impl Repository {
    /// 登记或更新同一事件的问题；候选变化递增 revision，不基于文案相似度合并。
    ///
    /// # Errors
    /// 来源、实体目录 revision 或问题形状失效时拒绝；队列满时保留调用方素材供 deferred 恢复。
    pub fn open_question(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        run_id: &str,
        draft: &QuestionDraft,
    ) -> Result<MemoryQuestion> {
        self.ensure_writable(conn, run_id)?;
        let (run, boundary) = self.boundary(conn, run_id, port)?;
        validate_draft(draft, &boundary)?;
        let draft = normalized_draft(draft)?;
        if !self.valid_sources(&run, &draft.identity.sources, port)? {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        if !mentioned_name_is_source_backed(&draft, port)? {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        let question_key = build_question_key(run_id, &draft.identity)?;
        let draft_json = canonical(&draft)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;

        if let Some(mut question) = self.pending_question(&tx, run_id, &question_key)? {
            if question.draft.identity != draft.identity {
                return Err(Error::Corrupt);
            }
            if question.draft == draft {
                tx.commit().map_err(sql)?;
                return Ok(question);
            }
            question.revision = question
                .revision
                .checked_add(1)
                .filter(|revision| *revision <= MAX_INTEGER)
                .ok_or(Error::Rejected(Reason::CapacityBlocked))?;
            question.draft = draft.clone();
            let updated = tx
                .execute(
                    "UPDATE memory_questions SET revision=?1,history_revision=?2,entity_revision=?3,evidence_revision=?4,draft_json=?5 WHERE question_id=?6 AND status='pending' AND revision=?7",
                    params![
                        question.revision,
                        draft.history_revision,
                        draft.entity_revision,
                        draft.evidence_revision,
                        draft_json,
                        question.question_id,
                        question.revision - 1
                    ],
                )
                .map_err(sql)?;
            if updated != 1 {
                return Err(Error::Rejected(Reason::StaleVersion));
            }
            question.answer = None;
            tx.commit().map_err(sql)?;
            return Ok(question);
        }

        let pending: u64 = tx
            .query_row(
                "SELECT count(*) FROM memory_questions WHERE run_id=?1 AND status='pending'",
                [run_id],
                |row| row.get(0),
            )
            .map_err(sql)?;
        if pending >= MAX_PENDING_QUESTIONS_PER_RUN {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }

        let question = MemoryQuestion {
            question_id: new_id(),
            run_id: run_id.into(),
            question_key,
            revision: 1,
            draft: draft.clone(),
            status: QuestionStatus::Pending,
            answer: None,
        };
        tx.execute(
            "INSERT INTO memory_questions(question_id,run_id,question_key,revision,history_revision,entity_revision,evidence_revision,draft_json,status) VALUES(?1,?2,?3,1,?4,?5,?6,?7,'pending')",
            params![
                question.question_id,
                run_id,
                question.question_key.as_slice(),
                draft.history_revision,
                draft.entity_revision,
                draft.evidence_revision,
                draft_json
            ],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(question)
    }

    /// 回答只接受当前 revision / historyRevision 和原候选；新名称仅形成待登记提议。
    /// 跳过仍核验当前版本和来源，并保留 pending 状态及空答案供稍后回答。
    ///
    /// # Errors
    /// 迟到回答、无效来源或不属于候选集合的实体拒绝，不修改旧问题状态。
    pub fn resolve_question(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        run_id: &str,
        resolution: QuestionResolution,
    ) -> Result<MemoryQuestion> {
        self.ensure_writable(conn, run_id)?;
        if !valid_id(&resolution.question_id) {
            return Err(reject());
        }
        let question = self.question(conn, run_id, &resolution.question_id)?;
        if question.status != QuestionStatus::Pending
            || question.revision != resolution.expected_revision
            || question.draft.history_revision != resolution.expected_history_revision
        {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        let (run, boundary) = self.boundary(conn, run_id, port)?;
        if boundary.history_revision != resolution.expected_history_revision
            || boundary.entity_revision != question.draft.entity_revision
            || boundary.evidence_revision != question.draft.evidence_revision
        {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        if !self.valid_sources(&run, &question.draft.identity.sources, port)? {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        if !mentioned_name_is_source_backed(&question.draft, port)? {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        validate_answer(&resolution.answer, &question.draft)?;
        if matches!(resolution.answer, QuestionAnswer::Skip) {
            return Ok(question);
        }
        let answer_json = canonical(&resolution.answer)?;
        let changed = conn
            .execute(
                "UPDATE memory_questions SET status='resolved',answer_json=?1 WHERE question_id=?2 AND run_id=?3 AND status='pending' AND revision=?4 AND history_revision=?5",
                params![
                    answer_json,
                    resolution.question_id,
                    run_id,
                    resolution.expected_revision,
                    resolution.expected_history_revision
                ],
            )
            .map_err(sql)?;
        if changed != 1 {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        Ok(MemoryQuestion {
            status: QuestionStatus::Resolved,
            answer: Some(resolution.answer),
            ..question
        })
    }

    /// 读取已持久问题；游标接口由设置 / 查询任务提供。
    pub fn question(
        &self,
        conn: &Connection,
        run_id: &str,
        question_id: &str,
    ) -> Result<MemoryQuestion> {
        if !valid_id(run_id) || !valid_id(question_id) {
            return Err(reject());
        }
        let row = conn
            .query_row(
                "SELECT question_id,run_id,length(question_key),question_key,revision,history_revision,entity_revision,evidence_revision,length(draft_json),draft_json,status,length(answer_json),answer_json FROM memory_questions WHERE run_id=?1 AND question_id=?2",
                params![run_id, question_id],
                read_question,
            )
            .optional()
            .map_err(sql)?
            .ok_or(Error::NotFound)?;
        decode_question(row)
    }

    /// 有界扫描待确认问题，并将来源或因果快照失效的项目持久标为 invalidated。
    ///
    /// # Errors
    /// 非法页大小 / 游标拒绝；游标历史修订过期时返回 staleVersion；来源读取故障原样传播。
    pub fn pending_questions(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        run_id: &str,
        cursor: Option<&QuestionCursor>,
        limit: usize,
    ) -> Result<QuestionPage> {
        if !(1..=MAX_QUESTION_PAGE_SIZE).contains(&limit) {
            return Err(reject());
        }
        self.ensure_writable(conn, run_id)?;
        let (run, boundary) = self.boundary(conn, run_id, port)?;
        if let Some(cursor) = cursor {
            if cursor.history_revision > MAX_INTEGER
                || cursor.snapshot_max_sequence > MAX_INTEGER
                || cursor.after_sequence > cursor.snapshot_max_sequence
            {
                return Err(reject());
            }
            if cursor.history_revision != boundary.history_revision {
                return Err(Error::Rejected(Reason::StaleVersion));
            }
        }

        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let snapshot_max_sequence = match cursor {
            Some(cursor) => cursor.snapshot_max_sequence,
            None => {
                let sequence = tx
                    .query_row(
                    "SELECT coalesce(max(rowid),0) FROM memory_questions WHERE run_id=?1 AND status='pending'",
                    [run_id],
                    |row| row.get::<_, u64>(0),
                )
                    .map_err(sql)?;
                if sequence > MAX_INTEGER {
                    return Err(Error::Corrupt);
                }
                sequence
            }
        };
        let after = cursor.map_or(0, |cursor| cursor.after_sequence);
        let mut statement = tx
            .prepare(
                "SELECT rowid,question_id FROM memory_questions WHERE run_id=?1 AND status='pending' AND rowid>?2 AND rowid<=?3 ORDER BY rowid LIMIT ?4",
            )
            .map_err(sql)?;
        let ids = statement
            .query_map(
                params![
                    run_id,
                    after,
                    snapshot_max_sequence,
                    (MAX_QUESTION_PAGE_SCAN + 1) as u64
                ],
                read_question_position,
            )
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        drop(statement);
        let has_more = ids.len() > MAX_QUESTION_PAGE_SCAN;
        let mut questions = Vec::with_capacity(limit);
        let mut last_scanned = None;
        for (sequence, question_id) in ids.iter().take(MAX_QUESTION_PAGE_SCAN) {
            last_scanned = Some(*sequence);
            let question = self.question(&tx, run_id, question_id)?;
            let snapshot_is_current = question.draft.history_revision == boundary.history_revision
                && question.draft.entity_revision == boundary.entity_revision
                && question.draft.evidence_revision == boundary.evidence_revision;
            let source_is_current =
                self.valid_sources(&run, &question.draft.identity.sources, port)?
                    && mentioned_name_is_source_backed(&question.draft, port)?;
            if snapshot_is_current && source_is_current {
                questions.push(question);
                if questions.len() == limit {
                    break;
                }
            } else {
                let invalidated = tx
                    .execute(
                        "UPDATE memory_questions SET status='invalidated' WHERE question_id=?1 AND run_id=?2 AND status='pending' AND revision=?3 AND history_revision=?4 AND entity_revision=?5 AND evidence_revision=?6",
                        params![
                            question.question_id,
                            run_id,
                            question.revision,
                            question.draft.history_revision,
                            question.draft.entity_revision,
                            question.draft.evidence_revision
                        ],
                    )
                    .map_err(sql)?;
                if invalidated != 1 {
                    return Err(Error::Rejected(Reason::StaleVersion));
                }
            }
        }
        let has_unscanned = last_scanned
            .is_some_and(|last| ids.iter().any(|(sequence, _)| *sequence > last) || has_more);
        let next_cursor = if has_unscanned {
            last_scanned.map(|after_sequence| QuestionCursor {
                history_revision: boundary.history_revision,
                snapshot_max_sequence,
                after_sequence,
            })
        } else {
            None
        };
        tx.commit().map_err(sql)?;
        Ok(QuestionPage {
            questions,
            next_cursor,
            history_revision: boundary.history_revision,
        })
    }

    fn pending_question(
        &self,
        conn: &Connection,
        run_id: &str,
        key: &Hash,
    ) -> Result<Option<MemoryQuestion>> {
        let row = conn
            .query_row(
                "SELECT question_id,run_id,length(question_key),question_key,revision,history_revision,entity_revision,evidence_revision,length(draft_json),draft_json,status,length(answer_json),answer_json FROM memory_questions WHERE run_id=?1 AND question_key=?2 AND status='pending'",
                params![run_id, key.as_slice()],
                read_question,
            )
            .optional()
            .map_err(sql)?;
        row.map(decode_question).transpose()
    }
}

fn validate_draft(draft: &QuestionDraft, boundary: &Boundary) -> Result<()> {
    if draft.version != 1
        || draft.history_revision != boundary.history_revision
        || draft.entity_revision != boundary.entity_revision
        || draft.evidence_revision != boundary.evidence_revision
        || draft.history_revision > MAX_INTEGER
        || draft.entity_revision > MAX_INTEGER
        || draft.evidence_revision > MAX_INTEGER
        || draft.identity.event_slot > 1_000_000
        || draft.identity.sources.is_empty()
        || draft.identity.sources.len() > MAX_QUESTION_SOURCES
        || draft.candidates.is_empty()
        || draft.candidates.len() > MAX_QUESTION_CANDIDATES
        || draft.mentioned_name.as_ref().is_some_and(|name| {
            name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control)
        })
        || (draft.identity.kind == QuestionKind::MemoryClassification
            && draft.mentioned_name.is_some())
    {
        return Err(reject());
    }
    let sources = draft
        .identity
        .sources
        .iter()
        .map(canonical)
        .collect::<Result<BTreeSet<_>>>()?;
    if sources.len() != draft.identity.sources.len()
        || draft
            .identity
            .sources
            .iter()
            .any(|source| source.validate().is_err())
    {
        return Err(reject());
    }
    let candidates = draft
        .candidates
        .iter()
        .map(canonical)
        .collect::<Result<BTreeSet<_>>>()?;
    if candidates.len() != draft.candidates.len()
        || draft
            .candidates
            .iter()
            .any(|candidate| match (draft.identity.kind, candidate) {
                (QuestionKind::EntityAssociation, QuestionCandidate::Entity { entity_id }) => {
                    !valid_name(entity_id)
                }
                (
                    QuestionKind::MemoryClassification,
                    QuestionCandidate::MemoryClassification { .. },
                ) => false,
                _ => true,
            })
    {
        return Err(reject());
    }
    Ok(())
}

fn normalized_draft(draft: &QuestionDraft) -> Result<QuestionDraft> {
    let mut normalized = draft.clone();
    normalized.identity.sources = normalized_sources(&draft.identity.sources)?;
    Ok(normalized)
}

fn mentioned_name_is_source_backed(draft: &QuestionDraft, port: &dyn CausalPort) -> Result<bool> {
    let Some(name) = draft.mentioned_name.as_deref() else {
        return Ok(true);
    };
    for source in &draft.identity.sources {
        if port
            .source_text(source)?
            .is_some_and(|text| text.contains(name))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn normalized_sources(sources: &[SourceRef]) -> Result<Vec<SourceRef>> {
    let mut sources = sources
        .iter()
        .map(|source| Ok((canonical(source)?, source.clone())))
        .collect::<Result<Vec<_>>>()?;
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(sources.into_iter().map(|(_, source)| source).collect())
}

fn validate_answer(answer: &QuestionAnswer, draft: &QuestionDraft) -> Result<()> {
    match answer {
        QuestionAnswer::SelectCandidate { candidate }
            if draft.candidates.iter().any(|item| item == candidate) =>
        {
            Ok(())
        }
        QuestionAnswer::ProposeEntity { display_name }
            if draft.identity.kind == QuestionKind::EntityAssociation
                && !display_name.trim().is_empty()
                && display_name.len() <= 256
                && !display_name.contains('\0')
                && !display_name.chars().any(char::is_control) =>
        {
            Ok(())
        }
        QuestionAnswer::None | QuestionAnswer::Skip => Ok(()),
        _ => Err(reject()),
    }
}

fn build_question_key(run_id: &str, identity: &QuestionIdentity) -> Result<Hash> {
    let mut normalized = identity.clone();
    normalized.sources = normalized_sources(&identity.sources)?;
    Ok(hash(&canonical(&(run_id, normalized))?))
}

type QuestionRow = (
    String,
    String,
    BoundedBlob,
    u64,
    u64,
    u64,
    u64,
    BoundedBlob,
    String,
    Option<Vec<u8>>,
);

fn read_question_position(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, String)> {
    Ok((row.get(0)?, row.get(1)?))
}

fn read_question(row: &rusqlite::Row<'_>) -> rusqlite::Result<QuestionRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        read_bounded_blob(row, 2, 3, 32)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        read_bounded_blob(row, 8, 9, MAX_QUESTION_BYTES)?,
        row.get(10)?,
        read_optional_bounded_blob(row, 11, 12, MAX_QUESTION_ANSWER_BYTES)?,
    ))
}

fn read_optional_bounded_blob(
    row: &rusqlite::Row<'_>,
    length_index: usize,
    blob_index: usize,
    limit: usize,
) -> rusqlite::Result<Option<Vec<u8>>> {
    use rusqlite::types::ValueRef;
    let length: Option<i64> = row.get(length_index)?;
    match (length, row.get_ref(blob_index)?) {
        (None, ValueRef::Null) => Ok(None),
        (Some(length), ValueRef::Blob(bytes))
            if length >= 0 && length as usize <= limit && length as usize == bytes.len() =>
        {
            Ok(Some(bytes.to_vec()))
        }
        _ => Ok(Some(Vec::new())),
    }
}

fn decode_question(row: QuestionRow) -> Result<MemoryQuestion> {
    let (
        question_id,
        run_id,
        key,
        revision,
        history_revision,
        entity_revision,
        evidence_revision,
        raw,
        status,
        answer,
    ) = row;
    let question_key: Hash = key.into_result()?.try_into().map_err(|_| Error::Corrupt)?;
    let draft: QuestionDraft = decode(&raw.into_result()?, MAX_QUESTION_BYTES)?;
    let status: QuestionStatus = decode_enum(&status)?;
    let answer = answer
        .map(|bytes| decode::<QuestionAnswer>(&bytes, MAX_QUESTION_ANSWER_BYTES))
        .transpose()?;
    if !valid_id(&question_id)
        || !valid_id(&run_id)
        || revision == 0
        || draft.version != 1
        || draft.history_revision != history_revision
        || draft.entity_revision != entity_revision
        || draft.evidence_revision != evidence_revision
        || (status == QuestionStatus::Resolved) != answer.is_some()
    {
        return Err(Error::Corrupt);
    }
    let expected = build_question_key(&run_id, &draft.identity)?;
    if question_key != expected {
        return Err(Error::Corrupt);
    }
    validate_draft(
        &draft,
        &Boundary {
            script_id: "stored".into(),
            session_id: draft
                .identity
                .sources
                .first()
                .map(|source| source.session_id.clone())
                .unwrap_or_default(),
            history_revision,
            entity_revision,
            evidence_revision,
        },
    )
    .map_err(|_| Error::Corrupt)?;
    if let Some(answer) = &answer {
        validate_answer(answer, &draft).map_err(|_| Error::Corrupt)?;
    }
    Ok(MemoryQuestion {
        question_id,
        run_id,
        question_key,
        revision,
        draft,
        status,
        answer,
    })
}

#[cfg(test)]
mod tests;
