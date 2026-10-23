//! 一个游戏回合借用同一个应用 lease；存储工作开始后必须等到确认边界。

use super::{
    domain::{CompletedRound, CompletedRounds, Domain, FrozenRound},
    publication::{Finished, Publisher},
    recovery::{self, Recovery},
    reducer::{Action, Event, Receipt, Stage, State},
    state::{CheckStatus, CheckSummary, InFlight, Operation, OperationOutcome, Phase},
};
use crate::{
    fault::Fault,
    ports::{Outcome, OutputWriter, Terminal},
    record::{
        facts::{CheckPlan, DiceMode, Fact, GenerationTarget, RoundIdentity, SkipReason, Step},
        format::{Body, InputMode},
        grammar::PromptTarget,
        projection::{self, WorldView},
        session::{PersistentWriter, Session, Target, store_fault},
    },
    request::PreparedGeneration,
    turn::{Coordinator, Lease},
};
use aoidos_memory::model::{CycleEventKind, CycleLifecycleEvent};
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

type SharedDomain = Arc<Mutex<Box<dyn Domain>>>;

const CHECK_PROPOSAL_GUIDANCE: &str = "\
判定门槛：只有同时满足以下条件时才提议 check：当前行动的结果在已确认情境下确实不确定；成功与失败/代价都合理且会造成有意义的不同后果；并且存在准确适用的已登记规则。\n\
普通移动、观察、拿取触手可及的物品、平常交谈，以及没有阻力或实际风险的尝试，直接选择 noCheck 并交给叙事处理；不要因为玩家描述了一个动作就自动要求掷骰，也不要为了触发骰判而臆造危险。结果显而易见、行动不可能、只是表达意图，或失败不会产生不同后果时，也选择 noCheck。\n\
只依据当前玩家行动与已确认情境判断；若无法指出具体的不确定性、阻力和失败后果，就选择 noCheck。不得发明角色或规则。";

/// 注册域保留唯一 SQL 解释器；驱动与恢复均使用这份所有者，不另开连接。
pub struct Context {
    pub player_id: String,
    pub actor_id: String,
    pub session: Arc<Mutex<Session>>,
    pub domain: SharedDomain,
    pub publisher: Arc<Publisher>,
    pub coordinator: Coordinator,
    pub completed: Arc<dyn CompletedRounds>,
}
impl Context {
    /// 可信注册入口只恢复既有事实 / applied 并封存中断接纳；不自动生成或采随机。
    /// # Errors
    /// 非法玩家 / 场景、未知记录或初始化失败拒绝；解释器恢复失败保留只读快照。
    pub async fn open(
        session: Arc<Mutex<Session>>,
        mut domain: Box<dyn Domain>,
        player_id: String,
        actor_id: String,
        coordinator: Coordinator,
        events: Arc<dyn super::state::PhaseEvents>,
        completed: Arc<dyn CompletedRounds>,
    ) -> Result<Arc<Self>, Fault> {
        crate::blocking::run(move || {
            if !crate::record::format::valid_id(&player_id)
                || !crate::record::format::valid_id(&actor_id)
            {
                return Err(Fault::bad_request());
            }
            let mut record = lock(&session);
            let baseline = domain.baseline_scene();
            if let Some(position) = &baseline {
                domain.catalog().verify_position(position)?;
            }
            if record.needs_recovery() && !record.is_read_only() {
                // 未登记解释器保持 pending；任何失败都不启动新的付费或重掷步骤。
                if record.recover_history(domain.as_mut()).is_ok() {
                    let _ = record.recover_world(domain.as_mut());
                }
            }
            let mut recovered = recovery::derive(&record, baseline)?;
            if let Some(fact) = recovered.close.take() {
                let body = fact_body(&fact)?;
                let seq = record.append(body.clone())?;
                recovery::apply_record(&mut recovered, seq, body)?;
                recovery::project(&mut recovered, record.history_revision());
            }
            if let Some(position) = &recovered.scene {
                domain.catalog().verify_position(position)?;
            }
            let check = paused_check(&recovered);
            let initial = super::state::PhaseState {
                session_id: record.header().session_id.clone(),
                state_epoch: String::new(),
                phase_revision: 0,
                history_revision: record.history_revision(),
                phase: recovered.phase,
                scene: recovered.scene,
                in_flight: None,
                check,
                needs_recovery: recovered.needs_recovery || record.is_read_only(),
                resume_required: recovered.checkpoint.is_some(),
                checkpoint: recovered.checkpoint,
                last_operation: recovered.last_operation,
            };
            let publisher = Publisher::new(initial, events)?;
            drop(record);
            Ok(Arc::new(Self {
                player_id,
                actor_id,
                session,
                domain: Arc::new(Mutex::new(domain)),
                publisher: Arc::new(publisher),
                coordinator,
                completed,
            }))
        })
        .await
    }
}

/// actor 独占转换状态；异步子调用和阻塞提交只借用不可变资源。
pub struct Runner {
    context: Arc<Context>,
    lease: Lease,
    frozen: FrozenRound,
    state: State,
    recovered: Recovery,
    target: GenerationTarget,
    selected: Option<Fact>,
    public_turn: Option<String>,
    delivery_error: Option<Fault>,
    child_outcome: Option<Outcome>,
    pub identity: RoundIdentity,
}

pub(super) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
#[cfg(test)]
pub(super) use lock as test_lock;
fn fact_body(fact: &Fact) -> Result<Body, Fault> {
    fact.body("引擎确认回合事实").map_err(store_fault)
}

// 结算收尾直接携带可信意图元数据，不重新解析刚构造的 JSON 来判断步骤。
enum CommitMode {
    Record,
    World,
    Settlement {
        narrative_seq: u64,
        applied_through_mutation_id: Option<String>,
    },
}

impl Runner {
    /// 输入与接纳记录耐久确认后才能向客户端返回身份；此函数不启动 HTTP。
    /// # Errors
    /// 场景、规则、恢复状态、事件准备或持久化失败拒绝接纳。
    pub async fn accept(
        context: Arc<Context>,
        lease: Lease,
        frozen: FrozenRound,
        parsed: super::input::ParsedInput<'_>,
    ) -> Result<Self, Fault> {
        Self::accept_targeted(context, lease, frozen, parsed, GenerationTarget::Narration).await
    }

    /// 显式选择场内角色作为回复对象；目标由 Rust 目录与场景成员关系校验。
    /// # Errors
    /// 接收对象不在当前场景、非角色实体或等于玩家角色时拒绝，且不追加输入。
    pub async fn accept_targeted(
        context: Arc<Context>,
        lease: Lease,
        frozen: FrozenRound,
        parsed: super::input::ParsedInput<'_>,
        target: GenerationTarget,
    ) -> Result<Self, Fault> {
        Self::accept_targeted_with_presence_revision(context, lease, frozen, parsed, target, None)
            .await
    }

    /// 接收 UI 确认的在场版本；目标、版本与玩家输入在同一接纳事实中持久化。
    /// # Errors
    /// revision 已过期、角色已离场或快照不可用时，在追加输入前拒绝。
    pub async fn accept_targeted_with_presence_revision(
        context: Arc<Context>,
        lease: Lease,
        frozen: FrozenRound,
        parsed: super::input::ParsedInput<'_>,
        target: GenerationTarget,
        expected_presence_revision: Option<String>,
    ) -> Result<Self, Fault> {
        let raw = parsed.raw.to_owned();
        let mode = parsed.mode;
        let content_range = parsed.content_range;
        crate::blocking::run(move || {
            let mut session = lock(&context.session);
            let domain = lock(&context.domain);
            let recovered = recovery::derive(&session, domain.baseline_scene())?;
            if recovered.needs_recovery || recovered.close.is_some() {
                return Err(super::invalid_phase());
            }
            let position = recovered.scene.as_ref().ok_or_else(no_scene)?;
            domain.catalog().verify_position(position)?;
            let scene = domain.catalog().scene(&position.scene_id)?;
            if mode != InputMode::InCharacter
                && matches!(&target, GenerationTarget::CharacterSpeech { .. })
            {
                return Err(Fault::bad_request());
            }
            validate_generation_target(&target, &context.actor_id)
                .map_err(|_| target_unavailable())?;
            match (&target, expected_presence_revision.as_deref()) {
                (GenerationTarget::Narration, None) => {}
                (GenerationTarget::CharacterSpeech { speaker_id }, Some(expected)) => {
                    if !crate::record::format::valid_hash(expected) {
                        return Err(Fault::bad_request());
                    }
                    let presence = domain.scene_presence(&session)?;
                    if presence.session_id != session.header().session_id
                        || presence.scene_id != position.scene_id
                    {
                        return Err(Fault::new("engine.invalid-phase", "在场快照不可用"));
                    }
                    if !presence.characters.iter().any(|character| {
                        character.kind == crate::game::catalog::EntityKind::Character
                            && character.id == speaker_id.as_str()
                    }) {
                        return Err(target_unavailable());
                    }
                    if presence.presence_revision != expected {
                        return Err(stale_presence());
                    }
                }
                _ => return Err(Fault::bad_request()),
            }
            let view = domain.confirmed(&session)?;
            if !crate::record::format::valid_id(&context.player_id)
                || !scene.actor_ids.contains(&context.actor_id)
            {
                return Err(super::invalid_phase());
            }
            let forced = if mode == InputMode::InCharacter && !scene.dice_disabled {
                scene
                    .forced_check
                    .as_ref()
                    .map(|rule| domain.check_plan(rule, &context.actor_id, &view))
                    .transpose()?
            } else {
                None
            };
            if let Some(plan) = &forced {
                super::dice::validate_plan(plan, view.world_revision())?;
            }
            let identity = RoundIdentity {
                version: 1,
                round_id: uuid::Uuid::new_v4().to_string(),
                operation_id: uuid::Uuid::new_v4().to_string(),
            };
            let mut state = State::new(
                context.publisher.snapshot().state.state_epoch,
                identity.round_id.clone(),
                mode,
                frozen.dice_mode,
                scene.dice_disabled,
                forced,
            )
            .step(Event::Begin)?
            .state;
            let mut next = context.publisher.snapshot().state;
            reserve_round(&context.publisher.snapshot())?;
            next.phase = Phase::Generating;
            next.resume_required = false;
            next.checkpoint = None;
            next.check = None;
            next.in_flight = Some(in_flight(&identity, None));
            next.last_operation = Some(Operation {
                operation_id: identity.operation_id.clone(),
                round_id: Some(identity.round_id.clone()),
                outcome: OperationOutcome::Accepted,
                error: None,
            });
            let prepared = context.publisher.prepare(next, None)?;
            let mut updated = recovered.clone();
            if let Some(checkpoint) = &recovered.checkpoint {
                let body_fact = Fact::AbandonCheckpoint {
                    identity: identity.clone(),
                    source_round_id: checkpoint.source_round_id.clone(),
                    checkpoint_seq: checkpoint.through_seq,
                };
                let body = fact_body(&body_fact)?;
                let seq = session.append(body.clone())?;
                recovery::apply_record(&mut updated, seq, body)?;
            }
            let speech = Body::PlayerSpeech {
                player_id: context.player_id.clone(),
                text: raw,
                mode: Some(mode),
                content_range: Some(content_range),
            };
            let input_seq = session.append(speech)?;
            let body_fact = Fact::RoundAccepted {
                identity: identity.clone(),
                input_seq,
                mode,
                dice_mode: frozen.dice_mode,
                profile_id: frozen.profile_id.clone(),
                profile_revision: frozen.profile_revision.clone(),
                target: target.clone(),
                scene_id: position.scene_id.clone(),
                presence_revision: expected_presence_revision,
                source_round_id: None,
                checkpoint_seq: None,
            };
            let body = fact_body(&body_fact)?;
            let seq = session.append(body.clone())?;
            recovery::apply_record(&mut updated, seq, body)?;
            state = complete_effect(&state, Receipt::Input(input_seq))?;
            // 接纳已经耐久确认，仍返回其身份；actor 在下一步封口，禁止启动请求。
            let delivery_error = context.publisher.confirm_committed(prepared)?;
            drop(domain);
            drop(session);
            Ok(Self {
                context,
                lease,
                frozen,
                state,
                recovered: updated,
                target,
                selected: None,
                public_turn: None,
                delivery_error,
                child_outcome: None,
                identity,
            })
        })
        .await
    }

    pub(super) fn has_delivery_error(&self) -> bool {
        self.delivery_error.is_some()
    }

    pub fn stage(&self) -> Stage {
        self.state.stage
    }
    pub fn round_id(&self) -> &str {
        &self.identity.round_id
    }
    pub fn plan_id(&self) -> Option<&str> {
        self.state.plan.as_ref().map(|plan| plan.plan_id.as_str())
    }
    pub fn turn_id(&self) -> Option<&str> {
        self.public_turn.as_deref()
    }
    pub(super) fn lease(&self) -> Lease {
        self.lease.clone()
    }

    /// 显式恢复创建新回合，引用当前有效检查点的真正所有者；不自动发请求。
    /// # Errors
    /// 没有确认检查点、pending 或场景失配拒绝，沿用当前设置冻结值。
    pub async fn resume(
        context: Arc<Context>,
        lease: Lease,
        frozen: FrozenRound,
    ) -> Result<Self, Fault> {
        Self::resume_as(
            context,
            lease,
            frozen,
            RoundIdentity {
                version: 1,
                round_id: uuid::Uuid::new_v4().to_string(),
                operation_id: uuid::Uuid::new_v4().to_string(),
            },
        )
        .await
    }
    pub(super) async fn resume_as(
        context: Arc<Context>,
        lease: Lease,
        frozen: FrozenRound,
        identity: RoundIdentity,
    ) -> Result<Self, Fault> {
        crate::blocking::run(move || {
            let mut session = lock(&context.session);
            let domain = lock(&context.domain);
            let mut recovered = recovery::derive(&session, domain.baseline_scene())?;
            let checkpoint = recovered
                .checkpoint
                .clone()
                .ok_or_else(super::invalid_phase)?;
            let round = recovered.round.as_ref().ok_or_else(super::invalid_phase)?;
            let target = round.target.clone();
            let presence_revision = round.presence_revision.clone();
            let state = State::resume(
                context.publisher.snapshot().state.state_epoch,
                identity.round_id.clone(),
                &recovered,
                frozen.dice_mode,
            );
            let state = state?;
            let scene_id = round.scene_id.clone();
            domain.catalog().scene(&scene_id)?;
            let body_fact = Fact::RoundAccepted {
                identity: identity.clone(),
                input_seq: round.input_seq,
                mode: round.mode,
                dice_mode: frozen.dice_mode,
                profile_id: frozen.profile_id.clone(),
                profile_revision: frozen.profile_revision.clone(),
                target: target.clone(),
                scene_id,
                presence_revision,
                source_round_id: Some(checkpoint.source_round_id),
                checkpoint_seq: Some(checkpoint.through_seq),
            };
            let body = fact_body(&body_fact)?;
            let mut next = context.publisher.snapshot().state;
            reserve_round(&context.publisher.snapshot())?;
            next.phase = state.stage.phase(state.checked);
            next.resume_required = false;
            next.checkpoint = None;
            next.in_flight = Some(in_flight(&identity, None));
            next.last_operation = Some(Operation {
                operation_id: identity.operation_id.clone(),
                round_id: Some(identity.round_id.clone()),
                outcome: OperationOutcome::Accepted,
                error: None,
            });
            next.check = check_summary(&state);
            let prepared = context.publisher.prepare(next, None)?;
            let seq = session.append(body.clone())?;
            recovery::apply_record(&mut recovered, seq, body)?;
            let delivery_error = context.publisher.confirm_committed(prepared)?;
            drop(domain);
            drop(session);
            Ok(Self {
                context,
                lease,
                frozen,
                state,
                recovered,
                target,
                selected: None,
                public_turn: None,
                delivery_error,
                child_outcome: None,
                identity,
            })
        })
        .await
    }

    /// 子调用退出并结束已开始的提交后封口；不撤销已 applied 的世界或安全前文。
    /// # Errors
    /// 存储不可确认时保持恢复需求，不能伪造终态记录。
    pub async fn finish(&mut self, outcome: Outcome, error: Option<Fault>) -> Result<(), Fault> {
        if outcome == Outcome::Completed || (outcome == Outcome::Failed) != error.is_some() {
            return Err(Fault::bad_request());
        }
        let context = self.context.clone();
        let lease = self.lease.clone();
        let identity = self.identity.clone();
        let steps = completed_steps(&self.state);
        crate::blocking::run(move || {
            let _lease = lease;
            let mut session = lock(&context.session);
            let domain = lock(&context.domain);
            let mut recovered = recovery::derive(&session, domain.baseline_scene())?;
            let round = recovered.round.as_ref().ok_or_else(super::invalid_phase)?;
            let through_seq = round
                .advance
                .or(round.settled)
                .or(round.settlement)
                .or(round.narrative)
                .or(round.check)
                .or(round.dice)
                .or_else(|| round.plan.as_ref().map(|(seq, _)| *seq))
                .or(round.skipped)
                .unwrap_or(round.accepted_seq);
            let body_fact = Fact::RoundEnded {
                identity: identity.clone(),
                outcome,
                through_seq,
                completed_steps: steps,
                error: error.clone(),
            };
            let body = fact_body(&body_fact)?;
            recovery::apply_record(&mut recovered, session.next_sequence(), body.clone())?;
            recovery::project(&mut recovered, session.history_revision());
            let mut next = context.publisher.snapshot().state;
            next.phase = recovered.phase;
            next.in_flight = None;
            next.check = paused_check(&recovered);
            next.scene = recovered.scene;
            next.needs_recovery = recovered.needs_recovery;
            next.resume_required = recovered.checkpoint.is_some();
            next.checkpoint = recovered.checkpoint;
            let prepared = context.publisher.prepare(
                next,
                Some(Finished {
                    operation_id: identity.operation_id,
                    outcome,
                    error,
                }),
            )?;
            session.append(body)?;
            // 终态已经确认，窗口故障只影响通知；快照和幂等结果保留。
            context.publisher.confirm_committed(prepared).map(|_| ())
        })
        .await
    }

    /// waiting → rolling 的标记先确认，之后才允许获取系统熵。
    /// # Errors
    /// 身份不符、过期计划或准备失败拒绝；rolling 中的同计划重试不重掷。
    pub fn submit_check(&mut self, plan: &str) -> Result<(), Fault> {
        if self.plan_id() != Some(plan) {
            return Err(Fault::bad_request());
        }
        let decision = self.state.step(Event::SubmitCheck)?;
        if !decision.ignored {
            let prepared = self
                .context
                .publisher
                .prepare(self.phase_state(&decision.state), None)?;
            self.delivery_error = self.context.publisher.confirm_committed(prepared)?;
            self.state = decision.state;
        }
        Ok(())
    }

    fn phase_state(&self, state: &State) -> super::state::PhaseState {
        let mut next = self.context.publisher.snapshot().state;
        next.phase = state.stage.phase(state.checked);
        next.scene = self.recovered.scene.clone();
        next.resume_required = false;
        next.checkpoint = None;
        next.check = check_summary(state);
        next.in_flight = Some(in_flight(&self.identity, self.public_turn.clone()));
        next
    }
    fn publish(&self, state: &State, finished: Option<Finished>) -> Result<(), Fault> {
        let prepared = self
            .context
            .publisher
            .prepare(self.phase_state(state), finished)?;
        self.context.publisher.confirm(prepared)
    }

    /// 当前副作用的固定回执；调用方必须等待，不能因收到取消命令丢弃 Future。
    /// # Errors
    /// 任一步骤失败保留已确认事实，由 actor 决定失败封口和暂停恢复。
    pub async fn step(&mut self, cancel: CancellationToken) -> Result<(), Fault> {
        self.child_outcome = None;
        if let Some(error) = self.delivery_error.take() {
            return Err(error);
        }
        ensure_running(&cancel)?;
        let action = self
            .state
            .current
            .as_ref()
            .ok_or_else(super::invalid_phase)?
            .action;
        match action {
            Action::ProposeCheck => self.propose_check(cancel).await,
            Action::StartNarration => self.narrate(cancel).await,
            Action::SelectScene => self.select_scene(cancel).await,
            Action::CommitInput => Err(super::invalid_phase()),
            _ => self.commit(action).await,
        }
    }
    async fn request(
        &self,
        target: PromptTarget,
        extra: String,
    ) -> Result<PreparedGeneration, Fault> {
        let context = self.context.clone();
        let generation = self.frozen.generation.clone();
        crate::blocking::run(move || {
            let session = lock(&context.session);
            let domain = lock(&context.domain);
            let view = domain.confirmed(&session)?;
            let mut world = domain.context(&view)?;
            world.push_str(&extra);
            let working = session.projection_working_set()?;
            let plan = projection::project_request(
                &working.view(),
                &WorldView {
                    context: &world,
                    needs_recovery: false,
                },
                &generation.projection_budget(),
                &target,
                generation.shape(),
            )
            .map_err(projection_error)?;
            let request = generation.prepare(plan.input, plan.guard, true)?;
            if target.proposal() {
                request.with_private_limit(super::proposal::MAX_PROPOSAL_BYTES)
            } else {
                Ok(request)
            }
        })
        .await
    }
    pub(super) fn child_outcome(&self) -> Option<Outcome> {
        self.child_outcome
    }

    async fn private(
        &mut self,
        request: PreparedGeneration,
        cancel: CancellationToken,
    ) -> Result<crate::turn::PrivateResult, Fault> {
        let result = self
            .context
            .coordinator
            .run_private_cancellable(&self.lease, request, cancel)
            .await;
        self.child_outcome = Some(match &result {
            Ok(result) => result.outcome,
            Err(_) => Outcome::Failed,
        });
        result
    }

    async fn propose_check(&mut self, cancel: CancellationToken) -> Result<(), Fault> {
        let extra = {
            let domain = lock(&self.context.domain);
            let scene = domain
                .catalog()
                .scene(&self.recovered.scene.as_ref().ok_or_else(no_scene)?.scene_id)?;
            format!(
                "\n{CHECK_PROPOSAL_GUIDANCE}\n仅返回 JSON：{{\"kind\":\"noCheck\"}} 或 {{\"kind\":\"check\",\"ruleId\":\"登记规则\",\"actorId\":\"登记角色\",\"reason\":\"理由\"}}。规则 {:?}；角色 {:?}。",
                scene.check_rule_ids, scene.actor_ids
            )
        };
        let request = self.request(PromptTarget::CheckProposal, extra).await?;
        ensure_running(&cancel)?;
        let result = self.private(request, cancel).await?;
        let receipt = match super::proposal::check(&result)? {
            super::proposal::CheckProposal::NoCheck {} => Receipt::NoCheck,
            super::proposal::CheckProposal::Check {
                rule_id, actor_id, ..
            } => {
                let session = lock(&self.context.session);
                let domain = lock(&self.context.domain);
                let scene = domain
                    .catalog()
                    .scene(&self.recovered.scene.as_ref().ok_or_else(no_scene)?.scene_id)?;
                if !scene.check_rule_ids.contains(&rule_id) || !scene.actor_ids.contains(&actor_id)
                {
                    return Err(Fault::new(
                        "llm.bad-response",
                        "提议不在登记规则或角色范围内",
                    ));
                }
                let view = domain.confirmed(&session)?;
                let plan = domain.check_plan(&rule_id, &actor_id, &view)?;
                super::dice::validate_plan(&plan, view.world_revision())?;
                Receipt::Check(Box::new(plan))
            }
        };
        let next = complete_effect(&self.state, receipt)?;
        self.publish(&next, None)?;
        self.state = next;
        Ok(())
    }
    async fn narrate(&mut self, cancel: CancellationToken) -> Result<(), Fault> {
        let extra = if self.state.mode == InputMode::OutOfCharacter {
            "\n本轮为场外问答，仅回答问题，不推进世界或场景。".into()
        } else {
            String::new()
        };
        let prompt_target = prompt_target(&self.target);
        let request = self.request(prompt_target, extra).await?;
        ensure_running(&cancel)?;
        let delivery_error = Arc::new(Mutex::new(None));
        let writer = Arc::new(GameWriter {
            delivery_error: delivery_error.clone(),
            context: self.context.clone(),
            lease: self.lease.clone(),
            persistent: PersistentWriter {
                session: self.context.session.clone(),
                target: record_target(&self.target),
                high: false,
            },
        });
        let call = self
            .context
            .coordinator
            .prepare_borrowed(&self.lease, request, writer)?;
        self.public_turn = Some(call.id().into());
        self.publish(&self.state, None)?;
        let id = call.id().to_owned();
        call.start();
        let wait = self.context.coordinator.wait(&id);
        tokio::pin!(wait);
        let snapshot = tokio::select! {
            result = &mut wait => result?,
            () = cancel.cancelled() => {
                self.context.coordinator.cancel(&id).await?;
                wait.await?
            }
        };
        self.child_outcome = snapshot.outcome;
        match snapshot.outcome {
            Some(Outcome::Completed) => {
                let session = lock(&self.context.session);
                let seq = session.next_sequence() - 1;
                let body = session
                    .read(seq)
                    .map_err(store_fault)?
                    .body
                    .ok_or_else(super::invalid_phase)?
                    .body;
                recovery::apply_record(&mut self.recovered, seq, body)?;
                drop(session);
                self.state = complete_effect(&self.state, Receipt::Narrative(seq))?;
                self.public_turn = None;
                match lock(&delivery_error).take() {
                    Some(error) => Err(error),
                    None => Ok(()),
                }
            }
            _ => Err(snapshot
                .error
                .unwrap_or_else(|| Fault::new("engine.interrupted", "生成已中断，确认前文保留"))),
        }
    }
    async fn select_scene(&mut self, cancel: CancellationToken) -> Result<(), Fault> {
        let (candidates, revision, exit) = {
            let session = lock(&self.context.session);
            let domain = lock(&self.context.domain);
            let view = domain.confirmed(&session)?;
            let scene = &self.recovered.scene.as_ref().ok_or_else(no_scene)?.scene_id;
            if self.state.mode == InputMode::OutOfCharacter {
                (Vec::new(), view.world_revision().to_owned(), None)
            } else {
                let exit = domain.exit_reason(scene, &view)?;
                let candidates = if exit.is_some() {
                    Vec::new()
                } else {
                    domain.catalog().candidates(scene, &view)?.0
                };
                (candidates, view.world_revision().to_owned(), exit)
            }
        };
        let previous_scene_id = self
            .recovered
            .scene
            .as_ref()
            .ok_or_else(no_scene)?
            .scene_id
            .clone();
        let selected = if let Some(reason_id) = exit {
            Fact::SessionEnded {
                identity: self.identity.clone(),
                previous_scene_id,
                reason_id,
                world_revision: revision,
                mutation_id: uuid::Uuid::new_v4().to_string(),
            }
        } else if candidates.is_empty() {
            Fact::SceneStayed {
                identity: self.identity.clone(),
                scene_id: previous_scene_id,
                world_revision: revision,
            }
        } else {
            let request = self.request(PromptTarget::SceneProposal, format!(
                "\n仅返回 JSON：{{\"kind\":\"stay\"}} 或 {{\"kind\":\"scene\",\"sceneId\":\"候选\"}}。候选 {:?}。", candidates
            )).await?;
            ensure_running(&cancel)?;
            let result = self.private(request, cancel).await?;
            match super::proposal::scene(&result)? {
                super::proposal::SceneProposal::Stay {} => Fact::SceneStayed {
                    identity: self.identity.clone(),
                    scene_id: previous_scene_id,
                    world_revision: revision,
                },
                super::proposal::SceneProposal::Scene { scene_id } => {
                    if !candidates.contains(&scene_id) {
                        return Err(Fault::new("llm.bad-response", "场景提议不在确认候选内"));
                    }
                    let session = lock(&self.context.session);
                    let domain = lock(&self.context.domain);
                    let view = domain.confirmed(&session)?;
                    if view.world_revision() != revision {
                        return Err(super::invalid_phase());
                    }
                    Fact::SceneAdvanced {
                        identity: self.identity.clone(),
                        previous_scene_id,
                        position: domain.catalog().scene(&scene_id)?.position.clone(),
                        world_revision: revision,
                        mutation_id: uuid::Uuid::new_v4().to_string(),
                    }
                }
            }
        };
        let next = complete_effect(&self.state, Receipt::SceneSelected)?;
        self.publish(&next, None)?;
        self.selected = Some(selected);
        self.state = next;
        Ok(())
    }

    async fn commit(&mut self, action: Action) -> Result<(), Fault> {
        let context = self.context.clone();
        let lease = self.lease.clone();
        let state = self.state.clone();
        let mut recovered = self.recovered.clone();
        let identity = self.identity.clone();
        let selected = self.selected.clone();
        let public_turn = self.public_turn.clone();
        let (next, updated, delivered) = crate::blocking::run(move || {
            let _lease = lease;
            let mut session = lock(&context.session);
            let mut domain = lock(&context.domain);
            let view = domain.confirmed(&session)?;
            let seq = session.next_sequence();
            let round = recovered.round.as_ref().ok_or_else(super::invalid_phase)?;
            let (body, receipt, mode) = match action {
                Action::CommitPlan => {
                    let fact = Fact::CheckPlanned {
                        identity: identity.clone(),
                        dice_mode: state.dice_mode,
                        plan: state.plan.clone().ok_or_else(super::invalid_phase)?,
                    };
                    (fact_body(&fact)?, Receipt::Plan(seq), CommitMode::Record)
                }
                Action::CommitSkip => {
                    let reason = if state.mode == InputMode::OutOfCharacter {
                        SkipReason::OutOfCharacter
                    } else if state.dice_disabled {
                        SkipReason::Disabled
                    } else {
                        SkipReason::NoCheck
                    };
                    let fact = Fact::CheckSkipped {
                        identity: identity.clone(),
                        reason,
                        world_revision: view.world_revision().into(),
                    };
                    (fact_body(&fact)?, Receipt::Skip(seq), CommitMode::Record)
                }
                Action::RollDice => {
                    let plan = state.plan.as_ref().ok_or_else(super::invalid_phase)?;
                    let seed = super::dice::fresh_seed()?;
                    let body = super::dice::roll(plan, view.world_revision(), seed)?;
                    (body, Receipt::Dice(seq), CommitMode::Record)
                }
                Action::CommitCheck => {
                    let plan = state.plan.as_ref().ok_or_else(super::invalid_phase)?;
                    let dice_seq = state.dice_seq.ok_or_else(super::invalid_phase)?;
                    let Body::Dice { total, plan_id, .. } = session
                        .read(dice_seq)
                        .map_err(store_fault)?
                        .body
                        .ok_or_else(super::invalid_phase)?
                        .body
                    else {
                        return Err(super::invalid_phase());
                    };
                    if plan_id != plan.plan_id {
                        return Err(super::invalid_phase());
                    }
                    (
                        Body::Check {
                            dice_seq,
                            dc: None,
                            result: super::dice::classify(total),
                            rule_id: plan.rule_id.clone(),
                            plan_id,
                        },
                        Receipt::CheckResult(seq),
                        CommitMode::Record,
                    )
                }
                Action::ApplySettlement => {
                    if let Some(settlement_seq) = round.settlement {
                        let Body::System { code, data, .. } = session
                            .read(settlement_seq)
                            .map_err(store_fault)?
                            .body
                            .ok_or_else(super::invalid_phase)?
                            .body
                        else {
                            return Err(super::invalid_phase());
                        };
                        let Fact::SettlementPlanned { mutations, .. } =
                            Fact::decode(&code, &data).map_err(store_fault)?
                        else {
                            return Err(super::invalid_phase());
                        };
                        let fact = Fact::RoundSettled {
                            identity: identity.clone(),
                            narrative_seq: state.narrative_seq.ok_or_else(super::invalid_phase)?,
                            settlement_seq,
                            applied_through_mutation_id: mutations
                                .last()
                                .map(|mutation| mutation.mutation_id.clone()),
                        };
                        (
                            fact_body(&fact)?,
                            Receipt::Settlement(seq),
                            CommitMode::Record,
                        )
                    } else {
                        let mutations = if state.mode == InputMode::OutOfCharacter {
                            Vec::new()
                        } else {
                            domain.consequences(&session, round, &view)?
                        };
                        let narrative_seq = state.narrative_seq.ok_or_else(super::invalid_phase)?;
                        let applied_through_mutation_id = mutations
                            .last()
                            .map(|mutation| mutation.mutation_id.clone());
                        let fact = Fact::SettlementPlanned {
                            identity: identity.clone(),
                            narrative_seq,
                            world_revision: view.world_revision().into(),
                            mutations,
                        };
                        (
                            fact_body(&fact)?,
                            Receipt::Settlement(seq + 1),
                            CommitMode::Settlement {
                                narrative_seq,
                                applied_through_mutation_id,
                            },
                        )
                    }
                }
                Action::CommitAdvance => {
                    let fact = selected.as_ref().ok_or_else(super::invalid_phase)?;
                    let world =
                        matches!(fact, Fact::SceneAdvanced { .. } | Fact::SessionEnded { .. });
                    (
                        fact_body(fact)?,
                        Receipt::Advance(seq),
                        if world {
                            CommitMode::World
                        } else {
                            CommitMode::Record
                        },
                    )
                }
                Action::EndRound => {
                    let fact = Fact::RoundEnded {
                        identity: identity.clone(),
                        outcome: Outcome::Completed,
                        through_seq: state.advance_seq.ok_or_else(super::invalid_phase)?,
                        completed_steps: completed_steps(&state),
                        error: None,
                    };
                    (fact_body(&fact)?, Receipt::Ended, CommitMode::Record)
                }
                _ => return Err(super::invalid_phase()),
            };
            let next = complete_effect(&state, receipt)?;
            let mut phase = context.publisher.snapshot().state;
            phase.phase = next.stage.phase(next.checked);
            phase.in_flight = Some(in_flight(&identity, public_turn));
            phase.check = check_summary(&next);
            if action == Action::CommitAdvance {
                match &selected {
                    Some(Fact::SceneAdvanced { position, .. }) => {
                        phase.scene = Some(position.clone())
                    }
                    Some(Fact::SessionEnded { .. }) => phase.scene = None,
                    _ => {}
                }
            }
            let finished = if action == Action::EndRound {
                phase.in_flight = None;
                Some(Finished {
                    operation_id: identity.operation_id.clone(),
                    outcome: Outcome::Completed,
                    error: None,
                })
            } else {
                None
            };
            let prepared = context.publisher.prepare(phase, finished)?;
            let cycle_event = if let Some(run_id) = context.completed.run_id() {
                let session_id = session.header().session_id.clone();
                let history_revision = session.history_revision();
                match action {
                    Action::CommitAdvance => match selected.as_ref() {
                        Some(Fact::SceneAdvanced { position, .. }) => {
                            let evidence = super::cycle::current_progress_evidence(
                                domain.catalog(),
                                &position.scene_id,
                                &view,
                            )?;
                            Some(CycleLifecycleEvent {
                                version: 1,
                                event_id: CycleLifecycleEvent::event_id_for(
                                    &run_id,
                                    CycleEventKind::SceneEntered,
                                    history_revision,
                                    seq,
                                ),
                                run_id,
                                session_id,
                                kind: CycleEventKind::SceneEntered,
                                source_record_seq: seq,
                                history_revision,
                                scene_id: position.scene_id.clone(),
                                entry_record_seq: seq,
                                catalog_revision: domain.catalog().revision().to_owned(),
                                world_revision: view.world_revision().to_owned(),
                                round_id: None,
                                evidence,
                            })
                        }
                        _ => None,
                    },
                    Action::EndRound if round.mode == InputMode::InCharacter => {
                        let entry_record_seq = crate::record::memory::scene_entry_seq_before(
                            &session,
                            domain.catalog(),
                            &round.scene_id,
                            round.accepted_seq,
                        )
                        .map_err(|error| Fault::new(error.code(), "记忆场景来源不可用"))?;
                        let evidence = super::cycle::current_progress_evidence(
                            domain.catalog(),
                            &round.scene_id,
                            &view,
                        )?;
                        Some(CycleLifecycleEvent {
                            version: 1,
                            event_id: CycleLifecycleEvent::event_id_for(
                                &run_id,
                                CycleEventKind::RoundCompleted,
                                history_revision,
                                seq,
                            ),
                            run_id,
                            session_id,
                            kind: CycleEventKind::RoundCompleted,
                            source_record_seq: seq,
                            history_revision,
                            scene_id: round.scene_id.clone(),
                            entry_record_seq,
                            catalog_revision: domain.catalog().revision().to_owned(),
                            world_revision: view.world_revision().to_owned(),
                            round_id: Some(round.identity.round_id.clone()),
                            evidence,
                        })
                    }
                    _ => None,
                }
            } else {
                None
            };
            if let Some(event) = cycle_event {
                context.completed.prepare_cycle_event(event)?;
            }
            if !matches!(mode, CommitMode::Record) {
                session.commit_world(body.clone(), domain.as_mut())?;
            } else {
                session.append(body.clone())?;
            }
            recovery::apply_record(&mut recovered, seq, body.clone())?;
            if let CommitMode::Settlement {
                narrative_seq,
                applied_through_mutation_id,
            } = mode
            {
                let settled_fact = Fact::RoundSettled {
                    identity: identity.clone(),
                    narrative_seq,
                    settlement_seq: seq,
                    applied_through_mutation_id,
                };
                let settled = fact_body(&settled_fact)?;
                session.append(settled.clone())?;
                recovery::apply_record(&mut recovered, seq + 1, settled)?;
            }
            let delivered = context.publisher.confirm(prepared);
            if action == Action::EndRound {
                let completed = CompletedRound {
                    session_id: session.header().session_id.clone(),
                    round_id: identity.round_id,
                    history_revision: session.history_revision(),
                    through_seq: seq,
                };
                drop(domain);
                drop(session);
                context.completed.completed(completed);
            } else if matches!(action, Action::CommitAdvance)
                && selected
                    .as_ref()
                    .is_some_and(|fact| matches!(fact, Fact::SceneAdvanced { .. }))
            {
                drop(domain);
                drop(session);
                // 已提交的场景事实不能因派生记忆暂不可写而回报为失败。
                let _ = context.completed.drain_cycle_events();
            }
            Ok((next, recovered, delivered))
        })
        .await?;
        self.state = next;
        self.recovered = updated;
        delivered
    }
}

fn validate_generation_target(target: &GenerationTarget, actor_id: &str) -> Result<(), Fault> {
    let GenerationTarget::CharacterSpeech { speaker_id } = target else {
        return Ok(());
    };
    if !crate::record::format::valid_id(speaker_id) || speaker_id == actor_id {
        return Err(Fault::bad_request());
    }
    Ok(())
}

fn prompt_target(target: &GenerationTarget) -> PromptTarget {
    match target {
        GenerationTarget::Narration => PromptTarget::Narration,
        GenerationTarget::CharacterSpeech { speaker_id } => {
            PromptTarget::Character(speaker_id.clone())
        }
    }
}

fn record_target(target: &GenerationTarget) -> Target {
    match target {
        GenerationTarget::Narration => Target::Narration,
        GenerationTarget::CharacterSpeech { speaker_id } => Target::Character(speaker_id.clone()),
    }
}

fn in_flight(identity: &RoundIdentity, turn_id: Option<String>) -> InFlight {
    InFlight {
        operation_id: identity.operation_id.clone(),
        round_id: Some(identity.round_id.clone()),
        turn_id,
    }
}

fn complete_effect(state: &State, receipt: Receipt) -> Result<State, Fault> {
    let effect = state.current.as_ref().ok_or_else(super::invalid_phase)?;
    let event = Event::Completed {
        identity: effect.identity.clone(),
        receipt,
    };
    let decision = state.step(event)?;
    Ok(decision.state)
}

pub(super) fn paused_check(recovered: &Recovery) -> Option<CheckSummary> {
    recovered
        .round
        .as_ref()
        .filter(|_| recovered.phase == Phase::AwaitingCheck)
        .and_then(|round| {
            round
                .plan
                .as_ref()
                .map(|(_, plan)| summarize(plan, round.dice_mode, CheckStatus::Waiting))
        })
}

fn projection_error(error: projection::ProjectionError) -> Fault {
    error.fault()
}

fn completed_steps(state: &State) -> Vec<Step> {
    let mut steps = vec![Step::Proposal];
    if state.check_seq.is_some() {
        steps.push(Step::Check);
    }
    if state.narrative_seq.is_some() {
        steps.push(Step::Narration);
    }
    if state.settlement_seq.is_some() {
        steps.push(Step::Settlement);
    }
    if state.advance_seq.is_some() {
        steps.push(Step::Advance);
    }
    steps
}
fn check_summary(state: &State) -> Option<CheckSummary> {
    if !matches!(state.stage, Stage::Waiting | Stage::Rolling) {
        return None;
    }
    state.plan.as_ref().map(|plan| {
        summarize(
            plan,
            state.dice_mode,
            if state.stage == Stage::Waiting {
                CheckStatus::Waiting
            } else {
                CheckStatus::Rolling
            },
        )
    })
}
fn summarize(plan: &CheckPlan, mode: DiceMode, status: CheckStatus) -> CheckSummary {
    CheckSummary {
        plan_id: plan.plan_id.clone(),
        status,
        mode,
        rule_id: plan.rule_id.clone(),
        actor_id: plan.actor_id.clone(),
        expression: plan.expression.clone(),
        modifier_total: plan.modifiers.iter().map(|m| m.value).sum(),
    }
}
fn no_scene() -> Fault {
    Fault::new("engine.no-scene", "当前会话没有可推进场景")
}
fn stale_presence() -> Fault {
    Fault::bad_request().with_detail(serde_json::json!({ "reason": "stalePresenceRevision" }))
}
fn target_unavailable() -> Fault {
    Fault::bad_request().with_detail(serde_json::json!({ "reason": "targetUnavailable" }))
}
fn ensure_running(cancel: &CancellationToken) -> Result<(), Fault> {
    if cancel.is_cancelled() {
        Err(Fault::new(
            "engine.interrupted",
            "回合已取消，未启动新的生成请求",
        ))
    } else {
        Ok(())
    }
}
fn reserve_round(snapshot: &super::state::PhaseSnapshot) -> Result<(), Fault> {
    // 同一回合最多三个逻辑调用及固定提交阶段，预留失败封口的修订空间。
    let limit = crate::record::format::MAX_SEQ - 32;
    if snapshot.state.phase_revision > limit
        || [
            snapshot.seq.phase_changed,
            snapshot.seq.scene_advanced,
            snapshot.seq.operation_done,
            snapshot.seq.operation_failed,
        ]
        .into_iter()
        .any(|seq| seq > limit)
    {
        return Err(super::invalid_phase());
    }
    Ok(())
}

/// 封口关联阶段先准备，再耐久写入；与 actor 的下一转换之间没有第二个写者。
struct GameWriter {
    delivery_error: Arc<Mutex<Option<Fault>>>,
    context: Arc<Context>,
    lease: Lease,
    persistent: PersistentWriter,
}
impl OutputWriter for GameWriter {
    fn begin<'a>(&'a self, turn: &'a str) -> BoxFuture<'a, Result<(), Fault>> {
        self.persistent.begin(turn)
    }
    fn append<'a>(
        &'a self,
        turn: &'a str,
        seq: u64,
        text: &'a str,
    ) -> BoxFuture<'a, Result<(), Fault>> {
        self.persistent.append(turn, seq, text)
    }
    fn finish<'a>(
        &'a self,
        turn: &'a str,
        terminal: &'a Terminal,
    ) -> BoxFuture<'a, Result<(), Fault>> {
        let context = self.context.clone();
        let lease = self.lease.clone();
        let delivery_error = self.delivery_error.clone();
        let turn = turn.to_owned();
        let terminal = terminal.clone();
        let target = self.persistent.target.clone();
        Box::pin(async move {
            crate::blocking::run(move || {
                let _lease = lease;
                let mut session = lock(&context.session);
                let mut next = context.publisher.snapshot().state;
                if terminal.outcome() == Outcome::Completed {
                    next.phase = Phase::Settling;
                }
                let prepared = context.publisher.prepare(next, None)?;
                session.seal(&turn, &target, &terminal)?;
                // 正文已确认，通知故障单独交给驱动，不改写 child 的真实终态。
                *lock(&delivery_error) = context.publisher.confirm_committed(prepared)?;
                Ok(())
            })
            .await
        })
    }
}

#[cfg(test)]
mod tests;
