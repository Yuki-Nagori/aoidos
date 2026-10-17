//! 周目刻痕的幂等登记、有界归档，以及节点进度与提示状态。

use super::*;
use crate::model::{
    ABSOLUTE_CYCLE_CAPACITY, ACTIVE_CYCLE_CAPACITY, BeginSceneProgress, CYCLE_HINT_COOLDOWN_ROUNDS,
    ClaimSceneHint, CompleteSceneProgress, CycleMarkV1, CycleSourceRef, MAX_SCENE_EVIDENCE_BYTES,
    MAX_SCENE_PROGRESS_EVIDENCE, MAX_SCENE_RULE_EVIDENCE, NODE_HINT_AFTER_STAGNANT_ROUNDS,
    SceneNodeKey, SceneProgress, SceneProgressUpdate, SceneRuleEvidence, StoredCycleMark,
    is_canonical_scene_evidence_json,
};

const MAX_CYCLE_MARK_BYTES: usize = 32 * 1024;
const MAX_CYCLE_SOURCE_BYTES: usize = 32 * 1024;

mod outbox;

impl Repository {
    /// 仅登记当前有效来源锚定的刻痕；同一身份重复提交只补来源，不改写摘要。
    ///
    /// 调用方必须先按可信剧本规则核验 `rule_id`、对象、内容权限和优先级。
    /// 本仓储只确认数据形状、当前周目边界及来源仍在有效因果路径上。
    ///
    /// # Errors
    /// 未知规则授权不由本层判断；来源过期、跨周目、容量耗尽或持久化不一致时拒绝。
    pub fn record_cycle_mark(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        mark: &CycleMarkV1,
    ) -> Result<StoredCycleMark> {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let stored = record_cycle_mark_in_transaction(self, &tx, port, mark)?;
        tx.commit().map_err(sql)?;
        Ok(stored)
    }

    /// 读取一部剧本有界的刻痕清单；已归档项只读可见，不删除其来源。
    /// # Errors
    /// 非法剧本 ID、超限 / 损坏持久化数据或数据库错误显式返回。
    pub fn cycle_marks(
        &self,
        conn: &Connection,
        script_id: &str,
        include_archived: bool,
    ) -> Result<Vec<StoredCycleMark>> {
        if !valid_script(script_id) {
            return Err(reject());
        }
        let status_filter = if include_archived {
            "status IN ('active','archived','unavailable')"
        } else {
            "status='active'"
        };
        let query = format!(
            "SELECT mark_id FROM memory_cycle_marks WHERE script_id=?1 AND {status_filter} ORDER BY priority DESC,cycle_gen DESC,mark_id ASC LIMIT {}",
            ABSOLUTE_CYCLE_CAPACITY + 1
        );
        let mut statement = conn.prepare(&query).map_err(sql)?;
        let mut rows = statement.query([script_id]).map_err(sql)?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next().map_err(sql)? {
            if ids.len() >= ABSOLUTE_CYCLE_CAPACITY {
                return Err(Error::Corrupt);
            }
            ids.push(read_string(row).map_err(sql)?);
        }
        ids.into_iter()
            .map(|id| read_cycle_mark(conn, &id))
            .collect()
    }

    /// 读取当前因果修订对应的节点状态；回退后的旧状态不会被当作当前状态返回。
    /// # Errors
    /// 周目不存在、不可写或状态修订与当前历史不一致时显式失败。
    pub fn scene_progress(
        &self,
        conn: &Connection,
        port: &dyn CausalPort,
        run_id: &str,
        node: SceneNodeKey<'_>,
    ) -> Result<Option<SceneProgress>> {
        if !valid_name(node.scene_id)
            || node.history_revision > MAX_INTEGER
            || node.entry_record_seq > MAX_INTEGER
        {
            return Err(reject());
        }
        let (run, boundary) = self.boundary(conn, run_id, port)?;
        self.ensure_writable(conn, &run.run_id)?;
        if boundary.history_revision != node.history_revision {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        let progress = read_scene_progress(conn, run_id, node)?;
        if progress
            .as_ref()
            .is_some_and(|state| state.entry_history_revision != boundary.history_revision)
        {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        Ok(progress)
    }

    /// 场景进入时冻结已确认的规则依据。
    ///
    /// 调用方须传入当前登记规则视图在 `history_revision` 上产生的依据；这里仅将其记为基线，不计作进展。
    /// 重开同一场景节点时保持幂等，并拒绝不同的基线。
    /// # Errors
    /// 边界过期、依据无效、周目不可写或重放内容冲突时均拒绝处理。
    pub fn begin_scene_progress(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        input: BeginSceneProgress<'_>,
    ) -> Result<SceneProgress> {
        let BeginSceneProgress {
            run_id,
            node,
            catalog_revision,
            baseline,
        } = input;
        let SceneNodeKey {
            scene_id,
            history_revision,
            entry_record_seq,
        } = node;
        validate_scene_progress_input(scene_id, catalog_revision, history_revision, baseline)?;
        let (run, boundary) = self.boundary(conn, run_id, port)?;
        self.ensure_writable(conn, &run.run_id)?;
        if boundary.history_revision != history_revision
            || entry_record_seq > MAX_INTEGER
            || !port.valid_scene_entry(scene_id, entry_record_seq)?
        {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let existing = read_scene_progress(&tx, run_id, node)?;
        if let Some(progress) = existing {
            if progress.entry_history_revision != history_revision
                || progress.catalog_revision != catalog_revision
                || read_scene_evidence(&tx, &progress, Some(true))? != canonical_evidence(baseline)
            {
                return Err(Error::Rejected(Reason::InvalidSource));
            }
            tx.commit().map_err(sql)?;
            return Ok(progress);
        }
        tx.execute(
            "INSERT INTO memory_cycle_scene_progress(run_id,scene_id,catalog_revision,entry_history_revision,entry_record_seq,revision,progress_count,stagnation_rounds,completed_rounds,last_completed_record_seq,last_round_id,last_evidence_hash) VALUES(?1,?2,?3,?4,?5,0,0,0,0,?5,NULL,NULL)",
            params![run_id, scene_id, catalog_revision, history_revision, entry_record_seq],
        )
        .map_err(sql)?;
        for evidence in baseline {
            insert_scene_evidence(&tx, run_id, node, history_revision, evidence, true)?;
        }
        let progress = read_scene_progress(&tx, run_id, node)?.ok_or(Error::Corrupt)?;
        tx.commit().map_err(sql)?;
        Ok(progress)
    }

    /// 应用一个已确认完成的回合；只有基线和此前回合均未记录的依据才算新进展。
    /// 重放最近一次完成回执时不重复计数。
    /// # Errors
    /// 历史边界过期、场景节点未知、旧回合重复提交或依据格式错误时拒绝处理。
    pub fn complete_scene_progress(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        input: CompleteSceneProgress<'_>,
    ) -> Result<SceneProgressUpdate> {
        let CompleteSceneProgress {
            run_id,
            node,
            catalog_revision,
            round_id,
            completed_record_seq,
            evidence,
        } = input;
        let SceneNodeKey {
            scene_id,
            history_revision,
            entry_record_seq,
        } = node;
        validate_scene_progress_input(scene_id, catalog_revision, history_revision, evidence)?;
        if !valid_id(round_id) || entry_record_seq > MAX_INTEGER {
            return Err(reject());
        }
        let evidence_hash = hash(&canonical(&canonical_evidence(evidence))?);
        let (run, boundary) = self.boundary(conn, run_id, port)?;
        self.ensure_writable(conn, &run.run_id)?;
        if boundary.history_revision != history_revision
            || completed_record_seq > MAX_INTEGER
            || !port.valid_completed_round(
                round_id,
                scene_id,
                entry_record_seq,
                completed_record_seq,
            )?
        {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let mut progress = read_scene_progress(&tx, run_id, node)?.ok_or(Error::NotFound)?;
        if progress.catalog_revision != catalog_revision
            || progress.entry_history_revision != history_revision
            || completed_record_seq <= progress.entry_record_seq
        {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        if progress.last_round_id.as_deref() == Some(round_id)
            && progress.last_completed_record_seq == completed_record_seq
        {
            if progress.last_evidence_hash != Some(evidence_hash) {
                return Err(Error::Rejected(Reason::InvalidSource));
            }
            let newly_observed_rules = Vec::new();
            tx.commit().map_err(sql)?;
            return Ok(SceneProgressUpdate {
                state: progress,
                made_progress: false,
                newly_observed_rules,
            });
        }
        if completed_record_seq <= progress.last_completed_record_seq {
            return Err(Error::Rejected(Reason::InvalidSource));
        }

        let before_count = scene_evidence_count(&tx, &progress)?;
        let existing_evidence = read_scene_evidence(&tx, &progress, None)?
            .into_iter()
            .map(|(rule_id, evidence_hash, _)| (rule_id, evidence_hash))
            .collect::<BTreeSet<_>>();
        let new_evidence_count = evidence
            .iter()
            .filter(|item| !existing_evidence.contains(&(item.rule_id.clone(), item.evidence_hash)))
            .count();
        if before_count.saturating_add(new_evidence_count) > MAX_SCENE_PROGRESS_EVIDENCE {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        let mut newly_observed_rules = Vec::new();
        for item in evidence {
            if insert_scene_evidence(&tx, run_id, node, history_revision, item, false)? {
                newly_observed_rules.push(item.rule_id.clone());
            }
        }
        newly_observed_rules.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        newly_observed_rules.dedup();
        let made_progress = !newly_observed_rules.is_empty();
        progress.revision = progress.revision.checked_add(1).ok_or(Error::Corrupt)?;
        progress.progress_count = progress
            .progress_count
            .checked_add(if made_progress { 1 } else { 0 })
            .ok_or(Error::Corrupt)?;
        progress.stagnation_rounds = if made_progress {
            0
        } else {
            progress
                .stagnation_rounds
                .checked_add(1)
                .ok_or(Error::Corrupt)?
        };
        progress.completed_rounds = progress
            .completed_rounds
            .checked_add(1)
            .ok_or(Error::Corrupt)?;
        progress.last_completed_record_seq = completed_record_seq;
        progress.last_round_id = Some(round_id.into());
        progress.last_evidence_hash = Some(evidence_hash);
        write_scene_progress(&tx, &progress, progress.revision - 1)?;
        tx.commit().map_err(sql)?;
        Ok(SceneProgressUpdate {
            state: progress,
            made_progress,
            newly_observed_rules,
        })
    }

    /// 原子领取一条已求值的登记提示，供界面展示。
    /// 规则匹配和固定文案查询仍由调用方负责；领取提示不会产生世界事实。
    /// # Errors
    /// 进度修订过期时拒绝处理；当前不满足条件或已展示过的提示返回 `None`。
    pub fn claim_scene_hint(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        input: ClaimSceneHint<'_>,
    ) -> Result<Option<SceneProgress>> {
        let ClaimSceneHint {
            run_id,
            node,
            expected_revision,
            hint_id,
        } = input;
        if !valid_name(node.scene_id)
            || !valid_name(hint_id)
            || node.history_revision > MAX_INTEGER
            || node.entry_record_seq > MAX_INTEGER
        {
            return Err(reject());
        }
        let (run, boundary) = self.boundary(conn, run_id, port)?;
        self.ensure_writable(conn, &run.run_id)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let mut progress = read_scene_progress(&tx, run_id, node)?.ok_or(Error::NotFound)?;
        if boundary.history_revision != progress.entry_history_revision
            || progress.revision != expected_revision
        {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        if progress.stagnation_rounds < NODE_HINT_AFTER_STAGNANT_ROUNDS
            || progress.hints_shown >= crate::model::MAX_CYCLE_HINTS_PER_SCENE
        {
            tx.commit().map_err(sql)?;
            return Ok(None);
        }
        let already_shown: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_cycle_scene_hints WHERE run_id=?1 AND scene_id=?2 AND entry_history_revision=?3 AND entry_record_seq=?4 AND hint_id=?5)",
                params![
                    run_id,
                    node.scene_id,
                    node.history_revision,
                    node.entry_record_seq,
                    hint_id
                ],
                |row| row.get(0),
            )
            .map_err(sql)?;
        if already_shown
            || progress.last_hint_round.is_some_and(|last| {
                progress.completed_rounds.saturating_sub(last) < CYCLE_HINT_COOLDOWN_ROUNDS
            })
        {
            tx.commit().map_err(sql)?;
            return Ok(None);
        }
        let expected = progress.revision;
        progress.revision = progress.revision.checked_add(1).ok_or(Error::Corrupt)?;
        progress.hints_shown += 1;
        progress.last_hint_round = Some(progress.completed_rounds);
        progress.last_hint_id = Some(hint_id.into());
        write_scene_progress(&tx, &progress, expected)?;
        tx.execute(
            "INSERT INTO memory_cycle_scene_hints(run_id,scene_id,entry_history_revision,entry_record_seq,hint_id,shown_at_round) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                run_id,
                node.scene_id,
                node.history_revision,
                node.entry_record_seq,
                hint_id,
                progress.completed_rounds
            ],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(Some(progress))
    }
}

fn validate_scene_progress_input(
    scene_id: &str,
    catalog_revision: &str,
    history_revision: u64,
    evidence: &[SceneRuleEvidence],
) -> Result<()> {
    if !valid_name(scene_id)
        || catalog_revision.is_empty()
        || catalog_revision.len() > 256
        || history_revision > MAX_INTEGER
        || evidence.len() > MAX_SCENE_RULE_EVIDENCE
    {
        return Err(reject());
    }
    let unique = evidence
        .iter()
        .map(|item| (item.rule_id.as_str(), item.evidence_hash))
        .collect::<BTreeSet<_>>();
    if unique.len() != evidence.len() {
        return Err(reject());
    }
    for item in evidence {
        item.validate()?;
    }
    Ok(())
}

fn canonical_evidence(
    evidence: &[SceneRuleEvidence],
) -> Vec<(String, crate::model::Hash, Vec<u8>)> {
    let mut canonical = evidence
        .iter()
        .map(|item| {
            (
                item.rule_id.clone(),
                item.evidence_hash,
                item.evidence_json.clone(),
            )
        })
        .collect::<Vec<_>>();
    canonical.sort_by(|left, right| {
        left.0
            .as_bytes()
            .cmp(right.0.as_bytes())
            .then_with(|| left.1.cmp(&right.1))
    });
    canonical
}

fn read_scene_progress(
    conn: &Connection,
    run_id: &str,
    node: SceneNodeKey<'_>,
) -> Result<Option<SceneProgress>> {
    conn.query_row(
        "SELECT run_id,scene_id,catalog_revision,entry_history_revision,entry_record_seq,revision,progress_count,stagnation_rounds,completed_rounds,last_completed_record_seq,last_round_id,last_evidence_hash,hints_shown,last_hint_round,last_hint_id FROM memory_cycle_scene_progress WHERE run_id=?1 AND scene_id=?2 AND entry_history_revision=?3 AND entry_record_seq=?4",
        params![
            run_id,
            node.scene_id,
            node.history_revision,
            node.entry_record_seq
        ],
        read_scene_progress_row,
    )
    .optional()
    .map_err(sql)
}

fn read_scene_progress_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SceneProgress> {
    Ok(SceneProgress {
        run_id: row.get(0)?,
        scene_id: row.get(1)?,
        catalog_revision: row.get(2)?,
        entry_history_revision: row.get(3)?,
        entry_record_seq: row.get(4)?,
        revision: row.get(5)?,
        progress_count: row.get(6)?,
        stagnation_rounds: row.get(7)?,
        completed_rounds: row.get(8)?,
        last_completed_record_seq: row.get(9)?,
        last_round_id: row.get(10)?,
        last_evidence_hash: row
            .get::<_, Option<Vec<u8>>>(11)?
            .map(|hash| hash.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        hints_shown: row.get(12)?,
        last_hint_round: row.get(13)?,
        last_hint_id: row.get(14)?,
    })
}

fn read_scene_evidence(
    conn: &Connection,
    progress: &SceneProgress,
    baseline: Option<bool>,
) -> Result<Vec<(String, crate::model::Hash, Vec<u8>)>> {
    let mut statement = conn
        .prepare("SELECT rule_id,evidence_hash,length(evidence_json),evidence_json FROM memory_cycle_scene_evidence WHERE run_id=?1 AND scene_id=?2 AND entry_history_revision=?3 AND entry_record_seq=?4 AND (?5 IS NULL OR baseline=?5) ORDER BY rule_id,evidence_hash")
        .map_err(sql)?;
    let rows = statement
        .query_map(
            params![
                progress.run_id,
                progress.scene_id,
                progress.entry_history_revision,
                progress.entry_record_seq,
                baseline
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    read_bounded_blob(row, 2, 3, MAX_SCENE_EVIDENCE_BYTES)?,
                ))
            },
        )
        .map_err(sql)?;
    let mut evidence = Vec::new();
    for row in rows {
        let (rule_id, hash, raw) = row.map_err(sql)?;
        let hash = hash.try_into().map_err(|_| Error::Corrupt)?;
        let raw = raw.into_result().map_err(|_| Error::Corrupt)?;
        if crate::model::hash(&raw) != hash || !is_canonical_scene_evidence_json(&raw) {
            return Err(Error::Corrupt);
        }
        evidence.push((rule_id, hash, raw));
    }
    Ok(evidence)
}

fn scene_evidence_count(conn: &Connection, progress: &SceneProgress) -> Result<usize> {
    conn.query_row(
        "SELECT count(*) FROM memory_cycle_scene_evidence WHERE run_id=?1 AND scene_id=?2 AND entry_history_revision=?3 AND entry_record_seq=?4",
        params![progress.run_id, progress.scene_id, progress.entry_history_revision, progress.entry_record_seq],
        |row| row.get(0),
    )
    .map_err(sql)
}

fn insert_scene_evidence(
    conn: &Connection,
    run_id: &str,
    node: SceneNodeKey<'_>,
    first_seen_history_revision: u64,
    evidence: &SceneRuleEvidence,
    baseline: bool,
) -> Result<bool> {
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO memory_cycle_scene_evidence(run_id,scene_id,entry_history_revision,entry_record_seq,rule_id,evidence_hash,evidence_json,first_seen_history_revision,baseline) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                run_id,
                node.scene_id,
                node.history_revision,
                node.entry_record_seq,
                evidence.rule_id,
                evidence.evidence_hash.as_slice(),
                evidence.evidence_json,
                first_seen_history_revision,
                baseline
            ],
        )
        .map_err(sql)?;
    Ok(inserted == 1)
}

fn write_scene_progress(
    conn: &Connection,
    progress: &SceneProgress,
    expected_revision: u64,
) -> Result<()> {
    let updated = conn
        .execute(
            "UPDATE memory_cycle_scene_progress SET revision=?1,progress_count=?2,stagnation_rounds=?3,completed_rounds=?4,last_completed_record_seq=?5,last_round_id=?6,last_evidence_hash=?7,hints_shown=?8,last_hint_round=?9,last_hint_id=?10 WHERE run_id=?11 AND scene_id=?12 AND entry_history_revision=?13 AND entry_record_seq=?14 AND revision=?15",
            params![
                progress.revision,
                progress.progress_count,
                progress.stagnation_rounds,
                progress.completed_rounds,
                progress.last_completed_record_seq,
                progress.last_round_id,
                progress.last_evidence_hash.map(|hash| hash.to_vec()),
                progress.hints_shown,
                progress.last_hint_round,
                progress.last_hint_id,
                progress.run_id,
                progress.scene_id,
                progress.entry_history_revision,
                progress.entry_record_seq,
                expected_revision,
            ],
        )
        .map_err(sql)?;
    if updated != 1 {
        return Err(Error::Rejected(Reason::InvalidSource));
    }
    Ok(())
}

/// 在上层已有写事务中登记刻痕，供批次回执和刻痕保持原子提交。
pub(super) fn record_cycle_mark_in_transaction(
    repository: &Repository,
    tx: &rusqlite::Transaction<'_>,
    port: &dyn CausalPort,
    mark: &CycleMarkV1,
) -> Result<StoredCycleMark> {
    mark.validate()?;
    let (run, boundary) = repository.boundary(tx, &mark.source_refs[0].source.run_id, port)?;
    repository.ensure_writable(tx, &run.run_id)?;
    if run.script_id != mark.script_id
        || run.cycle_gen != mark.cycle_gen
        || mark.source_refs.iter().any(|source| {
            source.source.run_id != run.run_id
                || source.history_revision > boundary.history_revision
        })
    {
        return Err(Error::Rejected(Reason::InvalidSource));
    }
    for source in &mark.source_refs {
        if !repository.valid_sources(&run, std::slice::from_ref(&source.source), port)? {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
    }

    let mut canonical_mark = mark.clone();
    canonical_mark
        .subject_ids
        .sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    canonical_mark.source_refs.sort_by(|left, right| {
        left.source
            .run_id
            .as_bytes()
            .cmp(right.source.run_id.as_bytes())
            .then_with(|| {
                left.source
                    .session_id
                    .as_bytes()
                    .cmp(right.source.session_id.as_bytes())
            })
            .then_with(|| left.source.record_seq.cmp(&right.source.record_seq))
            .then_with(|| left.source.start_byte.cmp(&right.source.start_byte))
            .then_with(|| left.source.end_byte.cmp(&right.source.end_byte))
            .then_with(|| left.source.body_hash.cmp(&right.source.body_hash))
    });
    canonical_mark.validate()?;
    let identity = canonical_mark.identity_hash()?;

    let existing = tx
        .query_row(
            "SELECT mark_id FROM memory_cycle_marks WHERE script_id=?1 AND cycle_gen=?2 AND identity_hash=?3",
            params![canonical_mark.script_id, canonical_mark.cycle_gen, identity.as_slice()],
            read_string,
        )
        .optional()
        .map_err(sql)?;

    let mark_id = if let Some(mark_id) = existing {
        let stored = read_cycle_mark(tx, &mark_id)?;
        if !same_cycle_definition(&stored.mark, &canonical_mark) {
            return Err(Error::Corrupt);
        }
        for source in &stored.mark.source_refs {
            if source.history_revision > boundary.history_revision
                || !repository.valid_sources(&run, std::slice::from_ref(&source.source), port)?
            {
                return Err(Error::Rejected(Reason::InvalidSource));
            }
        }
        mark_id
    } else {
        let count: usize = tx
            .query_row(
                "SELECT count(*) FROM memory_cycle_marks WHERE script_id=?1",
                [&canonical_mark.script_id],
                |row| row.get(0),
            )
            .map_err(sql)?;
        if count >= ABSOLUTE_CYCLE_CAPACITY {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        let payload = canonical(&canonical_mark)?;
        if payload.len() > MAX_CYCLE_MARK_BYTES {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        let payload_hash = hash(&payload);
        tx.execute(
            "INSERT INTO memory_cycle_marks(mark_id,script_id,cycle_gen,identity_hash,rule_id,rule_version,subject_ids_json,imprint_key,kind,detail_policy,priority,summary,payload_hash,payload_json,status) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,'active')",
            params![
                canonical_mark.mark_id,
                canonical_mark.script_id,
                canonical_mark.cycle_gen,
                identity.as_slice(),
                canonical_mark.rule_id,
                canonical_mark.rule_version,
                canonical(&canonical_mark.subject_ids)?,
                canonical_mark.imprint_key,
                enum_text(canonical_mark.kind)?,
                enum_text(canonical_mark.detail_policy)?,
                canonical_mark.priority,
                canonical_mark.summary,
                payload_hash.as_slice(),
                payload,
            ],
        )
        .map_err(sql)?;
        canonical_mark.mark_id.clone()
    };

    for source in &canonical_mark.source_refs {
        insert_cycle_source(tx, &mark_id, source)?;
    }
    let source_count: usize = tx
        .query_row(
            "SELECT count(*) FROM memory_cycle_sources WHERE mark_id=?1",
            [&mark_id],
            |row| row.get(0),
        )
        .map_err(sql)?;
    if source_count > MAX_CYCLE_SOURCES {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    tx.execute(
        "UPDATE memory_cycle_marks SET status='active' WHERE mark_id=?1 AND status='archived'",
        [&mark_id],
    )
    .map_err(sql)?;
    enforce_active_capacity(tx, &canonical_mark.script_id)?;
    read_cycle_mark(tx, &mark_id)
}

fn same_cycle_definition(left: &CycleMarkV1, right: &CycleMarkV1) -> bool {
    left.mark_id == right.mark_id
        && left.script_id == right.script_id
        && left.cycle_gen == right.cycle_gen
        && left.rule_id == right.rule_id
        && left.rule_version == right.rule_version
        && left.subject_ids == right.subject_ids
        && left.imprint_key == right.imprint_key
        && left.kind == right.kind
        && left.detail_policy == right.detail_policy
        && left.priority == right.priority
}

fn insert_cycle_source(conn: &Connection, mark_id: &str, source: &CycleSourceRef) -> Result<()> {
    let raw = canonical(source)?;
    if raw.len() > MAX_CYCLE_SOURCE_BYTES {
        return Err(Error::Rejected(Reason::CapacityBlocked));
    }
    let digest = hash(&canonical(&source.source)?);
    let existing = conn
        .query_row(
            "SELECT length(source_json),source_json,history_revision FROM memory_cycle_sources WHERE mark_id=?1 AND source_hash=?2",
            params![mark_id,digest.as_slice()],
            |row| {
                Ok((
                    read_bounded_blob(row, 0, 1, MAX_CYCLE_SOURCE_BYTES)?,
                    row.get::<_, u64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sql)?;
    if let Some((existing, revision)) = existing {
        let existing = existing.into_result()?;
        let stored: CycleSourceRef = decode(&existing, MAX_CYCLE_SOURCE_BYTES)?;
        if revision != stored.history_revision {
            return Err(Error::Corrupt);
        }
        if stored != *source {
            return Err(Error::Rejected(Reason::InvalidSource));
        }
        return Ok(());
    }
    conn.execute(
        "INSERT INTO memory_cycle_sources(mark_id,source_hash,source_json,history_revision) VALUES(?1,?2,?3,?4)",
        params![mark_id,digest.as_slice(),raw,source.history_revision],
    )
    .map_err(sql)?;
    Ok(())
}

fn enforce_active_capacity(conn: &Connection, script_id: &str) -> Result<()> {
    let mut statement = conn
        .prepare(
            "SELECT mark_id FROM memory_cycle_marks WHERE script_id=?1 AND status='active' ORDER BY priority DESC,cycle_gen DESC,mark_id ASC",
        )
        .map_err(sql)?;
    let ids = statement
        .query_map([script_id], read_string)
        .map_err(sql)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(sql)?;
    drop(statement);
    // 归档排序依赖冗余 SQL 字段；先核对每条载荷，避免损坏优先级影响淘汰决定。
    for id in &ids {
        read_cycle_mark(conn, id)?;
    }
    for id in ids.into_iter().skip(ACTIVE_CYCLE_CAPACITY) {
        conn.execute(
            "UPDATE memory_cycle_marks SET status='archived' WHERE mark_id=?1 AND status='active'",
            [id],
        )
        .map_err(sql)?;
    }
    Ok(())
}

pub(super) fn read_cycle_mark(conn: &Connection, mark_id: &str) -> Result<StoredCycleMark> {
    let (
        payload,
        payload_hash,
        status,
        script_id,
        cycle_gen,
        identity_hash,
        rule_id,
        rule_version,
        subject_ids,
        imprint_key,
        kind,
        detail_policy,
        priority,
        summary,
    ) = conn
        .query_row(
            "SELECT length(payload_json),payload_json,length(payload_hash),payload_hash,status,script_id,cycle_gen,length(identity_hash),identity_hash,rule_id,rule_version,length(subject_ids_json),subject_ids_json,imprint_key,kind,detail_policy,priority,summary FROM memory_cycle_marks WHERE mark_id=?1",
            [mark_id],
            |row| {
                Ok((
                    read_bounded_blob(row, 0, 1, MAX_CYCLE_MARK_BYTES)?,
                    read_bounded_blob(row, 2, 3, 32)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    read_bounded_blob(row, 7, 8, 32)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                    read_bounded_blob(row, 11, 12, 2_048)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, String>(14)?,
                    row.get::<_, String>(15)?,
                    row.get::<_, i64>(16)?,
                    row.get::<_, String>(17)?,
                ))
            },
        )
        .optional()
        .map_err(sql)?
        .ok_or(Error::NotFound)?;
    let payload = payload.into_result()?;
    let payload_hash = payload_hash.into_result()?;
    let identity_hash = identity_hash.into_result()?;
    let subject_ids = subject_ids.into_result()?;
    if hash(&payload) != payload_hash.as_slice() {
        return Err(Error::Corrupt);
    }
    let mut mark: CycleMarkV1 = decode_versioned(&payload, MAX_CYCLE_MARK_BYTES)?;
    let expected_identity = mark.identity_hash()?;
    if mark.mark_id != mark_id
        || mark.script_id != script_id
        || i64::try_from(mark.cycle_gen).ok() != Some(cycle_gen)
        || expected_identity.as_slice() != identity_hash.as_slice()
        || mark.rule_id != rule_id
        || i64::from(mark.rule_version) != rule_version
        || canonical(&mark.subject_ids)? != subject_ids
        || mark.imprint_key != imprint_key
        || enum_text(mark.kind)? != kind
        || enum_text(mark.detail_policy)? != detail_policy
        || i64::from(mark.priority) != priority
        || mark.summary != summary
    {
        return Err(Error::Corrupt);
    }

    let original_sources = mark.source_refs.clone();
    let mut statement = conn
        .prepare("SELECT length(source_hash),source_hash,length(source_json),source_json,history_revision FROM memory_cycle_sources WHERE mark_id=?1 ORDER BY source_hash ASC")
        .map_err(sql)?;
    let mut rows = statement.query([mark_id]).map_err(sql)?;
    let mut sources = Vec::new();
    while let Some(row) = rows.next().map_err(sql)? {
        if sources.len() >= MAX_CYCLE_SOURCES {
            return Err(Error::Corrupt);
        }
        let stored_hash = read_bounded_blob(row, 0, 1, 32).map_err(sql)?;
        let raw = read_bounded_blob(row, 2, 3, MAX_CYCLE_SOURCE_BYTES).map_err(sql)?;
        let revision: u64 = row.get(4).map_err(sql)?;
        let stored_hash = stored_hash.into_result()?;
        let raw = raw.into_result()?;
        let source: CycleSourceRef = decode(&raw, MAX_CYCLE_SOURCE_BYTES)?;
        source.source.validate().map_err(stored_invalid)?;
        let digest = hash(&canonical(&source.source)?);
        if digest.as_slice() != stored_hash.as_slice() || source.history_revision != revision {
            return Err(Error::Corrupt);
        }
        sources.push(source);
    }
    drop(rows);
    drop(statement);
    if sources.is_empty() {
        return Err(Error::Corrupt);
    }
    if original_sources
        .iter()
        .any(|original| !sources.contains(original))
    {
        return Err(Error::Corrupt);
    }
    mark.source_refs = sources;
    mark.validate().map_err(stored_invalid)?;
    Ok(StoredCycleMark {
        mark,
        status: decode_enum(&status)?,
    })
}

#[cfg(test)]
mod tests;
