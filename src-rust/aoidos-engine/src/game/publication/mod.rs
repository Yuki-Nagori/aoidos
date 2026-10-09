//! 先预留事件，再确认状态与四基线；投递失败不会撤回已提交记录。

use super::state::{
    OperationOutcome, PhaseEvent, PhaseEvents, PhaseSnapshot, PhaseState, Sequences,
};
use crate::{fault::Fault, ports::Outcome, record::format};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

const MAX_EVENT_BYTES: usize = 64 * 1024;

/// 终态事件可属于已被插话替换的操作，不因此覆盖当前最近操作。
#[derive(Debug, Clone)]
pub struct Finished {
    pub operation_id: String,
    pub outcome: Outcome,
    pub error: Option<Fault>,
}

/// 单 session 确认副本；业务串行执行，查询只持有短内存锁。
pub struct Publisher {
    snapshot: Mutex<PhaseSnapshot>,
    events: Arc<dyn PhaseEvents>,
    retired: AtomicBool,
}
/// 不可公开构造的预留身份；确认时核验原 epoch / revision，防止旧操作覆盖。
pub struct Prepared {
    previous_revision: u64,
    state: PhaseState,
    seq: Sequences,
    events: Vec<(u64, PhaseEvent)>,
}
impl Publisher {
    /// 新打开的 session 使用新 stateEpoch，事件基线从零开始。
    /// # Errors
    /// 非法身份、场景路径、错误文案或状态组合拒绝注册。
    pub fn new(mut state: PhaseState, events: Arc<dyn PhaseEvents>) -> Result<Self, Fault> {
        state.state_epoch = uuid::Uuid::new_v4().to_string();
        state.phase_revision = 0;
        validate(&state)?;
        Ok(Self {
            snapshot: Mutex::new(PhaseSnapshot {
                state,
                seq: Sequences::default(),
            }),
            events,
            retired: AtomicBool::new(false),
        })
    }
    /// 一次短临界区读取状态及四条确认基线。
    pub fn snapshot(&self) -> PhaseSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    /// 终态也无法准备或存储结果不确定时冻结内存展示；不预留事件，不确认任何新记录。
    /// 快照保留原事件基线，客户端只能主动恢复；重新核验记录前不准继续生成。
    pub(super) fn quarantine(&self, error: Fault) {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.retired.load(Ordering::Acquire) {
            return;
        }
        snapshot.state.phase_revision = snapshot
            .state
            .phase_revision
            .saturating_add(1)
            .min(format::MAX_SEQ);
        snapshot.state.phase = super::state::Phase::Idle;
        snapshot.state.in_flight = None;
        snapshot.state.check = None;
        snapshot.state.resume_required = false;
        snapshot.state.checkpoint = None;
        snapshot.state.needs_recovery = true;
        if let Some(operation) = &mut snapshot.state.last_operation {
            operation.outcome = OperationOutcome::Failed;
            operation.error = Some(error);
        }
    }
    /// 预备完整状态 / 载荷及序号；之后才能执行关联的事实提交。
    /// # Errors
    /// 身份 / 修订不匹配、容量或平台准备失败时不能开始关联的持久写入。
    pub fn prepare(
        &self,
        mut next: PhaseState,
        finished: Option<Finished>,
    ) -> Result<Prepared, Fault> {
        if self.retired.load(Ordering::Acquire) {
            return Err(closed());
        }
        let previous = self.snapshot();
        if next.session_id != previous.state.session_id
            || next.state_epoch != previous.state.state_epoch
            || next.phase_revision != previous.state.phase_revision
            || next.history_revision < previous.state.history_revision
            || next.phase_revision >= format::MAX_SEQ
        {
            return Err(super::invalid_phase());
        }
        next.phase_revision += 1;
        if let Some(terminal) = &finished {
            if !format::valid_uuid(&terminal.operation_id)
                || (terminal.outcome == Outcome::Failed) != terminal.error.is_some()
            {
                return Err(Fault::bad_request());
            }
            if let Some(operation) = &mut next.last_operation
                && operation.operation_id == terminal.operation_id
            {
                operation.outcome = terminal.outcome.into();
                operation.error = terminal.error.clone();
            }
        }
        validate(&next)?;
        let mut payloads = vec![PhaseEvent::Changed(next.clone())];
        if previous.state.scene != next.scene {
            payloads.push(PhaseEvent::Scene {
                state: next.clone(),
                previous_scene_id: previous.state.scene.map(|scene| scene.scene_id),
            });
        }
        if let Some(terminal) = finished {
            payloads.push(match terminal.error {
                Some(error) => PhaseEvent::Failed {
                    state: next.clone(),
                    operation_id: terminal.operation_id,
                    code: error.code,
                    message: error.message,
                },
                None => PhaseEvent::Done {
                    state: next.clone(),
                    operation_id: terminal.operation_id,
                    outcome: terminal.outcome,
                },
            });
        }
        // 完整批次先校验容量，避免只因后一个载荷超限就预留前一个事件。
        for payload in &payloads {
            let bytes = aoidos_json::to_vec(payload).map_err(encode_error)?;
            if bytes.len() > MAX_EVENT_BYTES {
                return Err(Fault::bad_request());
            }
            if let PhaseEvent::Failed { code, message, .. } = payload {
                validate_error(code, message)?;
            }
        }
        let mut seq = previous.seq;
        let mut events = Vec::with_capacity(payloads.len());
        for payload in payloads {
            let reserved = self.events.prepare(&payload)?;
            let baseline = baseline(&mut seq, &payload);
            if reserved <= *baseline || reserved > format::MAX_SEQ {
                return Err(Fault::event());
            }
            *baseline = reserved;
            events.push((reserved, payload));
        }
        Ok(Prepared {
            previous_revision: previous.state.phase_revision,
            state: next,
            seq,
            events,
        })
    }
    /// 已完成事实 / applied 提交后确认整批状态；平台投递在释放内存锁后执行。
    /// # Errors
    /// 过期预留拒绝确认；投递失败返回故障，但快照和基线仍包含已确认状态。
    pub fn confirm(&self, prepared: Prepared) -> Result<(), Fault> {
        match self.confirm_committed(prepared)? {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    /// 区分确认失败与确认后的窗口投递故障；终态封口不能因窗口故障冻结存储。
    pub(super) fn confirm_committed(&self, prepared: Prepared) -> Result<Option<Fault>, Fault> {
        {
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.retired.load(Ordering::Acquire) {
                return Err(closed());
            }
            if snapshot.state.phase_revision != prepared.previous_revision
                || snapshot.state.state_epoch != prepared.state.state_epoch
                || snapshot.state.session_id != prepared.state.session_id
            {
                return Err(super::invalid_phase());
            }
            *snapshot = PhaseSnapshot {
                state: prepared.state,
                seq: prepared.seq,
            };
        }
        let mut fault = None;
        for (seq, event) in prepared.events {
            if let Err(error) = self.events.deliver(seq, event) {
                fault.get_or_insert(error);
            }
        }
        Ok(fault)
    }
    /// 所有生产者及投递退出后，在登记锁仍持有旧身份时清退；迟到 Arc 的 Drop 不再清退新实例。
    pub fn retire(&self) {
        if !self.retired.swap(true, Ordering::AcqRel) {
            self.events.retire(&self.snapshot().state.session_id);
        }
    }
}
impl Drop for Publisher {
    fn drop(&mut self) {
        self.retire();
    }
}
fn closed() -> Fault {
    Fault::new("app.not-ready", "阶段发布实例已关闭")
}
fn baseline<'a>(seq: &'a mut Sequences, event: &PhaseEvent) -> &'a mut u64 {
    match event {
        PhaseEvent::Changed(_) => &mut seq.phase_changed,
        PhaseEvent::Scene { .. } => &mut seq.scene_advanced,
        PhaseEvent::Done { .. } => &mut seq.operation_done,
        PhaseEvent::Failed { .. } => &mut seq.operation_failed,
    }
}
fn validate(state: &PhaseState) -> Result<(), Fault> {
    if !format::valid_uuid(&state.session_id)
        || !format::valid_uuid(&state.state_epoch)
        || state.phase_revision > format::MAX_SEQ
        || state.history_revision > format::MAX_SEQ
        || (state.in_flight.is_some() && state.resume_required)
        || state.resume_required != state.checkpoint.is_some()
        || (state.needs_recovery && state.checkpoint.is_some())
    {
        return Err(Fault::bad_request());
    }
    if let Some(scene) = &state.scene {
        let mut ids = std::collections::BTreeSet::new();
        if !format::valid_id(&scene.scene_id)
            || scene.path.is_empty()
            || scene.path.len() > 16
            || scene.path.last().map(|node| &node.id) != Some(&scene.scene_id)
            || scene.path.iter().any(|node| {
                !format::valid_id(&node.id)
                    || !format::valid_id(&node.kind)
                    || node.title.trim().is_empty()
                    || node.title.len() > 256
                    || !ids.insert(&node.id)
            })
        {
            return Err(Fault::bad_request());
        }
    }
    if let Some(active) = &state.in_flight
        && (!format::valid_uuid(&active.operation_id)
            || active
                .round_id
                .as_ref()
                .is_some_and(|id| !format::valid_uuid(id))
            || active
                .turn_id
                .as_ref()
                .is_some_and(|id| !format::valid_uuid(id)))
    {
        return Err(Fault::bad_request());
    }
    if let Some(check) = &state.check
        && (!format::valid_id(&check.plan_id)
            || !format::valid_id(&check.rule_id)
            || !format::valid_id(&check.actor_id)
            || format::dice_expression(&check.expression).is_none()
            || !(-100..=100).contains(&check.modifier_total))
    {
        return Err(Fault::bad_request());
    }
    if let Some(checkpoint) = &state.checkpoint
        && (!format::valid_uuid(&checkpoint.source_round_id)
            || checkpoint.through_seq == 0
            || checkpoint.through_seq > format::MAX_SEQ)
    {
        return Err(Fault::bad_request());
    }
    if let Some(operation) = &state.last_operation {
        if !format::valid_uuid(&operation.operation_id)
            || operation
                .round_id
                .as_ref()
                .is_some_and(|id| !format::valid_uuid(id))
            || (operation.outcome == OperationOutcome::Failed) != operation.error.is_some()
        {
            return Err(Fault::bad_request());
        }
        if let Some(error) = &operation.error {
            validate_error(&error.code, &error.message)?;
        }
    }
    Ok(())
}
fn validate_error(code: &str, message: &str) -> Result<(), Fault> {
    if !code
        .split_once('.')
        .is_some_and(|(domain, name)| format::valid_id(domain) && format::valid_id(name))
        || message.len() > 512
    {
        return Err(Fault::bad_request());
    }
    Ok(())
}
fn encode_error(_: aoidos_json::Error) -> Fault {
    Fault::bad_request()
}

#[cfg(test)]
mod tests;
