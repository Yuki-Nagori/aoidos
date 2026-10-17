//! 轮回生命周期事件的有界待处理队列。

use super::*;
use crate::model::{CycleLifecycleEvent, MAX_CYCLE_EVENT_BYTES, MAX_CYCLE_EVENT_PAGE};

impl Repository {
    /// 保存已规范化的生命周期事件；相同身份和内容的重试不重复入队。
    ///
    /// 来源记录有效性由引擎在消费时通过 SourcePort 核验，本方法只负责持久化幂等。
    /// # Errors
    /// 同一来源身份携带不同内容、载荷超限、运行不存在或数据库损坏时显式失败。
    pub fn prepare_cycle_event(
        &self,
        conn: &mut Connection,
        event: CycleLifecycleEvent,
    ) -> Result<CycleLifecycleEvent> {
        let event = event.normalize()?;
        let payload = event.encode()?;
        let payload_hash = hash(&payload);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        self.ensure_writable(&tx, &event.run_id)?;
        let existing = tx
            .query_row(
                "SELECT event_id,run_id,history_revision,kind,source_record_seq,length(payload_json),payload_json,length(payload_hash),payload_hash FROM memory_cycle_events WHERE event_id=?1 OR (run_id=?2 AND history_revision=?3 AND kind=?4 AND source_record_seq=?5)",
                params![
                    event.event_id,
                    event.run_id,
                    event.history_revision,
                    enum_text(event.kind)?,
                    event.source_record_seq
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, u64>(4)?,
                        read_bounded_blob(row, 5, 6, MAX_CYCLE_EVENT_BYTES)?,
                        read_bounded_blob(row, 7, 8, 32)?,
                    ))
                },
            )
            .optional()
            .map_err(sql)?;
        if let Some((
            event_id,
            run_id,
            history_revision,
            kind,
            source_seq,
            existing_payload,
            existing_hash,
        )) = existing
        {
            let existing_payload = existing_payload.into_result().map_err(|_| Error::Corrupt)?;
            let existing_hash = existing_hash.into_result().map_err(|_| Error::Corrupt)?;
            if event_id != event.event_id {
                return Err(Error::Rejected(Reason::InvalidSource));
            }
            let stored: CycleLifecycleEvent =
                decode_versioned(&existing_payload, MAX_CYCLE_EVENT_BYTES)?;
            if stored.run_id != run_id
                || stored.history_revision != history_revision
                || enum_text(stored.kind)? != kind
                || stored.source_record_seq != source_seq
            {
                return Err(Error::Corrupt);
            }
            if existing_payload != payload || existing_hash.as_slice() != payload_hash {
                return Err(Error::Rejected(Reason::InvalidSource));
            }
            tx.commit().map_err(sql)?;
            return Ok(stored);
        }
        let pending_count: usize = tx
            .query_row(
                "SELECT count(*) FROM memory_cycle_events WHERE run_id=?1",
                [&event.run_id],
                |row| row.get(0),
            )
            .map_err(sql)?;
        if pending_count >= MAX_CYCLE_EVENT_PAGE {
            return Err(Error::Rejected(Reason::CapacityBlocked));
        }
        tx.execute(
            "INSERT INTO memory_cycle_events(event_id,run_id,history_revision,kind,source_record_seq,payload_hash,payload_json) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                event.event_id,
                event.run_id,
                event.history_revision,
                enum_text(event.kind)?,
                event.source_record_seq,
                payload_hash.as_slice(),
                payload
            ],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(event)
    }

    /// 按来源记录序号稳定读取待处理事件；调用方必须显式给出有界页大小。
    /// # Errors
    /// 无效 run ID、超出页上限或队列载荷损坏时失败。
    pub fn pending_cycle_events(
        &self,
        conn: &Connection,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<CycleLifecycleEvent>> {
        if !valid_id(run_id) || limit > MAX_CYCLE_EVENT_PAGE {
            return Err(reject());
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = conn
            .prepare("SELECT event_id,history_revision,kind,source_record_seq,length(payload_hash),payload_hash,length(payload_json),payload_json FROM memory_cycle_events WHERE run_id=?1 ORDER BY source_record_seq ASC,event_id ASC LIMIT ?2")
            .map_err(sql)?;
        let rows = statement
            .query_map(params![run_id, limit as u64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                    read_bounded_blob(row, 4, 5, 32)?,
                    read_bounded_blob(row, 6, 7, MAX_CYCLE_EVENT_BYTES)?,
                ))
            })
            .map_err(sql)?;
        let mut events = Vec::new();
        for row in rows {
            let (event_id, history_revision, kind, source_seq, stored_hash, stored_payload) =
                row.map_err(sql)?;
            let stored_hash = stored_hash.into_result().map_err(|_| Error::Corrupt)?;
            let payload = stored_payload.into_result().map_err(|_| Error::Corrupt)?;
            if payload.len() > MAX_CYCLE_EVENT_BYTES || hash(&payload).as_slice() != stored_hash {
                return Err(Error::Corrupt);
            }
            let event: CycleLifecycleEvent = decode_versioned(&payload, MAX_CYCLE_EVENT_BYTES)?;
            if event.event_id != event_id
                || event.history_revision != history_revision
                || enum_text(event.kind)? != kind
                || event.source_record_seq != source_seq
                || event.run_id != run_id
            {
                return Err(Error::Corrupt);
            }
            events.push(event.normalize()?);
        }
        Ok(events)
    }

    /// 删除已应用或经调用方确认失效的事件；重复删除成功且不改变结果。
    /// # Errors
    /// 无效身份或数据库错误显式返回。
    pub fn delete_cycle_event(
        &self,
        conn: &mut Connection,
        run_id: &str,
        event_id: &str,
    ) -> Result<()> {
        if !valid_id(run_id)
            || event_id.len() != 64
            || !event_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(reject());
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        self.ensure_writable(&tx, run_id)?;
        tx.execute(
            "DELETE FROM memory_cycle_events WHERE run_id=?1 AND event_id=?2",
            params![run_id, event_id],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(())
    }
}
