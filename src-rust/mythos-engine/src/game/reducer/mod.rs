//! 纯阶段转换；只返回副作用，确认记录后才交给发布层更新快照。

use super::state::Phase;
use crate::{
    fault::Fault,
    record::{
        facts::{CheckPlan, DiceMode},
        format::InputMode,
    },
};

/// 公开阶段之外的子状态，模型不能传入或覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Idle,
    Accepting,
    CheckProposal,
    PlanCommit,
    SkipCommit,
    Waiting,
    Rolling,
    CheckCommit,
    Narration,
    Settlement,
    SceneProposal,
    AdvanceCommit,
    Ending,
}
impl Stage {
    pub fn phase(self, checked: bool) -> Phase {
        match self {
            Self::Idle => Phase::Idle,
            Self::Waiting | Self::Rolling => Phase::AwaitingCheck,
            Self::CheckCommit | Self::Settlement => Phase::Settling,
            Self::Narration if checked => Phase::Settling,
            Self::SceneProposal | Self::AdvanceCommit | Self::Ending => Phase::Advancing,
            _ => Phase::Generating,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    CommitInput,
    ProposeCheck,
    CommitPlan,
    CommitSkip,
    RollDice,
    CommitCheck,
    StartNarration,
    ApplySettlement,
    SelectScene,
    CommitAdvance,
    EndRound,
}
/// token 联合 ownerEpoch / roundId 识别回执，不采随机，也不依赖操作系统。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectIdentity {
    pub owner_epoch: String,
    pub round_id: String,
    pub effect_id: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    pub identity: EffectIdentity,
    pub action: Action,
}
#[derive(Debug, Clone)]
pub struct State {
    pub stage: Stage,
    pub mode: InputMode,
    pub dice_mode: DiceMode,
    pub dice_disabled: bool,
    pub plan: Option<CheckPlan>,
    pub checked: bool,
    pub input_seq: Option<u64>,
    pub plan_seq: Option<u64>,
    pub skip_seq: Option<u64>,
    pub dice_seq: Option<u64>,
    pub check_seq: Option<u64>,
    pub narrative_seq: Option<u64>,
    pub settlement_seq: Option<u64>,
    pub advance_seq: Option<u64>,
    pub current: Option<Effect>,
    owner_epoch: String,
    round_id: String,
    next_effect: u64,
}
/// 内部回执只能由当前副作用提交方发出；过期回执直接丢弃。
#[derive(Debug, Clone)]
pub enum Receipt {
    Input(u64),
    NoCheck,
    Check(Box<CheckPlan>),
    Plan(u64),
    Skip(u64),
    Dice(u64),
    CheckResult(u64),
    Narrative(u64),
    Settlement(u64),
    SceneSelected,
    Advance(u64),
    Ended,
}
#[derive(Debug, Clone)]
pub enum Event {
    Begin,
    SubmitCheck,
    Completed {
        identity: EffectIdentity,
        receipt: Receipt,
    },
}
#[derive(Debug, Clone)]
pub struct Decision {
    pub state: State,
    pub effect: Option<Effect>,
    pub ignored: bool,
}
impl State {
    /// 显式 resume 的新回合继承确认步骤；有骰值时直接补分档，绝不返回 RollDice。
    /// # Errors
    /// 无确认检查点、pending 或锚点与步骤失配拒绝。
    pub fn resume(
        owner_epoch: String,
        round_id: String,
        recovery: &super::recovery::Recovery,
        dice_mode: DiceMode,
    ) -> Result<Self, Fault> {
        use super::state::CheckpointStage;
        let round = recovery.round.as_ref().ok_or_else(super::invalid_phase)?;
        let checkpoint = recovery
            .checkpoint
            .as_ref()
            .ok_or_else(super::invalid_phase)?;
        let expected_anchor = match checkpoint.stage {
            CheckpointStage::Check if round.dice.is_some() => round.dice,
            CheckpointStage::Check => round.plan.as_ref().map(|(seq, _)| *seq),
            CheckpointStage::Narration => round.check.or(round.skipped),
            CheckpointStage::Settle => round.settlement.or(round.narrative),
            CheckpointStage::Advance => round.advance.or(round.settled),
        };
        if recovery.needs_recovery
            || checkpoint.source_round_id != round.identity.round_id
            || expected_anchor != Some(checkpoint.through_seq)
        {
            return Err(super::invalid_phase());
        }
        let mut state = Self::new(
            owner_epoch,
            round_id,
            round.mode,
            dice_mode,
            false,
            round.plan.as_ref().map(|(_, plan)| plan.clone()),
        );
        state.input_seq = Some(round.input_seq);
        state.plan_seq = round.plan.as_ref().map(|(seq, _)| *seq);
        state.skip_seq = round.skipped;
        state.dice_seq = round.dice;
        state.check_seq = round.check;
        state.checked = round.check.is_some();
        state.narrative_seq = round.narrative;
        state.settlement_seq = round.settled;
        state.advance_seq = round.advance;
        let action = match checkpoint.stage {
            CheckpointStage::Check if round.dice.is_some() && round.check.is_none() => {
                state.stage = Stage::CheckCommit;
                Some(Action::CommitCheck)
            }
            CheckpointStage::Check if round.plan.is_some() && round.dice.is_none() => {
                if dice_mode == DiceMode::Manual {
                    state.stage = Stage::Waiting;
                    None
                } else {
                    state.stage = Stage::Rolling;
                    Some(Action::RollDice)
                }
            }
            CheckpointStage::Narration if round.check.is_some() || round.skipped.is_some() => {
                state.stage = Stage::Narration;
                Some(Action::StartNarration)
            }
            CheckpointStage::Settle if round.narrative.is_some() && round.settled.is_none() => {
                state.stage = Stage::Settlement;
                Some(Action::ApplySettlement)
            }
            CheckpointStage::Advance if round.settled.is_some() => {
                if round.advance.is_some() {
                    state.stage = Stage::Ending;
                    Some(Action::EndRound)
                } else {
                    state.stage = Stage::SceneProposal;
                    Some(Action::SelectScene)
                }
            }
            _ => return Err(super::invalid_phase()),
        };
        state.issue(action)?;
        Ok(state)
    }
    pub fn new(
        owner_epoch: String,
        round_id: String,
        mode: InputMode,
        dice_mode: DiceMode,
        dice_disabled: bool,
        forced_plan: Option<CheckPlan>,
    ) -> Self {
        Self {
            stage: Stage::Idle,
            mode,
            dice_mode,
            dice_disabled,
            plan: forced_plan,
            checked: false,
            input_seq: None,
            plan_seq: None,
            skip_seq: None,
            dice_seq: None,
            check_seq: None,
            narrative_seq: None,
            settlement_seq: None,
            advance_seq: None,
            current: None,
            owner_epoch,
            round_id,
            next_effect: 1,
        }
    }
    /// 计算下一候选与有序 effect；driver 持久化成功后才发布对应状态。
    /// # Errors
    /// 未列出的转移 / 当前 effect 的错误回执及身份耗尽拒绝。
    pub fn step(&self, event: Event) -> Result<Decision, Fault> {
        let mut next = self.clone();
        if let Event::Completed { identity, .. } = &event
            && self.current.as_ref().map(|e| &e.identity) != Some(identity)
        {
            return Ok(Decision {
                state: next,
                effect: None,
                ignored: true,
            });
        }
        let action = match (self.stage, event) {
            (Stage::Idle, Event::Begin) if self.input_seq.is_none() => {
                next.stage = Stage::Accepting;
                Some(Action::CommitInput)
            }
            (Stage::Waiting, Event::SubmitCheck) => {
                next.stage = Stage::Rolling;
                Some(Action::RollDice)
            }
            (Stage::Rolling, Event::SubmitCheck) => {
                return Ok(Decision {
                    state: next,
                    effect: None,
                    ignored: true,
                });
            }
            (
                Stage::Accepting,
                Event::Completed {
                    receipt: Receipt::Input(seq),
                    ..
                },
            ) => {
                next.input_seq = Some(seq);
                if self.mode == InputMode::OutOfCharacter || self.dice_disabled {
                    next.plan = None;
                    next.stage = Stage::SkipCommit;
                    Some(Action::CommitSkip)
                } else if self.plan.is_some() {
                    next.stage = Stage::PlanCommit;
                    Some(Action::CommitPlan)
                } else {
                    next.stage = Stage::CheckProposal;
                    Some(Action::ProposeCheck)
                }
            }
            (
                Stage::CheckProposal,
                Event::Completed {
                    receipt: Receipt::NoCheck,
                    ..
                },
            ) => {
                next.stage = Stage::SkipCommit;
                Some(Action::CommitSkip)
            }
            (
                Stage::CheckProposal,
                Event::Completed {
                    receipt: Receipt::Check(plan),
                    ..
                },
            ) => {
                next.plan = Some(*plan);
                next.stage = Stage::PlanCommit;
                Some(Action::CommitPlan)
            }
            (
                Stage::PlanCommit,
                Event::Completed {
                    receipt: Receipt::Plan(seq),
                    ..
                },
            ) => {
                next.plan_seq = Some(seq);
                if self.dice_mode == DiceMode::Auto {
                    next.stage = Stage::Rolling;
                    Some(Action::RollDice)
                } else {
                    next.stage = Stage::Waiting;
                    None
                }
            }
            (
                Stage::SkipCommit,
                Event::Completed {
                    receipt: Receipt::Skip(seq),
                    ..
                },
            ) => {
                next.skip_seq = Some(seq);
                next.stage = Stage::Narration;
                Some(Action::StartNarration)
            }
            (
                Stage::Rolling,
                Event::Completed {
                    receipt: Receipt::Dice(seq),
                    ..
                },
            ) => {
                next.dice_seq = Some(seq);
                next.stage = Stage::CheckCommit;
                Some(Action::CommitCheck)
            }
            (
                Stage::CheckCommit,
                Event::Completed {
                    receipt: Receipt::CheckResult(seq),
                    ..
                },
            ) => {
                next.check_seq = Some(seq);
                next.checked = true;
                next.stage = Stage::Narration;
                Some(Action::StartNarration)
            }
            (
                Stage::Narration,
                Event::Completed {
                    receipt: Receipt::Narrative(seq),
                    ..
                },
            ) => {
                next.narrative_seq = Some(seq);
                next.stage = Stage::Settlement;
                Some(Action::ApplySettlement)
            }
            (
                Stage::Settlement,
                Event::Completed {
                    receipt: Receipt::Settlement(seq),
                    ..
                },
            ) => {
                next.settlement_seq = Some(seq);
                next.stage = Stage::SceneProposal;
                Some(Action::SelectScene)
            }
            (
                Stage::SceneProposal,
                Event::Completed {
                    receipt: Receipt::SceneSelected,
                    ..
                },
            ) => {
                next.stage = Stage::AdvanceCommit;
                Some(Action::CommitAdvance)
            }
            (
                Stage::AdvanceCommit,
                Event::Completed {
                    receipt: Receipt::Advance(seq),
                    ..
                },
            ) => {
                next.advance_seq = Some(seq);
                next.stage = Stage::Ending;
                Some(Action::EndRound)
            }
            (
                Stage::Ending,
                Event::Completed {
                    receipt: Receipt::Ended,
                    ..
                },
            ) => {
                next.stage = Stage::Idle;
                None
            }
            _ => return Err(super::invalid_phase()),
        };
        next.issue(action)?;
        Ok(Decision {
            effect: next.current.clone(),
            state: next,
            ignored: false,
        })
    }
    fn issue(&mut self, action: Option<Action>) -> Result<(), Fault> {
        self.current = if let Some(action) = action {
            if self.next_effect > crate::record::format::MAX_SEQ {
                return Err(super::invalid_phase());
            }
            let effect = Effect {
                identity: EffectIdentity {
                    owner_epoch: self.owner_epoch.clone(),
                    round_id: self.round_id.clone(),
                    effect_id: self.next_effect,
                },
                action,
            };
            self.next_effect += 1;
            Some(effect)
        } else {
            None
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests;
