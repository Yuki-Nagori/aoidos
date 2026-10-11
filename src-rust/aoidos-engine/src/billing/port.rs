use super::{BillingError, BillingLedger, Reservation, Settlement};
use crate::{fault::Fault, storage::Storage};
use aoidos_billing::{money::NanoMoney, usage::NormalizedUsage};
use aoidos_llm::schedule::{AttemptIdentity, AttemptOutcome, BudgetDenial, BudgetPort};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// 将既有单次尝试调度端口接入持久账本的生产适配器。
pub struct LedgerBudgetPort {
    storage: Arc<Storage>,
    run_id: String,
    profile_id: String,
    provider_id: String,
    model_id: String,
    route_policy: String,
    max_input_tokens: u64,
}

impl LedgerBudgetPort {
    pub fn new(
        storage: Arc<Storage>,
        run_id: String,
        profile_id: String,
        provider_id: String,
        model_id: String,
        route_policy: String,
        max_input_tokens: u64,
    ) -> Self {
        Self {
            storage,
            run_id,
            profile_id,
            provider_id,
            model_id,
            route_policy,
            max_input_tokens,
        }
    }

    fn reserve_attempt(&self, attempt: &AttemptIdentity) -> Result<(), String> {
        if attempt.model != self.model_id {
            return Err("budget.price-missing".into());
        }
        let dispatch_at = now();
        self.storage
            .with_database(|connection| {
                let price = BillingLedger::profile_price(
                    connection,
                    &self.profile_id,
                    &self.provider_id,
                    &self.model_id,
                    &self.route_policy,
                    &dispatch_at,
                )
                .map_err(billing_fault)?
                .ok_or(BillingError::PriceMissing)
                .map_err(billing_fault)?;
                let reserve = price
                    .upper_bound_cost(self.max_input_tokens, u64::from(attempt.max_tokens))
                    .and_then(|cost| cost.reserve().map_err(Into::into))
                    .map_err(|_| Fault::new("budget.price-missing", "模型价格或预留上界不可用"))?;
                let reservation = Reservation {
                    request_id: attempt.request_id.clone(),
                    run_id: self.run_id.clone(),
                    turn_id: attempt.turn_id.clone(),
                    profile_id: self.profile_id.clone(),
                    attempt: attempt.attempt,
                    provider_id: self.provider_id.clone(),
                    model_id: self.model_id.clone(),
                    route_policy: self.route_policy.clone(),
                    price_version_id: price.version_id,
                    currency: price.currency,
                    dispatch_at: dispatch_at.clone(),
                    amount: reserve,
                };
                BillingLedger::reserve(connection, &reservation).map_err(billing_fault)
            })
            .map_err(|fault| fault.code)
    }

    fn settle_attempt(&self, attempt: &AttemptIdentity, outcome: &AttemptOutcome) {
        let updated_at = now();
        let settlement = match outcome {
            AttemptOutcome::Settled { usage: Some(usage) } => {
                let Some(cached_input) = usage.cached_prompt_tokens else {
                    self.mark_unknown(attempt, &updated_at);
                    return;
                };
                let Some(total_tokens) = usage.prompt_tokens.checked_add(usage.completion_tokens)
                else {
                    self.mark_unknown(attempt, &updated_at);
                    return;
                };
                let reasoning_included = usage.reasoning_tokens;
                let cached_write = usage.cache_creation_tokens;
                let request_id = attempt.request_id.clone();
                let calculated = self.storage.with_database(|connection| {
                    let (price, dispatch_at) =
                        BillingLedger::request_price_context(connection, &request_id)
                            .map_err(billing_fault)?;
                    if (cached_write.is_none() && price.input_cached_write.is_some())
                        || (cached_write.is_some_and(|tokens| tokens > 0)
                            && price.input_cached_write.is_none())
                    {
                        return Err(Fault::new(
                            "budget.usage-unconfirmed",
                            "缓存创建用量或对应费率缺失",
                        ));
                    }
                    let normalized = NormalizedUsage::from_provider_with_cache_write(
                        usage.prompt_tokens,
                        Some(cached_input),
                        cached_write,
                        usage.completion_tokens,
                        reasoning_included,
                    )
                    .map_err(|_| Fault::new("budget.invalid-usage", "供应商用量不合法"))?;
                    let amount = normalized
                        .exact_cost_at(&price, &dispatch_at)
                        .and_then(|cost| cost.settle().map_err(Into::into))
                        .map_err(|_| Fault::new("budget.invalid-usage", "供应商用量或费用溢出"))?;
                    Ok(Settlement {
                        input_total: usage.prompt_tokens,
                        input_cached: cached_input,
                        input_cached_write: cached_write.unwrap_or(0),
                        output_total: usage.completion_tokens,
                        reasoning_included,
                        total_tokens,
                        amount,
                        result_digest: digest(
                            usage.prompt_tokens,
                            cached_input,
                            cached_write.unwrap_or(0),
                            usage.completion_tokens,
                            amount,
                        ),
                    })
                });
                calculated.ok()
            }
            AttemptOutcome::Settled { usage: None }
            | AttemptOutcome::Failed
            | AttemptOutcome::Cancelled => None,
            AttemptOutcome::NotDispatched => {
                let amount = NanoMoney::ZERO;
                Some(Settlement {
                    input_total: 0,
                    input_cached: 0,
                    input_cached_write: 0,
                    output_total: 0,
                    reasoning_included: None,
                    total_tokens: 0,
                    amount,
                    result_digest: digest(0, 0, 0, 0, amount),
                })
            }
        };
        let request_id = attempt.request_id.clone();
        let _ = self.storage.with_database(|connection| {
            BillingLedger::settle(connection, &request_id, settlement.as_ref(), &updated_at)
                .map(|_| ())
                .map_err(billing_fault)
        });
    }

    fn mark_unknown(&self, attempt: &AttemptIdentity, updated_at: &str) {
        let request_id = attempt.request_id.clone();
        let _ = self.storage.with_database(|connection| {
            BillingLedger::settle(connection, &request_id, None, updated_at)
                .map(|_| ())
                .map_err(billing_fault)
        });
    }
}

impl BudgetPort for LedgerBudgetPort {
    fn reserve(&self, attempt: &AttemptIdentity) -> Result<(), BudgetDenial> {
        self.reserve_attempt(attempt)
            .map_err(|reason| BudgetDenial { reason })
    }

    fn settle(&self, attempt: &AttemptIdentity, outcome: &AttemptOutcome) {
        self.settle_attempt(attempt, outcome);
    }
}

fn now() -> String {
    format_timestamp(OffsetDateTime::now_utc())
}

fn format_timestamp(timestamp: OffsetDateTime) -> String {
    timestamp
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

fn digest(input: u64, cached: u64, cached_write: u64, output: u64, amount: NanoMoney) -> String {
    let mut hash = Sha256::new();
    hash.update(input.to_be_bytes());
    hash.update(cached.to_be_bytes());
    hash.update(cached_write.to_be_bytes());
    hash.update(output.to_be_bytes());
    hash.update(amount.nanos().to_be_bytes());
    format!("sha256:{:x}", hash.finalize())
}

fn billing_fault(error: BillingError) -> Fault {
    match error {
        BillingError::Invalid | BillingError::StaleRevision => Fault::bad_request(),
        BillingError::LimitMissing => {
            Fault::new("budget.run-limit-missing", "本局尚未设置对应币种上限")
        }
        BillingError::Exceeded => Fault::new("budget.exceeded", "本局对应币种额度不足"),
        BillingError::PriceMissing => Fault::new("budget.price-missing", "模型价格尚未登记"),
        BillingError::PriceHistoryLimit => {
            Fault::new("budget.price-history-limit", "该模型价格版本已达到安全上限")
        }
        BillingError::Conflict => Fault::new("budget.conflict", "请求计费状态发生冲突"),
        BillingError::NotFound => Fault::new("app.not-found", "费用记录不存在"),
        BillingError::Storage => Fault::new("store.database", "费用账本不可用"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        migration::{MigrationEvent, MigrationEvents},
        record::session::{Appended, RecordEvents},
    };
    use aoidos_billing::{
        money::{Currency, UnitPrice},
        price::PriceVersion,
    };
    use aoidos_llm::{
        provider::{RequestMode, Usage},
        schedule::AttemptKind,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    #[derive(Default)]
    struct Events(AtomicU64);

    impl MigrationEvents for Events {
        fn prepare(&self, _: &MigrationEvent) -> Result<u64, Fault> {
            Ok(self.0.fetch_add(1, Ordering::SeqCst) + 1)
        }
        fn deliver(&self, _: u64, _: MigrationEvent) -> Result<(), Fault> {
            Ok(())
        }
        fn retire(&self, _: &str) {}
    }

    impl RecordEvents for Events {
        fn prepare(&self, _: &Appended) -> Result<u64, Fault> {
            Ok(self.0.fetch_add(1, Ordering::SeqCst) + 1)
        }
        fn deliver(&self, _: u64, _: Appended) -> Result<(), Fault> {
            Ok(())
        }
    }

    fn attempt(model: &str, request_id: &str) -> AttemptIdentity {
        AttemptIdentity {
            request_id: request_id.into(),
            turn_id: "turn-1".into(),
            attempt: 1,
            kind: AttemptKind::Initial,
            model: model.into(),
            mode: RequestMode::Completion,
            max_tokens: 20,
            temperature: 1.0,
        }
    }

    fn price() -> PriceVersion {
        PriceVersion {
            version_id: "price-1".into(),
            provider_id: "provider".into(),
            model_id: "model".into(),
            route_policy: "direct".into(),
            currency: Currency::Cny,
            unit_tokens: 1000,
            input_uncached: UnitPrice::parse("1").unwrap(),
            input_cached: UnitPrice::parse("0.5").unwrap(),
            input_cached_write: Some(UnitPrice::parse("1.5").unwrap()),
            output: UnitPrice::parse("2").unwrap(),
            schedule: None,
            usage_mapping_version: "v1".into(),
            source_kind: "user".into(),
            source_url: None,
            checked_at: "2020-01-01T00:00:00Z".into(),
            effective_from: "2020-01-01T00:00:00Z".into(),
            valid_until: None,
        }
    }

    fn storage() -> (Arc<Storage>, std::path::PathBuf) {
        let root =
            std::env::temp_dir().join(format!("aoidos-billing-port-{}", uuid::Uuid::new_v4()));
        let events = Arc::new(Events::default());
        let storage = Arc::new(Storage::new(&root, events.clone(), events));
        storage.ready().unwrap();
        (storage, root)
    }

    fn port(storage: Arc<Storage>) -> LedgerBudgetPort {
        LedgerBudgetPort::new(
            storage,
            "run-1".into(),
            "profile-1".into(),
            "provider".into(),
            "model".into(),
            "direct".into(),
            100,
        )
    }

    fn configure(storage: &Storage, limit: &str) {
        storage
            .with_database(|connection| {
                BillingLedger::set_settings(
                    connection,
                    "run-1",
                    NanoMoney::parse(limit).unwrap(),
                    NanoMoney::parse("0").unwrap(),
                    None,
                    "2020-01-01T00:00:00Z",
                )
                .map_err(billing_fault)?;
                BillingLedger::register_price(connection, &price(), "2020-01-01T00:00:00Z")
                    .map_err(billing_fault)?;
                BillingLedger::select_profile_price(
                    connection,
                    "profile-1",
                    "provider",
                    "model",
                    "direct",
                    Currency::Cny,
                    "2020-01-01T00:00:00Z",
                )
                .map_err(billing_fault)?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn reservation_and_settlement_use_the_persisted_price_and_keep_unknown_usage() {
        let (storage, root) = storage();
        let port = port(storage.clone());
        let denial = port.reserve(&attempt("other", "wrong-model")).unwrap_err();
        assert_eq!(denial.reason, "budget.price-missing");

        configure(&storage, "1");
        port.reserve(&attempt("model", "request-1")).unwrap();
        port.settle(
            &attempt("model", "request-1"),
            &AttemptOutcome::Settled {
                usage: Some(Usage {
                    prompt_tokens: 50,
                    completion_tokens: 10,
                    cached_prompt_tokens: None,
                    cache_creation_tokens: None,
                    reasoning_tokens: None,
                }),
            },
        );
        let incomplete = storage
            .with_database(|connection| {
                connection
                    .query_row(
                        "SELECT state FROM billing_physical_requests WHERE request_id='request-1'",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(query_fault)
            })
            .unwrap();
        assert_eq!(incomplete, "unconfirmed");

        port.reserve(&attempt("model", "request-2")).unwrap();
        port.settle(
            &attempt("model", "request-2"),
            &AttemptOutcome::Settled {
                usage: Some(Usage {
                    prompt_tokens: 50,
                    completion_tokens: 10,
                    cached_prompt_tokens: Some(10),
                    cache_creation_tokens: Some(0),
                    reasoning_tokens: Some(2),
                }),
            },
        );
        let settled = storage
            .with_database(|connection| {
                connection
                    .query_row(
                        "SELECT state FROM billing_physical_requests WHERE request_id='request-2'",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(query_fault)
            })
            .unwrap();
        assert_eq!(settled, "settled");
        port.reserve(&attempt("model", "request-cache-write"))
            .unwrap();
        port.settle(
            &attempt("model", "request-cache-write"),
            &AttemptOutcome::Settled {
                usage: Some(Usage {
                    prompt_tokens: 50,
                    completion_tokens: 10,
                    cached_prompt_tokens: Some(10),
                    cache_creation_tokens: Some(5),
                    reasoning_tokens: Some(2),
                }),
            },
        );
        let cache_write_state = storage.with_database(|connection| {
            connection.query_row("SELECT state FROM billing_physical_requests WHERE request_id='request-cache-write'", [], |row| row.get::<_, String>(0))
                .map_err(query_fault)
        }).unwrap();
        assert_eq!(cache_write_state, "settled");
        let persisted_write = storage.with_database(|connection| {
            connection.query_row(
                "SELECT input_cached_write,actual_nanos FROM billing_physical_requests WHERE request_id='request-cache-write'",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            ).map_err(query_fault)
        }).unwrap();
        assert_eq!(persisted_write, (5, 67_500_000));
        port.reserve(&attempt("model", "request-overflow")).unwrap();
        port.settle(
            &attempt("model", "request-overflow"),
            &AttemptOutcome::Settled {
                usage: Some(Usage {
                    prompt_tokens: u64::MAX,
                    completion_tokens: 1,
                    cached_prompt_tokens: Some(0),
                    cache_creation_tokens: Some(0),
                    reasoning_tokens: None,
                }),
            },
        );
        let overflow_state = storage.with_database(|connection| {
            connection.query_row("SELECT state FROM billing_physical_requests WHERE request_id='request-overflow'", [], |row| row.get::<_, String>(0))
                .map_err(query_fault)
        }).unwrap();
        assert_eq!(overflow_state, "unconfirmed");
        storage.close().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reservation_denial_and_non_success_outcomes_leave_auditable_records() {
        let (limited_storage, root) = storage();
        configure(&limited_storage, "0.01");
        let limited_port = port(limited_storage.clone());
        assert_eq!(
            limited_port
                .reserve(&attempt("model", "too-large"))
                .unwrap_err()
                .reason,
            "budget.exceeded"
        );
        limited_storage.close().unwrap();
        std::fs::remove_dir_all(root).unwrap();

        let (storage, root) = storage();
        configure(&storage, "10");
        let port = port(storage.clone());
        port.reserve(&attempt("model", "cancelled")).unwrap();
        port.settle(&attempt("model", "cancelled"), &AttemptOutcome::Cancelled);
        let state = storage
            .with_database(|connection| {
                connection
                    .query_row(
                        "SELECT state FROM billing_physical_requests WHERE request_id='cancelled'",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(query_fault)
            })
            .unwrap();
        assert_eq!(state, "unconfirmed");

        port.reserve(&attempt("model", "not-dispatched")).unwrap();
        port.settle(
            &attempt("model", "not-dispatched"),
            &AttemptOutcome::NotDispatched,
        );
        let unsent: (String, Option<i64>) = storage
            .with_database(|connection| {
                connection
                    .query_row(
                        "SELECT state,actual_nanos FROM billing_physical_requests WHERE request_id='not-dispatched'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(query_fault)
            })
            .unwrap();
        assert_eq!(unsent, ("settled".into(), Some(0)));
        storage.close().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn settlement_preserves_unknown_cache_writes_and_supports_legacy_zero_writes() {
        let (storage, root) = storage();
        configure(&storage, "10");
        let port = port(storage.clone());
        let mut legacy_price = price();
        legacy_price.version_id = "legacy-price".into();
        legacy_price.route_policy = "legacy-direct".into();
        legacy_price.input_cached_write = None;
        storage
            .with_database(|connection| {
                BillingLedger::register_price(connection, &legacy_price, "2020-01-01T00:00:00Z")
                    .map_err(billing_fault)?;
                BillingLedger::select_profile_price(
                    connection,
                    "legacy-profile",
                    "provider",
                    "model",
                    "legacy-direct",
                    Currency::Cny,
                    "2020-01-01T00:00:00Z",
                )
                .map_err(billing_fault)?;
                Ok(())
            })
            .unwrap();

        for (request_id, price_version_id, cache_creation_tokens, expected_state, actual) in [
            ("missing-write-usage", "price-1", None, "unconfirmed", None),
            (
                "legacy-no-writes",
                "legacy-price",
                None,
                "settled",
                Some(65_000_000),
            ),
            (
                "legacy-zero-writes",
                "legacy-price",
                Some(0),
                "settled",
                Some(65_000_000),
            ),
            (
                "legacy-positive-writes",
                "legacy-price",
                Some(5),
                "unconfirmed",
                None,
            ),
        ] {
            let profile_id = if price_version_id == "legacy-price" {
                "legacy-profile"
            } else {
                "profile-1"
            };
            let route_policy = if price_version_id == "legacy-price" {
                "legacy-direct"
            } else {
                "direct"
            };
            // 旧版本已持久化的预留仍须可结算；当前端口不为缺少缓存创建费率的新请求预留。
            storage
                .with_database(|connection| {
                    BillingLedger::reserve(
                        connection,
                        &Reservation {
                            request_id: request_id.into(),
                            run_id: "run-1".into(),
                            turn_id: "turn-1".into(),
                            profile_id: profile_id.into(),
                            attempt: 1,
                            provider_id: "provider".into(),
                            model_id: "model".into(),
                            route_policy: route_policy.into(),
                            price_version_id: price_version_id.into(),
                            currency: Currency::Cny,
                            dispatch_at: "2020-01-01T00:00:00Z".into(),
                            amount: NanoMoney::parse("0.5").unwrap(),
                        },
                    )
                    .map_err(billing_fault)
                })
                .unwrap();
            port.settle(
                &attempt("model", request_id),
                &AttemptOutcome::Settled {
                    usage: Some(Usage {
                        prompt_tokens: 50,
                        completion_tokens: 10,
                        cached_prompt_tokens: Some(10),
                        cache_creation_tokens,
                        reasoning_tokens: None,
                    }),
                },
            );
            let persisted = storage.with_database(|connection| {
                connection.query_row(
                    "SELECT state,actual_nanos,input_cached_write,reserved_nanos FROM billing_physical_requests WHERE request_id=?1",
                    [request_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, Option<i64>>(2)?, row.get::<_, i64>(3)?)),
                ).map_err(query_fault)
            }).unwrap();
            assert_eq!(
                persisted,
                (
                    expected_state.into(),
                    actual,
                    actual.map(|_| 0),
                    500_000_000,
                ),
                "{request_id}"
            );
        }
        storage.close().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fault_mapping_and_digest_are_stable() {
        for (error, code) in [
            (BillingError::Invalid, "app.bad-request"),
            (BillingError::StaleRevision, "app.bad-request"),
            (BillingError::LimitMissing, "budget.run-limit-missing"),
            (BillingError::Exceeded, "budget.exceeded"),
            (BillingError::PriceMissing, "budget.price-missing"),
            (
                BillingError::PriceHistoryLimit,
                "budget.price-history-limit",
            ),
            (BillingError::Conflict, "budget.conflict"),
            (BillingError::NotFound, "app.not-found"),
            (BillingError::Storage, "store.database"),
        ] {
            assert_eq!(billing_fault(error).code, code);
        }
        assert_eq!(
            digest(1, 0, 0, 2, NanoMoney::parse("3").unwrap()),
            digest(1, 0, 0, 2, NanoMoney::parse("3").unwrap())
        );
        assert_ne!(now(), "");
    }

    #[test]
    fn event_fixture_implements_both_storage_event_ports() {
        let events = Events::default();
        let migration = MigrationEvent::Progress {
            migration_id: "migration".into(),
            from: 0,
            to: 1,
            current: 1,
            target: 1,
        };
        assert_eq!(MigrationEvents::prepare(&events, &migration).unwrap(), 1);
        MigrationEvents::deliver(&events, 1, migration).unwrap();
        MigrationEvents::retire(&events, "migration");
        let record = Appended {
            session_id: "session".into(),
            view_epoch: "epoch".into(),
            record_seq: 1,
            kind: "body".into(),
            turn_id: None,
        };
        assert_eq!(RecordEvents::prepare(&events, &record).unwrap(), 2);
        RecordEvents::deliver(&events, 2, record).unwrap();
    }
    fn query_fault(_: rusqlite::Error) -> Fault {
        Fault::new("test", "query failed")
    }

    #[test]
    fn timestamps_and_query_fixture_failures_keep_stable_fallbacks() {
        assert_eq!(
            format_timestamp(OffsetDateTime::UNIX_EPOCH),
            "1970-01-01T00:00:00Z"
        );
        let ancient = time::Date::from_calendar_date(-1, time::Month::January, 1)
            .unwrap()
            .midnight()
            .assume_utc();
        assert_eq!(format_timestamp(ancient), "1970-01-01T00:00:00Z");
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let fault = connection
            .execute("SELECT state FROM missing", [])
            .map_err(query_fault)
            .unwrap_err();
        assert_eq!(fault.code, "test");
    }

    #[test]
    fn invalid_provider_usage_and_cost_overflow_keep_the_reservation_unconfirmed() {
        let (storage, root) = storage();
        configure(&storage, "10");
        let port = port(storage.clone());
        for (request_id, usage) in [
            (
                "invalid-cached",
                Usage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    cached_prompt_tokens: Some(2),
                    cache_creation_tokens: Some(0),
                    reasoning_tokens: None,
                },
            ),
            (
                "cost-overflow",
                Usage {
                    prompt_tokens: u64::MAX - 1,
                    completion_tokens: 1,
                    cached_prompt_tokens: Some(0),
                    cache_creation_tokens: Some(0),
                    reasoning_tokens: None,
                },
            ),
        ] {
            let attempt = attempt("model", request_id);
            port.reserve(&attempt).unwrap();
            port.settle(&attempt, &AttemptOutcome::Settled { usage: Some(usage) });
            let state = storage
                .with_database(|connection| {
                    connection
                        .query_row(
                            "SELECT state FROM billing_physical_requests WHERE request_id=?1",
                            [request_id],
                            |row| row.get::<_, String>(0),
                        )
                        .map_err(query_fault)
                })
                .unwrap();
            assert_eq!(state, "unconfirmed");
        }
        let unlimited = LedgerBudgetPort::new(
            storage.clone(),
            "run-1".into(),
            "profile-1".into(),
            "provider".into(),
            "model".into(),
            "direct".into(),
            u64::MAX,
        );
        assert_eq!(
            unlimited
                .reserve(&attempt("model", "reserve-overflow"))
                .unwrap_err()
                .reason,
            "budget.price-missing"
        );
        storage.close().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
