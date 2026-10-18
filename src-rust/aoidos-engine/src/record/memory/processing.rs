//! 单批记忆生成在游戏 lease 释放后执行，并先持久登记计费请求身份。

use crate::{fault::Fault, request::PreparedGeneration, storage::Storage, turn::Coordinator};
use aoidos_llm::schedule::{AttemptIdentity, AttemptOutcome, BudgetDenial, BudgetPort};
use aoidos_memory::{
    error::{Error, Reason},
    model::{CausalPort, Proposal, ProposalContext},
    repository::{BatchCompletion, BatchState, CycleMarkCommit},
};
use std::sync::{Arc, Mutex};

/// 执行器依赖的最小持久化边界，便于用本地 mock 验证发送 / 回执顺序。
trait BatchPersistence: Send + Sync {
    fn dispatch(&self, attempt: &AttemptIdentity) -> Result<(), Fault>;
    fn receive(
        &self,
        turn_id: &str,
        request_id: &str,
        response: &[u8],
    ) -> Result<(), ReceiveFailure>;
    fn unknown(&self, turn_id: &str, request_id: &str, code: &'static str);
}

enum ReceiveFailure {
    Rejected(Fault),
    Uncertain(Fault),
}

struct SqliteBatchPersistence {
    storage: Arc<Storage>,
    session_id: String,
    catalog: Arc<crate::game::catalog::SceneCatalog>,
    batch_id: String,
}

impl BatchPersistence for SqliteBatchPersistence {
    fn dispatch(&self, attempt: &AttemptIdentity) -> Result<(), Fault> {
        if attempt.attempt != 1 {
            return Err(Fault::bad_request());
        }
        self.storage
            .with_memory(
                &self.session_id,
                &self.catalog,
                |repository, connection, port| {
                    let (_, state) = repository.batch(connection, &self.batch_id)?;
                    if state != BatchState::Queued
                        || !repository.dispatch_batch(
                            connection,
                            port,
                            &self.batch_id,
                            &attempt.turn_id,
                            &attempt.request_id,
                        )?
                    {
                        return Err(Error::Rejected(Reason::StaleVersion));
                    }
                    Ok(())
                },
            )
            .map_err(memory_fault)
    }

    fn receive(
        &self,
        turn_id: &str,
        request_id: &str,
        response: &[u8],
    ) -> Result<(), ReceiveFailure> {
        let result = self
            .storage
            .with_memory(
                &self.session_id,
                &self.catalog,
                |repository, connection, _port| {
                    let receipt = repository.batch_receipt(connection, &self.batch_id)?;
                    if receipt.state != BatchState::Dispatched
                        || receipt.turn_id != turn_id
                        || receipt.request_id != request_id
                    {
                        return Err(Error::Rejected(Reason::StaleVersion));
                    }
                    match repository.receive_batch(
                        connection,
                        &self.batch_id,
                        turn_id,
                        request_id,
                        response,
                    ) {
                        Ok(_) => Ok(None),
                        Err(Error::Rejected(
                            reason @ (Reason::InvalidSchema
                            | Reason::CapacityBlocked
                            | Reason::PolicyUnavailable),
                        )) => {
                            let error_code = match reason {
                                Reason::InvalidSchema => "response.schema",
                                Reason::CapacityBlocked => "response.capacity",
                                Reason::PolicyUnavailable => "response.version",
                                _ => unreachable!(),
                            };
                            repository.reject_batch(connection, &self.batch_id, error_code)?;
                            Ok(Some(Fault::new(
                                "llm.bad-response",
                                "记忆模型响应不符合协议",
                            )))
                        }
                        Err(error) => Err(error),
                    }
                },
            )
            .map_err(memory_fault)
            .map_err(ReceiveFailure::Uncertain)?;
        match result {
            Some(fault) => Err(ReceiveFailure::Rejected(fault)),
            None => Ok(()),
        }
    }

    fn unknown(&self, turn_id: &str, request_id: &str, code: &'static str) {
        let _ = self.storage.with_memory(
            &self.session_id,
            &self.catalog,
            |repository, connection, _| {
                repository.mark_batch_unknown(connection, &self.batch_id, turn_id, request_id, code)
            },
        );
    }
}

struct DispatchBudget {
    inner: Arc<dyn BudgetPort>,
    persistence: Arc<dyn BatchPersistence>,
    dispatched: Mutex<Option<AttemptIdentity>>,
    dispatch_fault: Mutex<Option<Fault>>,
    unknown_marked: Mutex<bool>,
}

impl DispatchBudget {
    fn mark_unknown(&self, attempt: &AttemptIdentity, code: &'static str) {
        let mut marked = self
            .unknown_marked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *marked {
            return;
        }
        self.persistence
            .unknown(&attempt.turn_id, &attempt.request_id, code);
        *marked = true;
    }
}

impl BudgetPort for DispatchBudget {
    fn reserve(&self, attempt: &AttemptIdentity) -> Result<(), BudgetDenial> {
        if attempt.attempt != 1
            || self
                .dispatched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some()
        {
            return Err(dispatch_denied());
        }

        self.inner.reserve(attempt)?;
        if let Err(error) = self.persistence.dispatch(attempt) {
            self.inner.settle(attempt, &AttemptOutcome::NotDispatched);
            *self
                .dispatch_fault
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
            return Err(dispatch_denied());
        }
        *self
            .dispatched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(attempt.clone());
        Ok(())
    }

    fn settle(&self, attempt: &AttemptIdentity, outcome: &AttemptOutcome) {
        self.inner.settle(attempt, outcome);
        if !matches!(outcome, AttemptOutcome::Settled { .. }) {
            self.mark_unknown(attempt, "llm.unknown");
        }
    }
}

/// 在 coordinator 空闲时取得独立 lease，执行一个不可重试的记忆请求并保存原始响应。
///
/// `request` 必须由可信引擎按冻结批次输入构造，并携带本局 027 `LedgerBudgetPort`；
/// 调用方负责确认批次授权。此函数不构造候选操作、不读取设置，也没有自动触发器。
/// # Errors
/// 游戏仍占用 lease、预算预留 / 批次登记失败或响应不能持久化时返回脱敏错误。
pub async fn process_authorized_batch(
    coordinator: &Coordinator,
    storage: Arc<Storage>,
    session_id: String,
    catalog: Arc<crate::game::catalog::SceneCatalog>,
    batch_id: String,
    request: PreparedGeneration,
) -> Result<crate::turn::PrivateResult, Fault> {
    let persistence: Arc<dyn BatchPersistence> = Arc::new(SqliteBatchPersistence {
        storage,
        session_id,
        catalog,
        batch_id,
    });
    execute_batch(coordinator, persistence, request).await
}

/// 从当前 queued 批次恢复完整提案上下文，并复核其中的刻痕授权。
///
/// 输出 context 完全来自冻结快照；当前剧本、周目、实体目录和来源重建结果必须与已冻结
/// 授权一致，不能由调用方补充或替换普通候选字段。
/// # Errors
/// 批次状态、历史 / 实体 / 证据修订或任一来源不再匹配时拒绝构造请求上下文。
pub fn prepare_cycle_mark_context(
    storage: &Storage,
    session_id: &str,
    catalog: &crate::game::catalog::SceneCatalog,
    batch_id: &str,
    context: &mut ProposalContext,
) -> Result<(), Fault> {
    storage
        .with_memory_context(
            session_id,
            catalog,
            |repository, connection, port, session, directory| {
                let (snapshot, state) = repository.batch(connection, batch_id)?;
                if state != BatchState::Queued {
                    return Err(Error::Rejected(Reason::StaleVersion));
                }
                let run = repository.run_for_session(connection, session_id)?;
                let cycle_context = super::build_cycle_mark_context(super::CycleMarkContextInput {
                    catalog,
                    run: &run,
                    session,
                    directory,
                    batch: &snapshot,
                    source_port: port,
                })
                .map_err(|_| Error::Rejected(Reason::StaleVersion))?;
                let frozen = frozen_proposal_context(&snapshot)?;
                let mut rebuilt = frozen.clone();
                cycle_context
                    .apply_to(&mut rebuilt)
                    .map_err(|_| Error::Rejected(Reason::InvalidSchema))?;
                if !same_proposal_context(&rebuilt, &frozen)? {
                    return Err(Error::Rejected(Reason::StaleVersion));
                }
                *context = frozen;
                Ok(())
            },
        )
        .map_err(memory_fault)
}

/// 按当前冻结授权校验已收回的提案，并将刻痕与普通候选摘要原子完成。
///
/// 提案上下文只从收到响应的冻结批次恢复；此函数不会重新发送请求，也不会重新计算普通
/// 候选的操作结果。含刻痕时由仓储在同一事务写入刻痕和完成回执。
/// # Errors
/// 批次尚未收到响应、提案不符合冻结授权或当前来源已失效时拒绝完成；相同完成重放返回 `false`。
pub fn complete_received_batch(
    storage: &Storage,
    session_id: &str,
    catalog: &crate::game::catalog::SceneCatalog,
    batch_id: &str,
    completion: &BatchCompletion,
) -> Result<bool, Fault> {
    storage
        .with_memory_context(
            session_id,
            catalog,
            |repository, connection, port, session, directory| {
                let (snapshot, state) = repository.batch(connection, batch_id)?;
                let run = repository.run_for_session(connection, session_id)?;
                let boundary = port.boundary()?;
                if snapshot.run_id != run.run_id
                    || snapshot.history_revision != boundary.history_revision
                    || snapshot.entity_revision != boundary.entity_revision
                    || snapshot.evidence_revision != boundary.evidence_revision
                    || snapshot.policy_hash != run.policy_hash
                {
                    return Err(Error::Rejected(Reason::StaleVersion));
                }
                if !matches!(state, BatchState::Received | BatchState::Completed) {
                    return Err(Error::Rejected(Reason::StaleVersion));
                }
                let cycle_context = super::build_cycle_mark_context(super::CycleMarkContextInput {
                    catalog,
                    run: &run,
                    session,
                    directory,
                    batch: &snapshot,
                    source_port: port,
                })
                .map_err(|_| Error::Rejected(Reason::StaleVersion))?;
                let context = frozen_proposal_context(&snapshot)?;
                let mut rebuilt = context.clone();
                cycle_context
                    .apply_to(&mut rebuilt)
                    .map_err(|_| Error::Rejected(Reason::InvalidSchema))?;
                if !same_proposal_context(&rebuilt, &context)? {
                    return Err(Error::Rejected(Reason::StaleVersion));
                }
                let receipt = repository.batch_receipt(connection, batch_id)?;
                if receipt.state != state {
                    return Err(Error::Rejected(Reason::StaleVersion));
                }
                let response = receipt.response.as_deref().ok_or(Error::Corrupt)?;
                let proposal = Proposal::parse(response, &context)?;
                let cycle_marks = proposal
                    .cycle_marks
                    .iter()
                    .map(|candidate| {
                        Ok(CycleMarkCommit {
                            candidate_id: candidate.candidate_id.clone(),
                            mark: candidate.materialize(&context)?,
                        })
                    })
                    .collect::<aoidos_memory::error::Result<Vec<_>>>()?;

                if cycle_marks.is_empty() {
                    let bytes = aoidos_json::canonical_string(completion)
                        .map_err(|_| Error::Rejected(Reason::InvalidSchema))?
                        .into_bytes();
                    repository.complete_batch(connection, batch_id, &bytes)
                } else {
                    repository.complete_batch_with_cycle_marks(
                        connection,
                        port,
                        batch_id,
                        completion,
                        &cycle_marks,
                    )
                }
            },
        )
        .map_err(memory_fault)
}

fn frozen_proposal_context(
    snapshot: &aoidos_memory::repository::BatchSnapshot,
) -> aoidos_memory::error::Result<ProposalContext> {
    let value = snapshot
        .input
        .get("proposalContext")
        .cloned()
        .ok_or(Error::Rejected(Reason::InvalidSchema))?;
    aoidos_json::from_value(value).map_err(|_| Error::Rejected(Reason::InvalidSchema))
}

fn same_proposal_context(
    left: &ProposalContext,
    right: &ProposalContext,
) -> aoidos_memory::error::Result<bool> {
    let left =
        aoidos_json::canonical_string(left).map_err(|_| Error::Rejected(Reason::InvalidSchema))?;
    let right =
        aoidos_json::canonical_string(right).map_err(|_| Error::Rejected(Reason::InvalidSchema))?;
    Ok(left == right)
}

async fn execute_batch(
    coordinator: &Coordinator,
    persistence: Arc<dyn BatchPersistence>,
    mut request: PreparedGeneration,
) -> Result<crate::turn::PrivateResult, Fault> {
    // Acquiring here makes active game rounds win the shared app-wide gate.
    let lease = coordinator.acquire()?;
    let budget = Arc::new(DispatchBudget {
        inner: request.budget.clone(),
        persistence,
        dispatched: Mutex::new(None),
        dispatch_fault: Mutex::new(None),
        unknown_marked: Mutex::new(false),
    });
    request.budget = budget.clone();
    request.policy.max_requests = 1;
    request.policy.transport_retries = 0;
    request.policy.ladder.clear();

    let result = match coordinator.run_private(&lease, request).await {
        Ok(result) => result,
        Err(error) => {
            if let Some(dispatch_fault) = budget
                .dispatch_fault
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                return Err(dispatch_fault);
            }
            if let Some(attempt) = budget
                .dispatched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
            {
                budget.mark_unknown(&attempt, "llm.unknown");
            }
            return Err(error);
        }
    };
    if result.outcome == crate::ports::Outcome::Completed {
        let attempt = budget
            .dispatched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(receipt_failed)?;
        match budget.persistence.receive(
            &result.turn_id,
            &attempt.request_id,
            result.text.as_bytes(),
        ) {
            Ok(()) => {}
            Err(ReceiveFailure::Rejected(fault)) => return Err(fault),
            Err(ReceiveFailure::Uncertain(fault)) => {
                budget.mark_unknown(&attempt, "memory.receipt-failed");
                return Err(fault);
            }
        }
    }
    // 新游戏回合不能越过已确认的完整响应回执。
    drop(lease);
    Ok(result)
}

fn dispatch_denied() -> BudgetDenial {
    BudgetDenial {
        reason: "memory-batch-dispatch-unavailable".into(),
    }
}

fn receipt_failed() -> Fault {
    Fault::new("store.failed", "记忆批次回执无法保存")
}

fn memory_fault(error: Error) -> Fault {
    let code = error.code();
    let message = match code.as_str() {
        "app.bad-request" => "记忆批次状态已变化或请求不合法",
        "app.not-found" => "记忆批次不存在",
        "app.not-ready" => "记忆上下文当前不可用",
        "store.corrupt" => "记忆存储状态已损坏",
        _ if code.starts_with("store.") => "记忆存储操作失败",
        _ => "记忆操作失败",
    };
    Fault::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ports::Outcome, test_support::generation};
    use aoidos_llm::schedule::{BudgetDenial, BudgetPort};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct MemoryPort {
        calls: Arc<Mutex<Vec<String>>>,
        fail_dispatch: bool,
        receive_failure: Option<ReceiveFailureKind>,
        received: Mutex<Option<(String, String, Vec<u8>)>>,
        unknown: Mutex<Vec<String>>,
    }

    #[derive(Clone, Copy)]
    enum ReceiveFailureKind {
        Rejected,
        Uncertain,
    }

    impl BatchPersistence for MemoryPort {
        fn dispatch(&self, _attempt: &AttemptIdentity) -> Result<(), Fault> {
            self.calls.lock().unwrap().push("dispatch".into());
            if self.fail_dispatch {
                Err(Fault::new("store.corrupt", "测试用 dispatch 状态错误"))
            } else {
                Ok(())
            }
        }

        fn receive(
            &self,
            turn_id: &str,
            request_id: &str,
            response: &[u8],
        ) -> Result<(), ReceiveFailure> {
            self.calls.lock().unwrap().push("receive".into());
            if let Some(failure) = self.receive_failure {
                return Err(match failure {
                    ReceiveFailureKind::Rejected => ReceiveFailure::Rejected(Fault::new(
                        "llm.bad-response",
                        "记忆模型响应不符合协议",
                    )),
                    ReceiveFailureKind::Uncertain => ReceiveFailure::Uncertain(receipt_failed()),
                });
            }
            *self.received.lock().unwrap() =
                Some((turn_id.into(), request_id.into(), response.to_vec()));
            Ok(())
        }

        fn unknown(&self, _: &str, _: &str, code: &'static str) {
            self.unknown.lock().unwrap().push(code.into());
        }
    }

    struct RecordingBudget {
        calls: Arc<Mutex<Vec<String>>>,
        reserves: AtomicUsize,
    }

    impl BudgetPort for RecordingBudget {
        fn reserve(&self, _: &AttemptIdentity) -> Result<(), BudgetDenial> {
            self.reserves.fetch_add(1, Ordering::SeqCst);
            self.calls.lock().unwrap().push("reserve".into());
            Ok(())
        }

        fn settle(&self, _: &AttemptIdentity, _: &AttemptOutcome) {
            self.calls.lock().unwrap().push("settle".into());
        }
    }

    #[tokio::test]
    async fn dispatch_uses_one_billed_private_attempt_and_persists_response() {
        let events = Arc::new(crate::test_support::Events::default());
        let coordinator = Coordinator::new(events, 16).unwrap();
        let persistence = Arc::new(MemoryPort::default());
        let order = persistence.calls.clone();
        let budget = Arc::new(RecordingBudget {
            calls: order.clone(),
            reserves: AtomicUsize::new(0),
        });
        let (mut request, sends) = generation(
            vec![
                Ok(aoidos_llm::provider::ProviderDelta::Text(
                    r#"{"version":1,"candidates":[]}"#.into(),
                )),
                Ok(aoidos_llm::provider::ProviderDelta::Finish(
                    aoidos_llm::provider::ProviderFinish::Stop,
                )),
            ],
            false,
        );
        request.budget = budget.clone();

        let result = execute_batch(&coordinator, persistence.clone(), request)
            .await
            .unwrap();

        assert_eq!(result.outcome, Outcome::Completed);
        assert_eq!(sends.load(Ordering::SeqCst), 1);
        assert_eq!(budget.reserves.load(Ordering::SeqCst), 1);
        assert_eq!(
            *order.lock().unwrap(),
            ["reserve", "dispatch", "settle", "receive"]
        );
        let receipt = persistence.received.lock().unwrap().clone().unwrap();
        assert_eq!(receipt.0, result.turn_id);
        assert_eq!(receipt.2, br#"{"version":1,"candidates":[]}"#);
        coordinator.check_available().unwrap();
    }

    #[tokio::test]
    async fn game_lease_blocks_memory_dispatch_before_budget_or_network() {
        let events = Arc::new(crate::test_support::Events::default());
        let coordinator = Coordinator::new(events, 16).unwrap();
        let game_lease = coordinator.acquire().unwrap();
        let persistence = Arc::new(MemoryPort::default());
        let (request, sends) = generation(
            vec![Ok(aoidos_llm::provider::ProviderDelta::Finish(
                aoidos_llm::provider::ProviderFinish::Stop,
            ))],
            false,
        );

        let error = execute_batch(&coordinator, persistence.clone(), request)
            .await
            .err()
            .unwrap();

        assert_eq!(error.code, "app.busy");
        assert!(persistence.calls.lock().unwrap().is_empty());
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        drop(game_lease);
    }

    #[tokio::test]
    async fn uncertain_private_outcome_marks_the_dispatched_batch_unknown() {
        let events = Arc::new(crate::test_support::Events::default());
        let coordinator = Coordinator::new(events, 16).unwrap();
        let persistence = Arc::new(MemoryPort::default());
        let (request, _) = generation(
            vec![Err(aoidos_llm::error::ProviderError::Interrupted)],
            false,
        );

        let error = execute_batch(&coordinator, persistence.clone(), request)
            .await
            .err()
            .unwrap();

        assert!(error.code.starts_with("llm."));
        assert_eq!(*persistence.unknown.lock().unwrap(), ["llm.unknown"]);
        assert!(persistence.received.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn dispatch_failure_releases_the_reserved_budget_without_sending() {
        let events = Arc::new(crate::test_support::Events::default());
        let coordinator = Coordinator::new(events, 16).unwrap();
        let persistence = Arc::new(MemoryPort {
            fail_dispatch: true,
            ..MemoryPort::default()
        });
        let order = persistence.calls.clone();
        let budget = Arc::new(RecordingBudget {
            calls: order.clone(),
            reserves: AtomicUsize::new(0),
        });
        let (mut request, sends) = generation(
            vec![Ok(aoidos_llm::provider::ProviderDelta::Finish(
                aoidos_llm::provider::ProviderFinish::Stop,
            ))],
            false,
        );
        request.budget = budget.clone();

        let error = execute_batch(&coordinator, persistence, request)
            .await
            .err()
            .unwrap();

        assert_eq!(error.code, "store.corrupt");
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        assert_eq!(budget.reserves.load(Ordering::SeqCst), 1);
        assert_eq!(*order.lock().unwrap(), ["reserve", "dispatch", "settle"]);
    }

    #[tokio::test]
    async fn receipt_failure_marks_the_sent_batch_unknown_without_retry() {
        let events = Arc::new(crate::test_support::Events::default());
        let coordinator = Coordinator::new(events, 16).unwrap();
        let persistence = Arc::new(MemoryPort {
            receive_failure: Some(ReceiveFailureKind::Uncertain),
            ..MemoryPort::default()
        });
        let (request, sends) = generation(
            vec![
                Ok(aoidos_llm::provider::ProviderDelta::Text("响应".into())),
                Ok(aoidos_llm::provider::ProviderDelta::Finish(
                    aoidos_llm::provider::ProviderFinish::Stop,
                )),
            ],
            false,
        );

        let error = execute_batch(&coordinator, persistence.clone(), request)
            .await
            .err()
            .unwrap();

        assert_eq!(error.code, "store.failed");
        assert_eq!(sends.load(Ordering::SeqCst), 1);
        assert_eq!(
            *persistence.unknown.lock().unwrap(),
            ["memory.receipt-failed"]
        );
        assert!(persistence.received.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn invalid_response_is_rejected_without_becoming_unknown() {
        let events = Arc::new(crate::test_support::Events::default());
        let coordinator = Coordinator::new(events, 16).unwrap();
        let persistence = Arc::new(MemoryPort {
            receive_failure: Some(ReceiveFailureKind::Rejected),
            ..MemoryPort::default()
        });
        let (request, sends) = generation(
            vec![
                Ok(aoidos_llm::provider::ProviderDelta::Text("not JSON".into())),
                Ok(aoidos_llm::provider::ProviderDelta::Finish(
                    aoidos_llm::provider::ProviderFinish::Stop,
                )),
            ],
            false,
        );

        let error = execute_batch(&coordinator, persistence.clone(), request)
            .await
            .err()
            .unwrap();

        assert_eq!(error.code, "llm.bad-response");
        assert_eq!(sends.load(Ordering::SeqCst), 1);
        assert!(persistence.unknown.lock().unwrap().is_empty());
    }
}
