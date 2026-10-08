//! 阶段机与 actor 共用的无收费、真实记录夹具。

use super::execution::test_lock as lock;
use super::execution::*;
use super::{
    domain::{CompletedRound, CompletedRounds, Domain, FrozenRound},
    publication::Publisher,
    recovery,
    reducer::Stage,
};
use crate::{
    fault::Fault, record::session::Session, request::PreparedGeneration, turn::Coordinator,
};
use crate::{
    game::{catalog::*, domain::Generation, state::*},
    record::{
        facts::{CheckPlan, DiceMode, SceneNode, ScenePosition, WorldMutation},
        history::HistoryPort,
        projection::{Budget, Shape},
        test_support::{Events as RecordEvents, header},
        world::WorldPort,
    },
};
use mythos_llm::{
    guard::GuardSpec,
    provider::{ProviderDelta, ProviderFinish, ProviderInput},
};
use std::sync::{Arc, Mutex};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(crate) struct PrepareGate {
    pub(crate) entered: CancellationToken,
    released: Mutex<bool>,
    changed: std::sync::Condvar,
}
impl PrepareGate {
    pub(crate) fn block(&self, timeout: std::time::Duration) -> Result<(), Fault> {
        self.entered.cancel();
        let released = lock(&self.released);
        let (released, _) = self
            .changed
            .wait_timeout_while(released, timeout, |released| !*released)
            .unwrap();
        if *released {
            Ok(())
        } else {
            Err(Fault::event())
        }
    }
    pub(crate) fn release(&self) {
        *lock(&self.released) = true;
        self.changed.notify_all();
    }
}

#[derive(Default)]
pub(crate) struct PhaseEventsFixture {
    pub(crate) prepare_gate: Mutex<Option<Arc<PrepareGate>>>,
    pub(crate) seal_prepare_gate: Mutex<Option<Arc<PrepareGate>>>,
    pub(crate) ending_prepare_gate: Mutex<Option<Arc<PrepareGate>>>,
    pub(crate) waiting_delivery_gate: Mutex<Option<Arc<PrepareGate>>>,
    pub(crate) fail: std::sync::atomic::AtomicBool,
    pub(crate) fail_prepare: std::sync::atomic::AtomicBool,
    pub(crate) fail_done: std::sync::atomic::AtomicBool,
    pub(crate) fail_after_cancelled: std::sync::atomic::AtomicBool,
    pub(crate) fail_public: std::sync::atomic::AtomicBool,
    pub(crate) fail_settling: std::sync::atomic::AtomicBool,
    pub(crate) fail_rolling: std::sync::atomic::AtomicBool,
    seq: Mutex<BTreeMap<String, u64>>,
    sent: Mutex<Vec<PhaseEvent>>,
}
impl PhaseEvents for PhaseEventsFixture {
    fn prepare(&self, event: &PhaseEvent) -> Result<u64, Fault> {
        if event
            .state()
            .last_operation
            .as_ref()
            .is_some_and(|operation| operation.outcome == OperationOutcome::Completed)
        {
            let gate = lock(&self.ending_prepare_gate).take();
            if let Some(gate) = gate {
                gate.block(std::time::Duration::from_secs(5))?;
            }
        }
        if event.state().phase == Phase::Settling
            && event
                .state()
                .in_flight
                .as_ref()
                .is_some_and(|flight| flight.turn_id.is_some())
        {
            let gate = lock(&self.seal_prepare_gate).take();
            if let Some(gate) = gate {
                gate.block(std::time::Duration::from_secs(5))?;
            }
        }
        let gate = lock(&self.prepare_gate).take();
        if let Some(gate) = gate {
            gate.block(std::time::Duration::from_secs(5))?;
        }
        if self.fail_prepare.load(Ordering::SeqCst) {
            return Err(Fault::event());
        }
        let mut sequences = lock(&self.seq);
        let seq = sequences.entry(event.name().into()).or_default();
        *seq += 1;
        Ok(*seq)
    }
    fn deliver(&self, _: u64, event: PhaseEvent) -> Result<(), Fault> {
        if self.fail_after_cancelled.load(Ordering::SeqCst)
            && event
                .state()
                .last_operation
                .as_ref()
                .is_some_and(|operation| operation.outcome == OperationOutcome::Cancelled)
        {
            self.fail_prepare.store(true, Ordering::SeqCst);
        }
        if event.state().check.as_ref().is_some_and(|check| {
            matches!(check.status, CheckStatus::Waiting | CheckStatus::Rolling)
        }) {
            let gate = lock(&self.waiting_delivery_gate).take();
            if let Some(gate) = gate {
                gate.block(std::time::Duration::from_secs(5))?;
            }
        }
        if self.fail_done.load(Ordering::SeqCst) && event.name() == "engine:operation:done" {
            return Err(Fault::event());
        }
        if event
            .state()
            .check
            .as_ref()
            .is_some_and(|check| check.status == CheckStatus::Rolling)
            && self.fail_rolling.load(Ordering::SeqCst)
        {
            return Err(Fault::event());
        }
        if (self.fail_settling.load(Ordering::SeqCst)
            && event.state().phase == Phase::Settling
            && event
                .state()
                .in_flight
                .as_ref()
                .is_some_and(|flight| flight.turn_id.is_some()))
            || self.fail.load(Ordering::SeqCst)
            || (self.fail_public.load(Ordering::SeqCst)
                && event
                    .state()
                    .in_flight
                    .as_ref()
                    .is_some_and(|flight| flight.turn_id.is_some()))
        {
            return Err(Fault::new("app.event-failed", "夹具事件投递失败"));
        }
        lock(&self.sent).push(event);
        Ok(())
    }
    fn retire(&self, _: &str) {}
}
#[derive(Default)]
pub(crate) struct CompletedFixture(pub(crate) Mutex<Vec<CompletedRound>>);
impl CompletedRounds for CompletedFixture {
    fn completed(&self, round: CompletedRound) {
        lock(&self.0).push(round);
    }
}

pub(crate) struct TestGeneration {
    pub(crate) calls: AtomicUsize,
    pub(crate) hold: std::sync::atomic::AtomicBool,
    pub(crate) fail_auth: std::sync::atomic::AtomicBool,
    proposal: &'static str,
}
impl Generation for TestGeneration {
    fn shape(&self) -> Shape {
        Shape::Completion
    }
    fn projection_budget(&self) -> Budget {
        Budget {
            context_limit: Some(1_000_000),
            max_output_tokens: 64,
            ..Budget::default()
        }
    }
    fn prepare(
        &self,
        input: ProviderInput,
        guard: GuardSpec,
        _: bool,
    ) -> Result<PreparedGeneration, Fault> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let ProviderInput::Completion(ref prompt) = input else {
            return Err(Fault::bad_request());
        };
        let text = if prompt.prompt.ends_with("[MYTHOS:CHECK-PROPOSAL]\n")
            || prompt.prompt.ends_with("[MYTHOS:SCENE-PROPOSAL]\n")
        {
            self.proposal
        } else {
            "灯光照亮了房间。"
        };
        let hold = self.hold.load(Ordering::SeqCst);
        let mut deltas = if self.fail_auth.load(Ordering::SeqCst) {
            vec![Err(mythos_llm::error::ProviderError::Auth)]
        } else {
            vec![Ok(ProviderDelta::Text(text.into()))]
        };
        if !hold {
            deltas.push(Ok(ProviderDelta::Finish(ProviderFinish::Stop)));
        }
        let (mut generation, _) = crate::test_support::generation(deltas, hold);
        generation.request.provider_request.input = input;
        generation.request.guard = guard;
        Ok(generation)
    }
}
pub(crate) struct TestDomain {
    pub(crate) catalog: SceneCatalog,
    consequences: Arc<AtomicUsize>,
}
impl WorldPort for TestDomain {
    fn registered(&self, _: &str, _: u32) -> bool {
        false
    }
    fn applied(&self, _: &str, _: &str) -> Result<bool, Fault> {
        Ok(false)
    }
    fn apply(&mut self, _: &WorldMutation, _: &str) -> Result<(), Fault> {
        Err(Fault::new("engine.invalid-phase", "夹具未登记世界操作"))
    }
}
impl HistoryPort for TestDomain {
    fn verify_target(&self, session: &Session, target: u64) -> Result<(), Fault> {
        crate::game::control::verify_boundary(session, target)
    }
    fn verify_replay_target(
        &self,
        session: &Session,
        parent: u64,
        target: u64,
    ) -> Result<(), Fault> {
        crate::game::control::verify_replay_boundary(session, parent, target)
    }
    fn applied(&self, _: &str, _: &str) -> Result<bool, Fault> {
        Ok(false)
    }
    fn rebuild(&mut self, _: &Session, _: &[u64], _: &str, _: &str) -> Result<(), Fault> {
        Ok(())
    }
}
impl Domain for TestDomain {
    fn catalog(&self) -> &SceneCatalog {
        &self.catalog
    }
    fn baseline_scene(&self) -> Option<ScenePosition> {
        Some(self.catalog.scene("room").unwrap().position.clone())
    }
    fn confirmed(&self, session: &Session) -> Result<ConfirmedWorldView, Fault> {
        ConfirmedWorldView::capture(session, "0", BTreeMap::new())
    }
    fn context(&self, _: &ConfirmedWorldView) -> Result<String, Fault> {
        Ok("房间已确认".into())
    }
    fn check_plan(
        &self,
        rule: &str,
        actor: &str,
        view: &ConfirmedWorldView,
    ) -> Result<CheckPlan, Fault> {
        let mut plan = crate::game::dice::freeze_pbta(crate::game::dice::PlanSpec {
            rule_id: rule,
            actor_id: actor,
            modifiers: vec![],
            branches: ["success".into(), "costly".into(), "failure".into()],
            world_revision: view.world_revision(),
        })?;
        // 可信计划 ID 的契约是 valid_id；夹具验证它不被误收窄为 UUID。
        plan.plan_id = format!("check-{}", plan.plan_id);
        plan.plan_hash = crate::game::dice::plan_hash(&plan)?;
        Ok(plan)
    }
    fn consequences(
        &self,
        _: &Session,
        _: &recovery::RoundFacts,
        _: &ConfirmedWorldView,
    ) -> Result<Vec<WorldMutation>, Fault> {
        self.consequences.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }
    fn exit_reason(&self, _: &str, _: &ConfirmedWorldView) -> Result<Option<String>, Fault> {
        Ok(None)
    }
}
pub(crate) fn domain(
    script_revision: &str,
    forced: bool,
    consequences: Arc<AtomicUsize>,
) -> TestDomain {
    let position = ScenePosition {
        scene_id: "room".into(),
        path: vec![SceneNode {
            kind: "scene".into(),
            id: "room".into(),
            title: "房间".into(),
        }],
    };
    let catalog = SceneCatalog::new(
        "1".into(),
        script_revision.to_owned(),
        BTreeMap::from([(
            "room".into(),
            Node {
                kind: "scene".into(),
                title: "房间".into(),
                parent: None,
            },
        )]),
        BTreeMap::from([(
            "room".into(),
            Scene {
                position: position.clone(),
                rules: SceneRuleView {
                    scene_id: "room".into(),
                    catalog_revision: "1".into(),
                    advance_rule_ids: vec![],
                    progress_rule_ids: vec![],
                    hint_rules: vec![],
                    exit_rule_ids: vec![],
                },
                advance: vec![],
                check_rule_ids: vec!["search".into()],
                actor_ids: vec!["player".into()],
                forced_check: forced.then(|| "search".into()),
                dice_disabled: false,
            },
        )]),
        BTreeMap::new(),
    )
    .unwrap();
    TestDomain {
        catalog,
        consequences,
    }
}

pub(crate) struct Fixture {
    pub(crate) context: Arc<Context>,
    pub(crate) generation: Arc<TestGeneration>,
    pub(crate) completed: Arc<CompletedFixture>,
    pub(crate) consequences: Arc<AtomicUsize>,
    pub(crate) events: Arc<PhaseEventsFixture>,
    path: std::path::PathBuf,
    forced: bool,
}
impl Fixture {
    pub(crate) fn new(forced: bool, proposal: &'static str) -> Self {
        let header = header();
        let consequences = Arc::new(AtomicUsize::new(0));
        let path = std::env::temp_dir().join(format!("mythos-game-{}", header.session_id));
        let domain = domain(&header.script_revision, forced, consequences.clone());
        let position = domain.baseline_scene().unwrap();
        let session = Session::create(
            path.join(format!("{}.jsonl", header.session_id)),
            header.clone(),
            Arc::new(RecordEvents::default()),
        )
        .unwrap();
        let events = Arc::new(PhaseEventsFixture::default());
        let publisher = Publisher::new(
            PhaseState {
                session_id: header.session_id,
                state_epoch: String::new(),
                phase_revision: 0,
                history_revision: 0,
                phase: Phase::Idle,
                scene: Some(position),
                in_flight: None,
                check: None,
                needs_recovery: false,
                resume_required: false,
                checkpoint: None,
                last_operation: None,
            },
            events.clone(),
        )
        .unwrap();
        let completed = Arc::new(CompletedFixture::default());
        let context = Arc::new(Context {
            player_id: "player".into(),
            actor_id: "player".into(),
            session: Arc::new(Mutex::new(session)),
            domain: Arc::new(Mutex::new(Box::new(domain))),
            publisher: Arc::new(publisher),
            coordinator: Coordinator::new(Arc::new(crate::test_support::Events::default()), 16)
                .unwrap(),
            completed: completed.clone(),
        });
        Self {
            context,
            generation: Arc::new(TestGeneration {
                calls: AtomicUsize::new(0),
                hold: std::sync::atomic::AtomicBool::new(false),
                fail_auth: std::sync::atomic::AtomicBool::new(false),
                proposal,
            }),
            completed,
            consequences,
            events,
            path,
            forced,
        }
    }
    pub(crate) async fn reopen(&self) -> Arc<Context> {
        let header = lock(&self.context.session).header().clone();
        lock(&self.context.session).close().unwrap();
        self.context.publisher.retire();
        let session = Session::open(
            self.path.join(format!("{}.jsonl", header.session_id)),
            Arc::new(RecordEvents::default()),
        )
        .unwrap();
        Context::open(
            Arc::new(Mutex::new(session)),
            Box::new(domain(
                &header.script_revision,
                self.forced,
                self.consequences.clone(),
            )),
            "player".into(),
            "player".into(),
            self.context.coordinator.clone(),
            Arc::new(PhaseEventsFixture::default()),
            self.completed.clone(),
        )
        .await
        .unwrap()
    }
    pub(crate) fn frozen(&self, mode: DiceMode) -> FrozenRound {
        FrozenRound {
            profile_id: uuid::Uuid::new_v4().to_string(),
            profile_revision: "1".into(),
            dice_mode: mode,
            generation: self.generation.clone(),
        }
    }
    pub(crate) async fn accept(&self, text: &str, mode: DiceMode) -> Runner {
        Runner::accept(
            self.context.clone(),
            self.context.coordinator.acquire().unwrap(),
            self.frozen(mode),
            crate::game::input::parse_input(text).unwrap(),
        )
        .await
        .unwrap()
    }
    pub(crate) async fn complete(&self, runner: &mut Runner) {
        let mut remaining = 16;
        while runner.stage() != Stage::Idle {
            assert!(remaining > 0, "driver must terminate in bounded steps");
            remaining -= 1;
            runner.step(CancellationToken::new()).await.unwrap();
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unregistered_fixture_world_operations_are_explicitly_refused() {
        assert!(
            PrepareGate::default()
                .block(std::time::Duration::ZERO)
                .is_err()
        );
        let fixture = Fixture::new(false, "{}");
        let header = lock(&fixture.context.session).header().clone();
        let mut domain = domain(&header.script_revision, false, fixture.consequences.clone());
        let mutation = WorldMutation {
            mutation_id: uuid::Uuid::new_v4().to_string(),
            data: crate::record::facts::MutationData {
                kind: "unknown".into(),
                version: 1,
                payload: serde_json::json!({}),
            },
        };
        assert!(!WorldPort::registered(&domain, &mutation.data.kind, 1));
        assert!(!WorldPort::applied(&domain, &mutation.mutation_id, "hash").unwrap());
        assert!(WorldPort::apply(&mut domain, &mutation, "hash").is_err());
        assert!(!HistoryPort::applied(&domain, "unknown", "hash").unwrap());
        assert!(
            HistoryPort::verify_replay_target(&domain, &lock(&fixture.context.session), 0, 1)
                .is_err()
        );
        let request = ProviderInput::Chat(mythos_llm::provider::ChatInput {
            messages: vec![],
            assistant_prefix: None,
        });
        assert_eq!(
            fixture
                .generation
                .prepare(request, GuardSpec::default(), false)
                .err()
                .unwrap()
                .code,
            "app.bad-request"
        );
    }
}
