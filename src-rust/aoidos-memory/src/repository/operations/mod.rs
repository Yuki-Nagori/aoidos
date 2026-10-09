//! 计划登记、不可变正文准备和同事务 CAS；回执丢失只核验原计划，不另造身份。

use super::*;
impl Repository {
    /// # Errors
    /// 身份冲突为损坏；来源、策略或任一目标版本过期时拒绝整个依赖组。
    pub fn prepare(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        plan: &Plan,
    ) -> Result<OperationState> {
        self.preflight_plan(plan)?;
        self.ensure_writable(conn, &plan.run_id)?;
        let bytes = canonical(plan)?;
        self.validate_plan(plan, &bytes)?;
        if let Some((previous, state)) = self.operation_row(conn, &plan.operation_id)? {
            if previous.into_result()? != bytes {
                return Err(Error::Corrupt);
            }
            let state = decode_enum(&state)?;
            if state == OperationState::Applied {
                self.verify_applied(conn, plan)?;
            }
            return Ok(state);
        }
        self.check_plan(conn, port, plan)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        tx.execute("INSERT INTO memory_operations(operation_id,run_id,plan_hash,input_history_revision,state,plan_json) VALUES(?1,?2,?3,?4,'prepared',?5)",params![plan.operation_id,plan.run_id,hash(&bytes).as_slice(),plan.history_revision,bytes]).map_err(sql)?;
        for target in &plan.targets {
            tx.execute("INSERT OR IGNORE INTO memory_entries(entry_id,run_id,current_version_id,state_revision,status,effect_history_revision) VALUES(?1,?2,NULL,0,'unavailable',?3)",params![target.entry_id,plan.run_id,plan.history_revision]).map_err(sql)?;
            tx.execute(
                "INSERT INTO memory_operation_targets VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    plan.operation_id,
                    target.entry_id,
                    target.expected.version_id,
                    target.expected.state_revision,
                    target.next.as_ref().map(|v| &v.version_id),
                    target.expected.state_revision + 1
                ],
            )
            .map_err(sql)?;
            if let Some(version) = &target.next {
                self.register_version(&tx, version)?;
            }
        }
        tx.commit().map_err(sql)?;
        Ok(OperationState::Prepared)
    }
    fn preflight_plan(&self, plan: &Plan) -> Result<()> {
        if plan.targets.len() > 16
            || plan.processed_materials.len() > 128
            || plan.reason.len() > 512
        {
            return Err(reject());
        }
        for target in &plan.targets {
            if target.effects.len() > 128 {
                return Err(reject());
            }
            if let Some(version) = &target.next {
                version.validate()?;
            }
            for effect in &target.effects {
                effect.source.validate()?;
            }
        }
        Ok(())
    }
    fn validate_plan(&self, plan: &Plan, bytes: &[u8]) -> Result<()> {
        if !valid_id(&plan.operation_id)
            || !valid_id(&plan.run_id)
            || plan.expected_logical_clock > MAX_INTEGER
            || plan.history_revision > MAX_INTEGER
            || plan.entity_revision > MAX_INTEGER
            || plan.evidence_revision > MAX_INTEGER
            || plan.targets.len() > 16
            || plan.processed_materials.len() > 128
            || bytes.len() > MAX_PLAN_BYTES
            || plan.reason.len() > 512
            || (plan.targets.is_empty() && plan.processed_materials.is_empty())
        {
            return Err(reject());
        }
        let mut entries = BTreeSet::new();
        let mut versions = BTreeSet::new();
        let mut effects = BTreeSet::new();
        for target in &plan.targets {
            if !valid_id(&target.entry_id)
                || !entries.insert(&target.entry_id)
                || target.expected.state_revision >= MAX_INTEGER
                || target
                    .expected
                    .version_id
                    .as_ref()
                    .is_some_and(|id| !valid_id(id))
                || target.effects.len() > 128
                || (target.expected.version_id.is_none()
                    && (target.expected.state_revision != 0 || target.next.is_none()))
            {
                return Err(reject());
            }
            if let Some(version) = &target.next {
                version.validate()?;
                if version.entry_id != target.entry_id
                    || !versions.insert(&version.version_id)
                    || version.parent_version_id != target.expected.version_id
                    || version.policy_hash != plan.policy_hash
                    || (version.change == ChangeKind::New) != target.expected.version_id.is_none()
                {
                    return Err(reject());
                }
            }
            let mut source_effects = BTreeSet::new();
            for effect in &target.effects {
                effect.source.validate()?;
                if !valid_id(&effect.effect_id)
                    || !effects.insert(&effect.effect_id)
                    || effect.logical_position > MAX_INTEGER
                    || effect.gain > 1_000_000
                    || (effect.kind == EffectKind::Revival && effect.gain != 0)
                    || !source_effects.insert((
                        effect.source.session_id.as_str(),
                        effect.source.record_seq,
                        enum_text(effect.kind)?,
                    ))
                {
                    return Err(reject());
                }
            }
        }
        if plan.processed_materials.iter().any(|id| !valid_id(id))
            || plan
                .processed_materials
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != plan.processed_materials.len()
        {
            return Err(reject());
        }
        Ok(())
    }
    pub(super) fn check_plan(
        &self,
        conn: &Connection,
        port: &dyn CausalPort,
        plan: &Plan,
    ) -> Result<()> {
        let (run, view) = self.boundary(conn, &plan.run_id, port)?;
        if self.clock(conn, port, &plan.run_id)? != plan.expected_logical_clock
            || run.policy_hash != plan.policy_hash
            || view.history_revision != plan.history_revision
            || view.entity_revision != plan.entity_revision
            || view.evidence_revision != plan.evidence_revision
        {
            return Err(Error::Rejected(Reason::StaleVersion));
        }
        for target in &plan.targets {
            let mut subject = target.next.as_ref().map(|version| version.subject.clone());
            let current=conn.query_row("SELECT run_id,current_version_id,state_revision FROM memory_entries WHERE entry_id=?1",[&target.entry_id],read_expected).optional().map_err(sql)?;
            match current {
                Some((run_id, version, revision))
                    if run_id == plan.run_id
                        && version == target.expected.version_id
                        && revision == target.expected.state_revision => {}
                None if target.expected.version_id.is_none()
                    && target.expected.state_revision == 0 => {}
                _ => return Err(Error::Rejected(Reason::StaleVersion)),
            }
            if let Some(current_id) = &target.expected.version_id {
                let confirmed: Option<u64> = conn
                    .query_row(
                        "SELECT effect_history_revision FROM memory_entries WHERE entry_id=?1",
                        [&target.entry_id],
                        read_effect_history,
                    )
                    .map_err(sql)?;
                if confirmed != Some(view.history_revision) {
                    return Err(Error::Rejected(Reason::RecoveryRequired));
                }
                let metadata = self.version_metadata(conn, &target.entry_id, current_id)?;
                if let Some(next) = &target.next {
                    if canonical(&next.subject)? != metadata.subject {
                        return Err(Error::Rejected(Reason::InvalidSchema));
                    }
                } else {
                    let current = self.read_version(conn, &run, &target.entry_id, current_id)?;
                    if !self.valid_version(&run, &current, port)? {
                        return Err(Error::Rejected(Reason::InvalidSource));
                    }
                    subject = Some(current.subject);
                }
            }
            if let Some(version) = &target.next {
                if !self.valid_version(&run, version, port)? {
                    return Err(Error::Rejected(Reason::InvalidSource));
                }
                if let Some(restored) = &version.restored_from_version_id {
                    let permitted:bool=conn.query_row("SELECT status='unavailable' AND recovery_cursor=?1 AND recovery_history_revision=?2 FROM memory_entries WHERE entry_id=?3 AND run_id=?4",params![restored,plan.history_revision,target.entry_id,plan.run_id],read_bool).map_err(sql)?;
                    if !permitted {
                        return Err(Error::Rejected(Reason::InvalidSource));
                    }
                    let healthy = self.read_version(conn, &run, &target.entry_id, restored)?;
                    if !self.valid_version(&run, &healthy, port)?
                        || !same_content(version, &healthy)
                    {
                        return Err(Error::Rejected(Reason::InvalidSource));
                    }
                }
            }
            if !target.effects.is_empty() {
                let subject = subject.ok_or(Error::Corrupt)?;
                if matches!(subject, Subject::Character { .. }) {
                    // v1 效果只记录通用来源；主体绑定的获知证据进入 schema 前拒绝角色效果。
                    return Err(Error::Rejected(Reason::PolicyUnavailable));
                }
            }
            for effect in &target.effects {
                if !self.valid_sources(&run, std::slice::from_ref(&effect.source), port)? {
                    return Err(Error::Rejected(Reason::InvalidSource));
                }
                let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM memory_effects WHERE run_id=?1 AND entry_id=?2 AND session_id=?3 AND record_seq=?4 AND kind=?5)",params![plan.run_id,target.entry_id,effect.source.session_id,effect.source.record_seq,enum_text(effect.kind)?],read_bool).map_err(sql)?;
                if exists {
                    return Err(Error::Rejected(Reason::InvalidSource));
                }
            }
        }
        for material in &plan.processed_materials {
            let source = self.material_source(conn, &plan.run_id, material)?;
            if !self.valid_sources(&run, &[source], port)? {
                return Err(Error::Rejected(Reason::InvalidSource));
            }
            let processed: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM memory_processed WHERE material_id=?1)",
                    [material],
                    read_bool,
                )
                .map_err(sql)?;
            if processed {
                return Err(Error::Rejected(Reason::StaleVersion));
            }
        }
        Ok(())
    }
    fn register_version(&self, conn: &Connection, version: &Version) -> Result<()> {
        let bytes = canonical(version)?;
        conn.execute("INSERT INTO memory_versions(version_id,entry_id,parent_version_id,restored_from_version_id,body_hash,body_length,source_set_hash,evidence_set_hash,policy_hash,subject_json,change_kind) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![version.version_id,version.entry_id,version.parent_version_id,version.restored_from_version_id,hash(&bytes).as_slice(),bytes.len(),version.source_set_hash.as_slice(),version.evidence_set_hash.as_slice(),version.policy_hash.as_slice(),canonical(&version.subject)?,enum_text(version.change)?]).map_err(sql)?;
        for (ordinal, source) in version.sources.iter().enumerate() {
            conn.execute(
                "INSERT INTO memory_version_sources VALUES(?1,?2,?3)",
                params![version.version_id, ordinal, canonical(source)?],
            )
            .map_err(sql)?;
        }
        for evidence in &version.evidence {
            conn.execute(
                "INSERT INTO memory_version_evidence VALUES(?1,?2,?3)",
                params![
                    version.version_id,
                    evidence.evidence_id,
                    canonical(evidence)?
                ],
            )
            .map_err(sql)?;
        }
        Ok(())
    }
    /// # Errors
    /// 新文件使用排他发布；同身份不同字节隔离操作，不覆盖旧文件。
    /// I/O 返回不确定时保留 prepared；主动恢复核验已登记文件，不自行重做。
    pub fn write_versions(
        &self,
        conn: &mut Connection,
        operation_id: &str,
    ) -> Result<OperationState> {
        let (plan, state) = self.operation(conn, operation_id)?;
        self.ensure_writable(conn, &plan.run_id)?;
        if state != OperationState::Prepared {
            return Ok(state);
        }
        let run = self.run(conn, &plan.run_id)?;
        for target in &plan.targets {
            if let Some(version) = &target.next {
                let path = self.version_path(&run, &target.entry_id, &version.version_id)?;
                let bytes = canonical(version)?;
                match aoidos_store::read::read_text_bounded(&path, MAX_VERSION_BYTES as u32)? {
                    Some(existing) if existing.as_bytes() != bytes => {
                        self.set_state(
                            conn,
                            operation_id,
                            OperationState::Quarantined,
                            "bodyConflict",
                        )?;
                        return Err(Error::Corrupt);
                    }
                    Some(_) => {}
                    None => {
                        aoidos_store::atomic::write_new_atomic(&path, &bytes)?;
                    }
                }
                self.read_version(conn, &run, &target.entry_id, &version.version_id)?;
            }
        }
        self.set_state(conn, operation_id, OperationState::Ready, "")?;
        Ok(OperationState::Ready)
    }
    /// # Errors
    /// 依赖组在一个事务中 CAS；失败不撤销独立操作，不修改不确定提交的状态。
    pub fn apply(
        &self,
        conn: &mut Connection,
        port: &dyn CausalPort,
        operation_id: &str,
    ) -> Result<OperationState> {
        let (plan, state) = self.operation(conn, operation_id)?;
        self.ensure_writable(conn, &plan.run_id)?;
        if state == OperationState::Applied {
            self.verify_applied(conn, &plan)?;
            return Ok(state);
        }
        if state != OperationState::Ready {
            return Ok(state);
        }
        if let Err(error) = self.check_plan(conn, port, &plan) {
            if matches!(error, Error::Rejected(_)) {
                self.set_state(
                    conn,
                    operation_id,
                    OperationState::Rejected,
                    "staleOrInvalidSource",
                )?;
            }
            return Err(error);
        }
        let run = self.run(conn, &plan.run_id)?;
        for target in &plan.targets {
            if let Some(version) = &target.next {
                self.read_version(conn, &run, &target.entry_id, &version.version_id)?;
            }
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        self.check_plan(&tx, port, &plan)?;
        for target in &plan.targets {
            let next = target
                .next
                .as_ref()
                .map(|v| v.version_id.as_str())
                .or(target.expected.version_id.as_deref())
                .ok_or(Error::Corrupt)?;
            let changed=tx.execute("UPDATE memory_entries SET current_version_id=?1,state_revision=state_revision+1,status=?2,restore_status=CASE WHEN ?2 IN ('active','archived') THEN ?2 ELSE restore_status END,recovery_cursor=NULL,recovery_history_revision=NULL,recovery_visited=NULL WHERE entry_id=?3 AND run_id=?4 AND current_version_id IS ?5 AND state_revision=?6",params![next,enum_text(target.state)?,target.entry_id,plan.run_id,target.expected.version_id,target.expected.state_revision]).map_err(sql)?;
            if changed != 1 {
                return Err(Error::Rejected(Reason::StaleVersion));
            }
            for effect in &target.effects {
                tx.execute("INSERT INTO memory_effects(effect_id,operation_id,entry_id,run_id,session_id,record_seq,kind,logical_position,gain,source_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![effect.effect_id,plan.operation_id,target.entry_id,plan.run_id,effect.source.session_id,effect.source.record_seq,enum_text(effect.kind)?,effect.logical_position,effect.gain,canonical(&effect.source)?]).map_err(sql)?;
            }
        }
        for material in &plan.processed_materials {
            tx.execute(
                "INSERT INTO memory_processed VALUES(?1,?2)",
                params![material, plan.operation_id],
            )
            .map_err(sql)?;
            tx.execute(
                "UPDATE memory_materials SET status='processed' WHERE material_id=?1",
                [material],
            )
            .map_err(sql)?;
        }
        tx.execute(
            "INSERT INTO store_applied(id,content_hash) VALUES(?1,?2)",
            params![
                format!("memory:{}", plan.operation_id),
                hex(&hash(&canonical(&plan)?))
            ],
        )
        .map_err(sql)?;
        self.set_state(&tx, operation_id, OperationState::Applied, "")?;
        tx.commit().map_err(sql)?;
        self.verify_applied(conn, &plan)?;
        Ok(OperationState::Applied)
    }
    /// # Errors
    /// 计划 hash 与持久目标关系不吻合为损坏，不继续提交。
    pub fn operation(&self, conn: &Connection, id: &str) -> Result<(Plan, OperationState)> {
        if !valid_id(id) {
            return Err(reject());
        }
        let identity: Option<(String, String, bool, bool)> = conn
            .query_row(
                "SELECT run_id,state,EXISTS(SELECT 1 FROM store_applied WHERE id=?2),EXISTS(SELECT 1 FROM memory_runs WHERE run_id=memory_operations.run_id) FROM memory_operations WHERE operation_id=?1",
                params![id, format!("memory:{id}")],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(sql)?;
        let (run_id, persisted_state, has_applied_marker, persisted_run_exists) =
            identity.ok_or(Error::NotFound)?;
        let claims_applied = persisted_state == "applied";
        let result = self.read_operation(conn, id, &run_id);
        if !persisted_run_exists {
            if let Ok((plan, _)) = &result {
                self.freeze_corrupt_run_if_present(conn, &plan.run_id, id)?;
            }
            return Err(Error::Corrupt);
        }
        if has_applied_marker != claims_applied {
            self.freeze_corrupt_run_if_present(conn, &run_id, id)?;
            return Err(Error::Corrupt);
        }
        let corrupt = matches!(
            result,
            Err(Error::Corrupt | Error::Rejected(Reason::InvalidSchema))
        ) || matches!(&result, Err(Error::Store(error)) if error.code() == "corrupt");
        if claims_applied && corrupt {
            self.freeze_corrupt_run_if_present(conn, &run_id, id)?;
            return Err(Error::Corrupt);
        }
        result
    }
    fn read_operation(
        &self,
        conn: &Connection,
        id: &str,
        persisted_run_id: &str,
    ) -> Result<(Plan, OperationState)> {
        let (bytes, state) = self.operation_row(conn, id)?.ok_or(Error::NotFound)?;
        let bytes = bytes.into_result()?;
        let plan: Plan = decode(&bytes, MAX_PLAN_BYTES)?;
        if plan.run_id != persisted_run_id {
            self.freeze_corrupt_run_if_present(conn, persisted_run_id, id)?;
            self.freeze_corrupt_run_if_present(conn, &plan.run_id, id)?;
            return Err(Error::Corrupt);
        }
        if plan.operation_id != id {
            return Err(Error::Corrupt);
        }
        self.validate_plan(&plan, &bytes)?;
        let expected = conn
            .query_row(
                "SELECT length(plan_hash),plan_hash FROM memory_operations WHERE operation_id=?1",
                [id],
                |row| read_bounded_blob(row, 0, 1, 32),
            )
            .map_err(sql)?
            .into_result()?;
        if expected != hash(&bytes) {
            return Err(Error::Corrupt);
        }
        let target_count: usize = conn
            .query_row(
                "SELECT count(*) FROM memory_operation_targets WHERE operation_id=?1",
                [id],
                read_count,
            )
            .map_err(sql)?;
        if target_count != plan.targets.len() {
            return Err(Error::Corrupt);
        }
        let state = decode_enum(&state)?;
        if state == OperationState::Applied {
            self.verify_applied(conn, &plan)?;
        }
        Ok((plan, state))
    }
    pub(super) fn set_state(
        &self,
        conn: &Connection,
        id: &str,
        state: OperationState,
        reason: &str,
    ) -> Result<()> {
        conn.execute(
            "UPDATE memory_operations SET state=?1,reason=?2 WHERE operation_id=?3",
            params![enum_text(state)?, reason, id],
        )
        .map_err(sql)?;
        Ok(())
    }
    fn operation_row(&self, conn: &Connection, id: &str) -> Result<Option<(BoundedBlob, String)>> {
        conn.query_row(
            "SELECT length(plan_json),plan_json,state FROM memory_operations WHERE operation_id=?1",
            [id],
            read_operation,
        )
        .optional()
        .map_err(sql)
    }
    pub(super) fn verify_applied(&self, conn: &Connection, plan: &Plan) -> Result<()> {
        match self.verify_applied_inner(conn, plan) {
            Err(Error::Corrupt | Error::NotFound) => {
                self.freeze_corrupt_run(conn, &plan.run_id, &plan.operation_id)?;
                Err(Error::Corrupt)
            }
            Err(error) if error.code() == "store.corrupt" => {
                self.freeze_corrupt_run(conn, &plan.run_id, &plan.operation_id)?;
                Err(error)
            }
            result => result,
        }
    }
    fn verify_applied_inner(&self, conn: &Connection, plan: &Plan) -> Result<()> {
        if !aoidos_store::applied::contains(
            conn,
            &format!("memory:{}", plan.operation_id),
            &hex(&hash(&canonical(plan)?)),
        )? {
            return Err(Error::Corrupt);
        }
        // 回执必须覆盖完整集合，额外索引行也不能变成未登记的效果或素材消费。
        let complete: bool = conn.query_row(
            "SELECT (SELECT count(*) FROM memory_effects WHERE operation_id=?1)=?2 AND (SELECT count(*) FROM memory_processed WHERE operation_id=?1)=?3",
            params![plan.operation_id, plan.targets.iter().map(|target| target.effects.len()).sum::<usize>(), plan.processed_materials.len()],
            read_bool,
        ).map_err(sql)?;
        if !complete {
            return Err(Error::Corrupt);
        }
        for target in &plan.targets {
            let recorded=conn.query_row("SELECT expected_version_id,expected_state_revision,next_version_id,next_state_revision FROM memory_operation_targets WHERE operation_id=?1 AND entry_id=?2",params![plan.operation_id,target.entry_id],read_target).optional().map_err(sql)?.ok_or(Error::Corrupt)?;
            if recorded
                != (
                    target.expected.version_id.clone(),
                    target.expected.state_revision,
                    target.next.as_ref().map(|v| v.version_id.clone()),
                    target.expected.state_revision + 1,
                )
            {
                return Err(Error::Corrupt);
            }
            for effect in &target.effects {
                let operation = conn
                    .query_row(
                        "SELECT operation_id FROM memory_effects WHERE effect_id=?1 AND run_id=?2 AND session_id=?3 AND record_seq=?4",
                        params![effect.effect_id, plan.run_id, effect.source.session_id, effect.source.record_seq],
                        read_string,
                    )
                    .optional()
                    .map_err(sql)?;
                if operation.as_deref() != Some(&plan.operation_id) {
                    return Err(Error::Corrupt);
                }
                let recorded = conn
                    .query_row(
                        "SELECT length(source_json),source_json FROM memory_effects WHERE effect_id=?1",
                        [&effect.effect_id],
                        |row| read_bounded_blob(row, 0, 1, 32 * 1024),
                    )
                    .optional()
                    .map_err(sql)?
                    .ok_or(Error::Corrupt)?
                    .into_result()?;
                let values = conn.query_row("SELECT entry_id,kind,logical_position,gain FROM memory_effects WHERE effect_id=?1", [&effect.effect_id],read_effect_values).optional().map_err(sql)?.ok_or(Error::Corrupt)?;
                if recorded != canonical(&effect.source)?
                    || values
                        != (
                            target.entry_id.clone(),
                            enum_text(effect.kind)?,
                            effect.logical_position,
                            effect.gain,
                        )
                {
                    return Err(Error::Corrupt);
                }
            }
            if let Some(version) = &target.next {
                let metadata =
                    self.version_metadata(conn, &target.entry_id, &version.version_id)?;
                if metadata.hash != hash(&canonical(version)?)
                    || metadata.parent != version.parent_version_id
                    || metadata.restored_from != version.restored_from_version_id
                {
                    return Err(Error::Corrupt);
                }
            }
            let current = self.entry(conn, &plan.run_id, &target.entry_id)?;
            if current.state_revision < target.expected.state_revision + 1
                || (current.state_revision == target.expected.state_revision + 1
                    && (Some(&current.current_version_id)
                        != target
                            .next
                            .as_ref()
                            .map(|version| &version.version_id)
                            .or(target.expected.version_id.as_ref())
                        || current.state != target.state))
            {
                return Err(Error::Corrupt);
            }
        }
        for material in &plan.processed_materials {
            let operation = conn
                .query_row(
                    "SELECT operation_id FROM memory_processed WHERE material_id=?1",
                    [material],
                    read_string,
                )
                .optional()
                .map_err(sql)?;
            if operation.as_deref() != Some(&plan.operation_id) {
                return Err(Error::Corrupt);
            }
            let processed: bool = conn
                .query_row(
                    "SELECT status='processed' FROM memory_materials WHERE material_id=?1",
                    [material],
                    read_bool,
                )
                .optional()
                .map_err(sql)?
                .ok_or(Error::Corrupt)?;
            if !processed {
                return Err(Error::Corrupt);
            }
        }
        Ok(())
    }
}
fn read_expected(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, Option<String>, u64)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}
fn read_bool(row: &rusqlite::Row<'_>) -> rusqlite::Result<bool> {
    row.get(0)
}
fn read_count(row: &rusqlite::Row<'_>) -> rusqlite::Result<usize> {
    row.get(0)
}
fn read_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<(BoundedBlob, String)> {
    Ok((read_bounded_blob(row, 0, 1, MAX_PLAN_BYTES)?, row.get(2)?))
}
fn read_target(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(Option<String>, u64, Option<String>, u64)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}
fn hex(hash: &Hash) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}
pub(super) fn same_content(a: &Version, b: &Version) -> bool {
    a.subject == b.subject
        && a.dimension == b.dimension
        && a.summary == b.summary
        && a.attitude == b.attitude
        && a.inferred == b.inferred
        && a.sources == b.sources
        && a.evidence == b.evidence
        && a.policy_hash == b.policy_hash
}

#[cfg(test)]
mod tests;

fn read_effect_history(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<u64>> {
    row.get(0)
}

fn read_effect_values(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String, u64, u64)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}
