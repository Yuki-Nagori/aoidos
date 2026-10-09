//! 只投影已确认的有效因果路径；恢复计划不采随机、不发送请求，也不改写原块。

use super::state::{Checkpoint, CheckpointStage, Operation, OperationOutcome, Phase};
use crate::{
    fault::Fault,
    ports::{Outcome, Terminal},
    record::{
        facts::{CheckPlan, DiceMode, Fact, RoundIdentity, ScenePosition},
        format::{Body, InputMode},
        session::{Session, store_fault},
    },
};

/// 当前回合引用的确认事实；resume 的新身份继承锚点前的步骤，不继承旧终态。
#[derive(Debug, Clone)]
pub struct RoundFacts {
    pub identity: RoundIdentity,
    pub input_seq: u64,
    pub accepted_seq: u64,
    pub dice_mode: DiceMode,
    pub mode: InputMode,
    pub scene_id: String,
    pub plan: Option<(u64, CheckPlan)>,
    pub skipped: Option<u64>,
    pub dice: Option<u64>,
    pub check: Option<u64>,
    pub narrative: Option<u64>,
    pub settlement: Option<u64>,
    pub settled: Option<u64>,
    pub advance: Option<u64>,
    pub ended: Option<(u64, Outcome, Option<Fault>)>,
    abandoned: bool,
}
impl RoundFacts {
    fn belongs(&self, identity: &RoundIdentity) -> bool {
        self.identity.round_id == identity.round_id
            && self.identity.operation_id == identity.operation_id
    }
    fn truncate(&mut self, through: u64) {
        self.plan = self.plan.take().filter(|(seq, _)| *seq <= through);
        for seq in [
            &mut self.skipped,
            &mut self.dice,
            &mut self.check,
            &mut self.narrative,
            &mut self.settlement,
            &mut self.settled,
            &mut self.advance,
        ] {
            *seq = seq.filter(|value| *value <= through);
        }
        self.ended = None;
        self.abandoned = false;
    }
    fn anchor(&self, seq: u64) -> bool {
        self.plan.as_ref().is_some_and(|(value, _)| *value == seq)
            || [
                self.skipped,
                self.dice,
                self.check,
                self.narrative,
                self.settlement,
                self.settled,
                self.advance,
            ]
            .contains(&Some(seq))
    }
}

/// 无 live owner 的打开结果；close 是待幂等提交的失败事实，不是已发布的终态。
#[derive(Debug, Clone)]
pub struct Recovery {
    pub phase: Phase,
    pub scene: Option<ScenePosition>,
    pub checkpoint: Option<Checkpoint>,
    pub last_operation: Option<Operation>,
    pub round: Option<RoundFacts>,
    pub needs_recovery: bool,
    pub close: Option<Fact>,
}

/// 在 022 已核验 applied / 控制并封存 partial 后调用；baseline 来自可信世界注册域。
/// # Errors
/// 因果引用失配、重复步骤、未知事实或已提交行损坏返回 store.corrupt；不猜恢复位置。
pub fn derive(session: &Session, baseline: Option<ScenePosition>) -> Result<Recovery, Fault> {
    derive_prefix(session, baseline, None)
}
pub(crate) fn derive_prefix(
    session: &Session,
    baseline: Option<ScenePosition>,
    through: Option<u64>,
) -> Result<Recovery, Fault> {
    let mut path = session.path_at(session.history_revision())?;
    if let Some(target) = through {
        if !path.contains(&target) {
            return Err(Fault::bad_request());
        }
        path.retain(|seq| *seq <= target);
    }
    derive_path(
        session,
        baseline,
        path,
        session.history_revision(),
        session.needs_recovery() || session.is_read_only(),
    )
}

pub(super) fn derive_path(
    session: &Session,
    baseline: Option<ScenePosition>,
    path: Vec<u64>,
    revision: u64,
    needs_recovery: bool,
) -> Result<Recovery, Fault> {
    let mut result = Recovery {
        phase: Phase::Idle,
        scene: baseline,
        checkpoint: None,
        last_operation: None,
        round: None,
        needs_recovery,
        close: None,
    };
    if result.needs_recovery {
        return Ok(result);
    }
    for seq in path {
        let body = session
            .read(seq)
            .map_err(store_fault)?
            .body
            .ok_or_else(corrupt)?
            .body;
        apply_record(&mut result, seq, body)?;
    }
    project(&mut result, revision);
    if revision
        > result.round.as_ref().map_or(0, |round| {
            round
                .ended
                .as_ref()
                .map_or(round.accepted_seq, |(seq, _, _)| *seq)
        })
    {
        let body = session
            .read(revision)
            .map_err(store_fault)?
            .body
            .ok_or_else(corrupt)?
            .body;
        let Body::System { code, data, .. } = body else {
            return Err(corrupt());
        };
        let Fact::HistoryFork { operation_id, .. } =
            Fact::decode(&code, &data).map_err(store_fault)?
        else {
            return Err(corrupt());
        };
        result.last_operation = Some(Operation {
            operation_id,
            round_id: None,
            outcome: OperationOutcome::Completed,
            error: None,
        });
    }
    Ok(result)
}

/// 首次 / fork 扫描与运行期尾部增量使用同一规则，不为每个 effect 重读全部正文。
pub(super) fn apply_record(result: &mut Recovery, seq: u64, body: Body) -> Result<(), Fault> {
    match body {
        Body::System { code, data, .. } => {
            let fact = Fact::decode(&code, &data).map_err(store_fault)?;
            apply_fact(result, seq, fact)?;
        }
        Body::Dice { plan_id, .. } => {
            let round = result.round.as_mut().ok_or_else(corrupt)?;
            if !round
                .plan
                .as_ref()
                .is_some_and(|(_, plan)| plan.plan_id == plan_id)
                || round.skipped.is_some()
            {
                return Err(corrupt());
            }
            once(&mut round.dice, seq)?;
        }
        Body::Check {
            dice_seq, plan_id, ..
        } => {
            let round = result.round.as_mut().ok_or_else(corrupt)?;
            if round.dice != Some(dice_seq)
                || !round
                    .plan
                    .as_ref()
                    .is_some_and(|(_, plan)| plan.plan_id == plan_id)
            {
                return Err(corrupt());
            }
            once(&mut round.check, seq)?;
        }
        Body::Narration {
            terminal: Terminal::Completed { .. },
            ..
        }
        | Body::CharacterSpeech {
            terminal: Terminal::Completed { .. },
            ..
        } => {
            if let Some(round) = result.round.as_mut().filter(|r| r.ended.is_none()) {
                if round.check.is_none() && round.skipped.is_none() {
                    return Err(corrupt());
                }
                once(&mut round.narrative, seq)?;
            }
        }
        // 失败正文 / orphan 保留在历史，但不能成为 completed 工作的锚点。
        _ => {}
    }
    Ok(())
}

fn apply_fact(result: &mut Recovery, seq: u64, fact: Fact) -> Result<(), Fault> {
    if let Fact::RoundAccepted {
        identity,
        input_seq,
        dice_mode,
        mode,
        source_round_id,
        checkpoint_seq,
        scene_id,
        ..
    } = fact
    {
        let mut round = if let (Some(source), Some(anchor)) = (source_round_id, checkpoint_seq) {
            let mut previous = result.round.take().ok_or_else(corrupt)?;
            if previous.identity.round_id != source
                || !previous.anchor(anchor)
                || previous.input_seq != input_seq
                || previous.mode != mode
            {
                return Err(corrupt());
            }
            previous.truncate(anchor);
            previous
        } else {
            if result
                .round
                .as_ref()
                .is_some_and(|previous| previous.ended.is_none() && !previous.abandoned)
            {
                return Err(corrupt());
            }
            RoundFacts {
                identity: identity.clone(),
                input_seq,
                accepted_seq: seq,
                dice_mode,
                mode,
                scene_id: scene_id.clone(),
                plan: None,
                skipped: None,
                dice: None,
                check: None,
                narrative: None,
                settlement: None,
                settled: None,
                advance: None,
                ended: None,
                abandoned: false,
            }
        };
        round.identity = identity;
        round.input_seq = input_seq;
        round.accepted_seq = seq;
        round.dice_mode = dice_mode;
        round.mode = mode;
        round.scene_id = scene_id;
        result.round = Some(round);
        return Ok(());
    }
    match fact {
        Fact::Orphan { .. } | Fact::HistoryFork { .. } => return Ok(()),
        Fact::AbandonCheckpoint {
            source_round_id,
            checkpoint_seq,
            ..
        } => {
            let round = result.round.as_mut().ok_or_else(corrupt)?;
            if round.identity.round_id != source_round_id || !round.anchor(checkpoint_seq) {
                return Err(corrupt());
            }
            round.abandoned = true;
            return Ok(());
        }
        _ => {}
    }
    let round = result.round.as_mut().ok_or_else(corrupt)?;
    match fact {
        Fact::CheckPlanned { identity, plan, .. } if round.belongs(&identity) => {
            if round.plan.is_some() || round.skipped.is_some() {
                return Err(corrupt());
            }
            round.plan = Some((seq, plan));
        }
        Fact::CheckSkipped { identity, .. } if round.belongs(&identity) => {
            if round.plan.is_some() {
                return Err(corrupt());
            }
            once(&mut round.skipped, seq)?;
        }
        Fact::SettlementPlanned {
            identity,
            narrative_seq,
            ..
        } if round.belongs(&identity) => {
            if round.narrative != Some(narrative_seq) {
                return Err(corrupt());
            }
            once(&mut round.settlement, seq)?;
        }
        Fact::RoundSettled {
            identity,
            narrative_seq,
            settlement_seq,
            ..
        } if round.belongs(&identity) => {
            if round.narrative != Some(narrative_seq) || round.settlement != Some(settlement_seq) {
                return Err(corrupt());
            }
            once(&mut round.settled, seq)?;
        }
        Fact::SceneAdvanced {
            identity, position, ..
        } if round.belongs(&identity) => {
            advance(round, seq)?;
            result.scene = Some(position);
        }
        Fact::SceneStayed { identity, .. } if round.belongs(&identity) => advance(round, seq)?,
        Fact::SessionEnded { identity, .. } if round.belongs(&identity) => {
            advance(round, seq)?;
            result.scene = None;
        }
        Fact::RoundEnded {
            identity,
            outcome,
            error,
            ..
        } if round.belongs(&identity) => {
            if round.ended.is_some() || (outcome == Outcome::Completed && round.advance.is_none()) {
                return Err(corrupt());
            }
            round.ended = Some((seq, outcome, error));
        }
        _ => return Err(corrupt()),
    }
    Ok(())
}
fn advance(round: &mut RoundFacts, seq: u64) -> Result<(), Fault> {
    if round.settled.is_none() {
        return Err(corrupt());
    }
    once(&mut round.advance, seq)
}
fn once(slot: &mut Option<u64>, seq: u64) -> Result<(), Fault> {
    if slot.replace(seq).is_some() {
        return Err(corrupt());
    }
    Ok(())
}
fn corrupt() -> Fault {
    Fault::new("store.corrupt", "回合因果步骤或恢复引用不一致")
}

pub(super) fn project(result: &mut Recovery, history_revision: u64) {
    result.phase = Phase::Idle;
    result.checkpoint = None;
    result.close = None;
    let Some(round) = &result.round else {
        return;
    };
    if let Some((_, outcome, error)) = &round.ended {
        result.last_operation = Some(Operation {
            operation_id: round.identity.operation_id.clone(),
            round_id: Some(round.identity.round_id.clone()),
            outcome: (*outcome).into(),
            error: error.clone(),
        });
    } else {
        result.last_operation = Some(Operation {
            operation_id: round.identity.operation_id.clone(),
            round_id: Some(round.identity.round_id.clone()),
            outcome: OperationOutcome::Accepted,
            error: None,
        });
    }
    if round.abandoned || matches!(round.ended, Some((_, Outcome::Completed, _))) {
        return;
    }
    let checkpoint = if let Some(settled) = round.settled {
        Some((
            Phase::Advancing,
            CheckpointStage::Advance,
            round.advance.unwrap_or(settled),
        ))
    } else if let Some(narrative) = round.narrative {
        Some((
            Phase::Settling,
            CheckpointStage::Settle,
            round.settlement.unwrap_or(narrative),
        ))
    } else if let Some(dice) = round.dice.filter(|_| round.check.is_none()) {
        Some((Phase::Settling, CheckpointStage::Check, dice))
    } else if let Some(check) = round.check {
        Some((Phase::Settling, CheckpointStage::Narration, check))
    } else if let Some((seq, _)) = &round.plan
        && (round.dice_mode == DiceMode::Manual || history_revision > *seq)
    {
        Some((Phase::AwaitingCheck, CheckpointStage::Check, *seq))
    } else {
        round
            .skipped
            .filter(|seq| history_revision > *seq)
            .map(|skip| (Phase::Idle, CheckpointStage::Narration, skip))
    };
    if let Some((phase, stage, through_seq)) = checkpoint {
        result.phase = phase;
        result.checkpoint = Some(Checkpoint {
            source_round_id: round.identity.round_id.clone(),
            through_seq,
            stage,
        });
    } else if round.ended.is_none() {
        result.close = Some(Fact::RoundEnded {
            identity: round.identity.clone(),
            outcome: Outcome::Failed,
            through_seq: round
                .plan
                .as_ref()
                .map(|(seq, _)| *seq)
                .or(round.skipped)
                .unwrap_or(round.accepted_seq),
            completed_steps: vec![],
            error: Some(Fault::new("engine.interrupted", "进程中断，已提交记录保留")),
        });
    }
}

#[cfg(test)]
mod tests;
