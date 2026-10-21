use super::*;
use crate::{
    store_commands::{StorageEvents, StorageService},
    turn_commands::WindowEvents,
};
use aoidos_engine::billing::LedgerBudgetPort;
use secrecy::ExposeSecret;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{
    Manager,
    test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets},
    webview::InvokeRequest,
};

#[tokio::test]
async fn balance_cache_is_singleflight_bounded_by_freshness_and_credential_identity() {
    use aoidos_llm::providers::{BalanceAccount, BalanceSnapshot, BalanceStatus};
    let service = BalanceService::default();
    let profile = aoidos_llm::config::LlmProfile {
        profile_id: "profile-cache".into(),
        provider_id: "deepseek".into(),
        model: "deepseek-v4.1-flash".into(),
        mode: aoidos_llm::config::ProfileMode::Chat,
        thinking: false,
        sampling: aoidos_llm::sampling::Sampling {
            temperature: 0.7,
            max_tokens: 256,
        },
        proxy: aoidos_llm::config::ProxyConfig::System,
    };
    let credential = secrecy::SecretString::from("test-key-a".to_owned());
    let key = balance_cache_key(&profile, credential.expose_secret().as_bytes());
    let other_key = balance_cache_key(&profile, b"test-key-b");
    assert_ne!(key, other_key);
    let now = Instant::now();
    service.cache.lock().await.insert(
        key.clone(),
        CachedBalance {
            snapshot: BalanceSnapshot {
                provider_id: "deepseek".into(),
                queried_at: "2026-10-11T00:00:00Z".into(),
                stale: false,
                status: BalanceStatus::Available,
                accounts: vec![BalanceAccount {
                    currency: "CNY".into(),
                    total: "10".into(),
                    granted: "0".into(),
                    topped_up: "10".into(),
                }],
            },
            updated_at: now - Duration::from_secs(6 * 60),
            retry_after: now + Duration::from_secs(30),
            failure_count: 1,
        },
    );
    let client = aoidos_llm::proxy::build_client(
        &aoidos_llm::config::ProxyConfig::System,
        &aoidos_llm::proxy::SystemProxySnapshot::default(),
        None,
    )
    .unwrap();
    let cached = service
        .query(key, &client, &credential, "unused".into())
        .await;
    assert_eq!(cached.status, BalanceStatus::Available);
    assert!(cached.stale);
    assert_eq!(cached.queried_at, "2026-10-11T00:00:00Z");
}

#[test]
fn balance_cache_evicts_old_entries_at_its_hard_limit() {
    use aoidos_llm::providers::{BalanceSnapshot, BalanceStatus};
    let now = Instant::now();
    let mut cache = HashMap::new();
    for index in 0..MAX_BALANCE_CACHE_ENTRIES {
        cache.insert(
            format!("old-{index}"),
            CachedBalance {
                snapshot: BalanceSnapshot {
                    provider_id: "deepseek".into(),
                    queried_at: "checked".into(),
                    stale: false,
                    status: BalanceStatus::Unknown,
                    accounts: Vec::new(),
                },
                updated_at: now + Duration::from_secs(index as u64),
                retry_after: now,
                failure_count: 1,
            },
        );
    }
    insert_balance_cache(
        &mut cache,
        "new-profile".into(),
        CachedBalance {
            snapshot: BalanceSnapshot {
                provider_id: "deepseek".into(),
                queried_at: "checked".into(),
                stale: false,
                status: BalanceStatus::Unknown,
                accounts: Vec::new(),
            },
            updated_at: now,
            retry_after: now,
            failure_count: 1,
        },
    );
    assert_eq!(cache.len(), MAX_BALANCE_CACHE_ENTRIES);
    assert!(!cache.contains_key("old-0"));
    assert!(cache.contains_key("new-profile"));
}

fn price() -> serde_json::Value {
    serde_json::json!({
        "versionId":"price-v1", "providerId":"provider", "modelId":"model",
        "routePolicy":"direct", "currency":"CNY", "unitTokens":1000,
        "inputUncached":"1", "inputCached":"0.5", "inputCachedWrite":"0.25", "output":"2",
        "usageMappingVersion":"v1", "sourceKind":"user-entered", "sourceUrl":null,
        "checkedAt":"2020-01-01T00:00:00Z", "effectiveFrom":"2020-01-01T00:00:00Z",
        "validUntil":null
    })
}

#[tokio::test]
async fn registered_billing_commands_cover_prices_limits_reports_and_request_pages() {
    let root = std::env::temp_dir().join(format!("aoidos-billing-ipc-{}", uuid::Uuid::new_v4()));
    crate::llm_commands::init_llm_dir(root.join("profiles"));
    let events = StorageEvents::new(Arc::new(WindowEvents::new(|_, _| Ok(()))));
    let app = mock_builder()
        .manage(StorageService::new(&root, events))
        .manage(BalanceService::default())
        .manage(
            crate::turn_commands::TurnService::new(
                Arc::new(WindowEvents::new(|_, _| Ok(()))),
                aoidos_llm::proxy::SystemProxySnapshot::default(),
            )
            .unwrap(),
        )
        .invoke_handler(tauri::generate_handler![
            budget_get_monthly,
            budget_get_period,
            budget_list_requests,
            budget_retry_settlement,
            budget_preview_backfill,
            budget_commit_backfill,
            budget_get_provider_balance,
            budget_get_suggested_limits,
            budget_get_selected_price,
            budget_select_price,
            budget_get_settings,
            budget_set_settings,
            budget_get_price,
            budget_register_price,
            budget_confirm_deepseek_price,
            budget_get_deepseek_price_templates,
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let call = |name: &str, body: tauri::ipc::InvokeBody| {
        get_ipc_response(
            &window,
            InvokeRequest {
                cmd: name.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: window.url().unwrap(),
                body,
                headers: Default::default(),
                invoke_key: INVOKE_KEY.into(),
            },
        )
        .map(|body| body.deserialize::<serde_json::Value>().unwrap())
    };

    let valid_requests = [
        (
            "budget_get_monthly",
            serde_json::json!({"month":"2026-10","detectedTimeZone":"UTC"}),
        ),
        ("budget_get_period", serde_json::json!({"runId":"run-1"})),
        ("budget_list_requests", serde_json::json!({"runId":"run-1"})),
        (
            "budget_retry_settlement",
            serde_json::json!({"runId":"run-1","requestId":"request-1"}),
        ),
        (
            "budget_preview_backfill",
            serde_json::json!({"runId":"run-1","requestId":"request-1"}),
        ),
        (
            "budget_commit_backfill",
            serde_json::json!({"runId":"run-1","requestId":"request-1","operationId":"op-1","expectedRevision":0}),
        ),
        (
            "budget_get_suggested_limits",
            serde_json::json!({"providerId":"provider","modelId":"model","routePolicy":"direct"}),
        ),
        (
            "budget_get_selected_price",
            serde_json::json!({"profileId":"profile","providerId":"provider","modelId":"model","routePolicy":"direct"}),
        ),
        (
            "budget_select_price",
            serde_json::json!({"profileId":"profile","providerId":"provider","modelId":"model","routePolicy":"direct","currency":"CNY"}),
        ),
        ("budget_get_settings", serde_json::json!({"runId":"run-1"})),
        (
            "budget_set_settings",
            serde_json::json!({"runId":"run-1","cnyLimit":"10","usdLimit":"5"}),
        ),
        (
            "budget_get_price",
            serde_json::json!({"providerId":"provider","modelId":"model","routePolicy":"direct","currency":"CNY"}),
        ),
        (
            "budget_register_price",
            serde_json::json!({"price":price()}),
        ),
        (
            "budget_confirm_deepseek_price",
            serde_json::json!({"profileId":"deepseek-profile","modelId":"deepseek-flash","currency":"CNY"}),
        ),
    ];
    for (name, valid) in &valid_requests {
        for body in [
            tauri::ipc::InvokeBody::Json(serde_json::json!({})),
            tauri::ipc::InvokeBody::Raw(vec![]),
        ] {
            assert_eq!(
                call(name, body).unwrap_err(),
                serde_json::json!({"code":"app.bad-request","message":"费用参数不合法"}),
                "{name}"
            );
        }
        let mut unknown = valid.clone();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), serde_json::Value::Null);
        assert_eq!(
            call(name, tauri::ipc::InvokeBody::Json(unknown)).unwrap_err()["code"],
            "app.bad-request",
            "{name}"
        );
    }
    let mut unknown_balance_args = serde_json::json!({"profileId":"profile-1"});
    unknown_balance_args
        .as_object_mut()
        .unwrap()
        .insert("unexpected".into(), serde_json::Value::Null);
    assert_eq!(
        call(
            "budget_get_provider_balance",
            tauri::ipc::InvokeBody::Json(unknown_balance_args),
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );

    assert_eq!(
        call(
            "budget_get_settings",
            tauri::ipc::InvokeBody::Json(serde_json::json!({"runId":"run-1"}))
        )
        .unwrap(),
        serde_json::Value::Null
    );
    assert_eq!(
        call(
            "budget_get_settings",
            tauri::ipc::InvokeBody::Json(serde_json::json!({"runId":""}))
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    assert_eq!(
        call(
            "budget_get_period",
            tauri::ipc::InvokeBody::Json(serde_json::json!({"runId":"run-1"}))
        )
        .unwrap()["runId"],
        "run-1"
    );
    assert_eq!(
        call(
            "budget_list_requests",
            tauri::ipc::InvokeBody::Json(serde_json::json!({"runId":"run-1"}))
        )
        .unwrap()["items"],
        serde_json::json!([])
    );
    assert_eq!(
        call(
            "budget_preview_backfill",
            tauri::ipc::InvokeBody::Json(
                serde_json::json!({"runId":"run-1","requestId":"unknown"})
            )
        )
        .unwrap(),
        serde_json::Value::Null
    );
    assert_eq!(
        call(
            "budget_get_monthly",
            tauri::ipc::InvokeBody::Json(
                serde_json::json!({"month":"2026-10","detectedTimeZone":"UTC"})
            )
        )
        .unwrap()["timeZone"],
        "UTC"
    );
    assert_eq!(call("budget_get_suggested_limits", tauri::ipc::InvokeBody::Json(serde_json::json!({"providerId":"provider","modelId":"model","routePolicy":"direct"}))).unwrap()["cny"], serde_json::Value::Null);
    assert_eq!(call("budget_get_selected_price", tauri::ipc::InvokeBody::Json(serde_json::json!({"profileId":"profile","providerId":"provider","modelId":"model","routePolicy":"direct"}))).unwrap()["price"], serde_json::Value::Null);
    assert_eq!(call("budget_get_price", tauri::ipc::InvokeBody::Json(serde_json::json!({"providerId":"provider","modelId":"model","routePolicy":"direct","currency":"CNY"}))).unwrap()["price"], serde_json::Value::Null);
    let templates = call(
        "budget_get_deepseek_price_templates",
        tauri::ipc::InvokeBody::Json(serde_json::json!({})),
    )
    .unwrap();
    assert_eq!(templates.as_array().unwrap().len(), 2);
    assert_eq!(templates[0]["currency"], "CNY");
    assert_eq!(templates[0]["checkedAt"], "2026-10-21T00:00:00Z");

    let default_price = call(
        "budget_confirm_deepseek_price",
        tauri::ipc::InvokeBody::Json(
            serde_json::json!({"profileId":"deepseek-profile","modelId":"deepseek-flash","currency":"CNY"}),
        ),
    )
    .unwrap();
    assert_eq!(default_price["currency"], "CNY");
    assert_eq!(default_price["providerId"], "deepseek");
    let selected_price = call(
        "budget_get_selected_price",
        tauri::ipc::InvokeBody::Json(serde_json::json!({"profileId":"deepseek-profile","providerId":"deepseek","modelId":"deepseek-flash","routePolicy":"direct"})),
    )
    .unwrap();
    assert_eq!(selected_price["price"]["currency"], "CNY");

    let registered = call(
        "budget_register_price",
        tauri::ipc::InvokeBody::Json(serde_json::json!({"price":price()})),
    )
    .unwrap();
    assert_eq!(registered["versionId"], "price-v1");
    assert_eq!(call("budget_get_price", tauri::ipc::InvokeBody::Json(serde_json::json!({"providerId":"provider","modelId":"model","routePolicy":"direct","currency":"CNY"}))).unwrap()["price"]["versionId"], "price-v1");
    assert_eq!(call("budget_get_suggested_limits", tauri::ipc::InvokeBody::Json(serde_json::json!({"providerId":"provider","modelId":"model","routePolicy":"direct"}))).unwrap()["cny"], "3000");
    assert_eq!(call("budget_select_price", tauri::ipc::InvokeBody::Json(serde_json::json!({"profileId":"profile","providerId":"provider","modelId":"model","routePolicy":"direct","currency":"CNY"}))).unwrap()["versionId"], "price-v1");
    assert_eq!(call("budget_get_selected_price", tauri::ipc::InvokeBody::Json(serde_json::json!({"profileId":"profile","providerId":"provider","modelId":"model","routePolicy":"direct"}))).unwrap()["price"]["versionId"], "price-v1");

    let settings = call(
        "budget_set_settings",
        tauri::ipc::InvokeBody::Json(
            serde_json::json!({"runId":"run-1","cnyLimit":"10","usdLimit":"5"}),
        ),
    )
    .unwrap();
    assert_eq!(settings["revision"], 1);
    assert_eq!(
        call(
            "budget_get_settings",
            tauri::ipc::InvokeBody::Json(serde_json::json!({"runId":"run-1"}))
        )
        .unwrap()["usdLimit"],
        "5"
    );
    let budget = LedgerBudgetPort::new(
        app.state::<StorageService>().storage.clone(),
        "run-1".into(),
        "profile".into(),
        "provider".into(),
        "model".into(),
        "direct".into(),
        100,
    );
    let attempt = aoidos_llm::schedule::AttemptIdentity {
        request_id: "budget-port".into(),
        turn_id: "turn-1".into(),
        attempt: 1,
        kind: aoidos_llm::schedule::AttemptKind::Initial,
        model: "model".into(),
        mode: aoidos_llm::provider::RequestMode::Completion,
        max_tokens: 20,
        temperature: 1.0,
    };
    aoidos_llm::schedule::BudgetPort::reserve(&budget, &attempt).unwrap();
    aoidos_llm::schedule::BudgetPort::settle(
        &budget,
        &attempt,
        &aoidos_llm::schedule::AttemptOutcome::Settled {
            usage: Some(aoidos_llm::provider::Usage {
                prompt_tokens: 50,
                completion_tokens: 10,
                cached_prompt_tokens: Some(10),
                cache_creation_tokens: Some(0),
                reasoning_tokens: Some(2),
            }),
        },
    );
    let legacy_attempt = aoidos_llm::schedule::AttemptIdentity {
        request_id: "legacy-backfill-ipc".into(),
        ..attempt.clone()
    };
    aoidos_llm::schedule::BudgetPort::reserve(&budget, &legacy_attempt).unwrap();
    app.state::<StorageService>()
        .storage
        .with_database(|connection| {
            connection
                .execute(
                    "UPDATE billing_physical_requests SET state='unconfirmed',input_total=50,input_cached=5,input_cached_write=0,output_total=10,reasoning_included=2 WHERE request_id='legacy-backfill-ipc'",
                    [],
                )
                .map_err(|error| Fault::new("test", error.to_string()))?;
            Ok(())
        })
        .unwrap();
    let preview = call(
        "budget_preview_backfill",
        tauri::ipc::InvokeBody::Json(serde_json::json!({
            "runId":"run-1", "requestId":"legacy-backfill-ipc"
        })),
    )
    .unwrap();
    assert_eq!(preview["requestId"], "legacy-backfill-ipc");
    for command in ["budget_preview_backfill", "budget_commit_backfill"] {
        let mut args =
            serde_json::json!({"runId":"different-run", "requestId":"legacy-backfill-ipc"});
        if command == "budget_commit_backfill" {
            args["operationId"] = serde_json::json!("ipc-backfill-op");
            args["expectedRevision"] = preview["ledgerRevision"].clone();
        }
        assert_eq!(
            call(command, tauri::ipc::InvokeBody::Json(args)).unwrap_err()["code"],
            "app.not-found"
        );
    }
    assert_eq!(
        call(
            "budget_commit_backfill",
            tauri::ipc::InvokeBody::Json(serde_json::json!({
                "runId":"run-1", "requestId":"unknown", "operationId":"absent", "expectedRevision":0
            }))
        )
        .unwrap_err()["code"],
        "app.not-found"
    );
    assert_eq!(
        call(
            "budget_commit_backfill",
            tauri::ipc::InvokeBody::Json(serde_json::json!({
                "runId":"run-1", "requestId":"legacy-backfill-ipc", "operationId":"stale",
                "expectedRevision":preview["ledgerRevision"].as_u64().unwrap() + 1
            }))
        )
        .unwrap_err()["detail"]["reason"],
        "staleRevision"
    );

    call(
        "budget_commit_backfill",
        tauri::ipc::InvokeBody::Json(serde_json::json!({
            "runId":"run-1", "requestId":"legacy-backfill-ipc",
            "operationId":"ipc-backfill-op", "expectedRevision":preview["ledgerRevision"]
        })),
    )
    .unwrap();

    // 成功重放可携带最初的revision，既不重复结算也不改变账本。
    for operation_id in ["ipc-backfill-op", "different-op"] {
        let result = call(
            "budget_commit_backfill",
            tauri::ipc::InvokeBody::Json(serde_json::json!({
                "runId":"run-1", "requestId":"legacy-backfill-ipc", "operationId":operation_id,
                "expectedRevision":preview["ledgerRevision"]
            })),
        );
        if operation_id == "ipc-backfill-op" {
            assert!(result.is_ok());
        } else {
            assert_eq!(result.unwrap_err()["code"], "budget.conflict");
        }
    }
    assert_eq!(
        call(
            "budget_get_provider_balance",
            tauri::ipc::InvokeBody::Json(serde_json::json!({"profileId":""}))
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    let backfilled = call(
        "budget_list_requests",
        tauri::ipc::InvokeBody::Json(serde_json::json!({"runId":"run-1"})),
    )
    .unwrap();
    let item = backfilled["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["requestId"] == "legacy-backfill-ipc")
        .unwrap();
    assert_eq!(item["state"], "settled");
    assert_eq!(item["actualAmount"], preview["estimatedAmount"]);

    let retry_attempt = aoidos_llm::schedule::AttemptIdentity {
        request_id: "retry-ipc".into(),
        ..attempt.clone()
    };
    aoidos_llm::schedule::BudgetPort::reserve(&budget, &retry_attempt).unwrap();
    app.state::<StorageService>()
        .storage
        .with_database(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER fail_billing_settlement BEFORE UPDATE OF state ON billing_physical_requests WHEN NEW.state='settled' BEGIN SELECT RAISE(ABORT,'injected'); END;",
                )
                .map_err(|error| Fault::new("test", error.to_string()))?;
            Ok(())
        })
        .unwrap();
    aoidos_llm::schedule::BudgetPort::settle(
        &budget,
        &retry_attempt,
        &aoidos_llm::schedule::AttemptOutcome::Settled {
            usage: Some(aoidos_llm::provider::Usage {
                prompt_tokens: 80,
                completion_tokens: 20,
                cached_prompt_tokens: Some(0),
                cache_creation_tokens: Some(0),
                reasoning_tokens: None,
            }),
        },
    );
    app.state::<StorageService>()
        .storage
        .with_database(|connection| {
            connection
                .execute_batch("DROP TRIGGER fail_billing_settlement")
                .map_err(|error| Fault::new("test", error.to_string()))?;
            Ok(())
        })
        .unwrap();
    let pending_page = call(
        "budget_list_requests",
        tauri::ipc::InvokeBody::Json(serde_json::json!({"runId":"run-1"})),
    )
    .unwrap();
    let retry_row = pending_page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["requestId"] == "retry-ipc")
        .unwrap();
    assert_eq!(retry_row["settlementPending"], true);
    assert_eq!(retry_row["diagnosticCode"], "store.database");
    assert_eq!(
        call(
            "budget_retry_settlement",
            tauri::ipc::InvokeBody::Json(
                serde_json::json!({"runId":"another-run","requestId":"retry-ipc"})
            )
        )
        .unwrap_err()["code"],
        "app.not-found"
    );
    assert!(
        call(
            "budget_retry_settlement",
            tauri::ipc::InvokeBody::Json(
                serde_json::json!({"runId":"run-1","requestId":"retry-ipc"})
            )
        )
        .unwrap()
        .is_null()
    );
    for request_id in ["missing-request", "retry-ipc"] {
        assert_eq!(
            call(
                "budget_retry_settlement",
                tauri::ipc::InvokeBody::Json(
                    serde_json::json!({"runId":"run-1","requestId":request_id})
                )
            )
            .unwrap_err()["code"],
            "app.not-found"
        );
    }
    let recovered = call(
        "budget_list_requests",
        tauri::ipc::InvokeBody::Json(serde_json::json!({"runId":"run-1"})),
    )
    .unwrap();
    let recovered_row = recovered["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["requestId"] == "retry-ipc")
        .unwrap();
    assert_eq!(recovered_row["state"], "settled");
    assert_eq!(recovered_row["settlementPending"], false);
    app.state::<StorageService>()
        .storage
        .with_database(|connection| {
            for request_id in ["request-1", "request-2"] {
                BillingLedger::reserve(
                    connection,
                    &aoidos_engine::billing::Reservation {
                        request_id: request_id.into(),
                        run_id: "run-1".into(),
                        legacy_scope_id: None,
                        turn_id: "turn-1".into(),
                        profile_id: "profile".into(),
                        attempt: 1,
                        provider_id: "provider".into(),
                        model_id: "model".into(),
                        route_policy: "direct".into(),
                        price_version_id: "price-v1".into(),
                        currency: Currency::Cny,
                        dispatch_at: "2026-10-11T00:00:00Z".into(),
                        amount: NanoMoney::parse("1").unwrap(),
                    },
                )
                .map_err(|error| Fault::new("test", error.to_string()))?;
            }
            Ok(())
        })
        .unwrap();
    let first_page = call(
        "budget_list_requests",
        tauri::ipc::InvokeBody::Json(serde_json::json!({"runId":"run-1","limit":1})),
    )
    .unwrap();
    let cursor = first_page["nextCursor"].as_str().unwrap();
    app.state::<StorageService>()
        .storage
        .with_database(|connection| {
            BillingLedger::reserve(
                connection,
                &aoidos_engine::billing::Reservation {
                    request_id: "request-3".into(),
                    run_id: "run-1".into(),
                    legacy_scope_id: None,
                    turn_id: "turn-1".into(),
                    profile_id: "profile".into(),
                    attempt: 2,
                    provider_id: "provider".into(),
                    model_id: "model".into(),
                    route_policy: "direct".into(),
                    price_version_id: "price-v1".into(),
                    currency: Currency::Cny,
                    dispatch_at: "2026-10-11T00:00:00Z".into(),
                    amount: NanoMoney::parse("1").unwrap(),
                },
            )
            .map_err(|error| Fault::new("test", error.to_string()))
        })
        .unwrap();
    assert_eq!(
        call(
            "budget_list_requests",
            tauri::ipc::InvokeBody::Json(
                serde_json::json!({"runId":"run-1","cursor":cursor,"limit":1})
            )
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    assert_eq!(
        call(
            "budget_list_requests",
            tauri::ipc::InvokeBody::Json(serde_json::json!({"runId":"run-1","limit":201}))
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    assert_eq!(
        call(
            "budget_set_settings",
            tauri::ipc::InvokeBody::Json(
                serde_json::json!({"runId":"run-1","cnyLimit":"invalid","usdLimit":"1"})
            )
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    assert_eq!(
        call(
            "budget_register_price",
            tauri::ipc::InvokeBody::Json(serde_json::json!({"price":{}}))
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    assert_eq!(
        call(
            "budget_get_settings",
            tauri::ipc::InvokeBody::Raw(vec![1, 2, 3])
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );

    assert_eq!(
        call(
            "budget_set_settings",
            tauri::ipc::InvokeBody::Json(
                serde_json::json!({"runId":"run-1","cnyLimit":"1","usdLimit":"invalid"})
            )
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    let mut invalid_price = price();
    invalid_price["unitTokens"] = serde_json::json!(0);
    assert_eq!(
        call(
            "budget_register_price",
            tauri::ipc::InvokeBody::Json(serde_json::json!({"price":invalid_price}))
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );

    // 报表第二次查询及其余账本访问的数据库故障必须保留公共错误形状。
    let storage = &app.state::<StorageService>().storage;
    storage
        .with_database(|connection| {
            connection
                .execute_batch("ALTER TABLE billing_budget_warnings RENAME TO hidden_warnings;")
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        call(
            "budget_get_period",
            tauri::ipc::InvokeBody::Json(serde_json::json!({"runId":"run-1"}))
        )
        .unwrap_err()["code"],
        "store.database"
    );

    storage
        .with_database(|connection| {
            connection
                .execute_batch("ALTER TABLE billing_physical_requests RENAME TO hidden_requests;")
                .unwrap();
            Ok(())
        })
        .unwrap();
    for name in ["budget_get_period", "budget_get_monthly"] {
        let body = valid_requests
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .unwrap()
            .1
            .clone();
        assert_eq!(
            call(name, tauri::ipc::InvokeBody::Json(body)).unwrap_err()["code"],
            "store.database",
            "{name}"
        );
    }
    storage.with_database(|connection| { connection.execute_batch("ALTER TABLE billing_price_versions RENAME TO hidden_prices; ALTER TABLE billing_active_profile_prices RENAME TO hidden_profiles; ALTER TABLE billing_meta RENAME TO hidden_meta; ALTER TABLE billing_run_budgets RENAME TO hidden_budgets;").unwrap(); Ok(()) }).unwrap();
    for (name, body) in &valid_requests {
        assert_eq!(
            call(name, tauri::ipc::InvokeBody::Json(body.clone())).unwrap_err()["code"],
            "store.database",
            "{name}"
        );
    }

    app.state::<StorageService>().storage.close().unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn billing_error_helpers_keep_stable_public_codes_and_storage_job_errors() {
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
        (BillingError::SettlementBacklog, "budget.settlement-backlog"),
        (BillingError::NotFound, "app.not-found"),
        (BillingError::Conflict, "budget.conflict"),
        (
            BillingError::ReconciliationRequired,
            "budget.reconciliation-required",
        ),
        (BillingError::Storage, "store.database"),
    ] {
        assert_eq!(app_billing_fault(error).code, code);
    }
    assert_eq!(bad_amount().code(), "app.bad-request");
    assert!(!now().is_empty());
    let ancient = time::Date::from_calendar_date(-1, time::Month::January, 1)
        .unwrap()
        .midnight()
        .assume_utc();
    assert_eq!(format_timestamp(ancient), "1970-01-01T00:00:00Z");
    assert_eq!(storage_job(|| Ok::<_, Fault>(7)).await.unwrap(), 7);
    assert_eq!(
        storage_job(|| Err::<(), _>(Fault::new("store.database", "fixture")))
            .await
            .unwrap_err()
            .code(),
        "store.database"
    );
}

fn balance_snapshot(
    status: aoidos_llm::providers::BalanceStatus,
) -> aoidos_llm::providers::BalanceSnapshot {
    aoidos_llm::providers::BalanceSnapshot {
        provider_id: "deepseek".into(),
        queried_at: "2026-10-11T00:00:00Z".into(),
        stale: false,
        status,
        accounts: Vec::new(),
    }
}

#[tokio::test]
async fn balance_cache_refreshes_preserves_prior_success_and_caps_failure_backoff() {
    use aoidos_llm::providers::BalanceStatus;
    let service = BalanceService::default();
    let key = "fixture".to_owned();
    let snapshot = service
        .query_with(
            key.clone(),
            Box::pin(async { balance_snapshot(BalanceStatus::Available) }),
        )
        .await;
    assert_eq!(snapshot.status, BalanceStatus::Available);
    {
        let mut cache = service.cache.lock().await;
        let entry = cache.get_mut(&key).unwrap();
        // 缓存仍新鲜时，即使退避已结束也不会重复请求。
        entry.retry_after = Instant::now();
    }
    let cached = service
        .query_with(
            key.clone(),
            Box::pin(async { panic!("fresh cache must not fetch") }),
        )
        .await;
    assert!(!cached.stale);
    {
        let mut cache = service.cache.lock().await;
        let entry = cache.get_mut(&key).unwrap();
        entry.updated_at = Instant::now() - BALANCE_FRESH_FOR;
    }
    let stale = service
        .query_with(
            key.clone(),
            Box::pin(async { balance_snapshot(BalanceStatus::Unknown) }),
        )
        .await;
    assert!(stale.stale);
    assert_eq!(stale.status, BalanceStatus::Available);
    {
        let mut cache = service.cache.lock().await;
        let entry = cache.get_mut(&key).unwrap();
        entry.retry_after = Instant::now();
        entry.failure_count = u32::MAX;
    }
    service
        .query_with(
            key.clone(),
            Box::pin(async { balance_snapshot(BalanceStatus::Unknown) }),
        )
        .await;
    let cache = service.cache.lock().await;
    let entry = cache.get(&key).unwrap();
    assert_eq!(entry.failure_count, u32::MAX);
    assert!(entry.retry_after.duration_since(Instant::now()) <= BALANCE_MAX_BACKOFF);
    assert!(
        entry.retry_after.duration_since(Instant::now())
            > BALANCE_MAX_BACKOFF - Duration::from_secs(1)
    );
    drop(cache);

    let unknown = service
        .query_with(
            "unknown".into(),
            Box::pin(async { balance_snapshot(BalanceStatus::Unknown) }),
        )
        .await;
    assert_eq!(unknown.status, BalanceStatus::Unknown);
    {
        service
            .cache
            .lock()
            .await
            .get_mut("unknown")
            .unwrap()
            .retry_after = Instant::now();
    }
    assert_eq!(
        service
            .query_with(
                "unknown".into(),
                Box::pin(async { balance_snapshot(BalanceStatus::Unknown) })
            )
            .await
            .status,
        BalanceStatus::Unknown
    );
    // 恢复成功清空连续失败计数。
    service
        .cache
        .lock()
        .await
        .get_mut("unknown")
        .unwrap()
        .retry_after = Instant::now();
    assert_eq!(
        service
            .query_with(
                "unknown".into(),
                Box::pin(async { balance_snapshot(BalanceStatus::Unavailable) })
            )
            .await
            .status,
        BalanceStatus::Unavailable
    );
    assert_eq!(service.cache.lock().await["unknown"].failure_count, 0);
}

#[tokio::test]
async fn balance_cache_singleflight_fetches_once_for_concurrent_queries() {
    use aoidos_llm::providers::BalanceStatus;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let service = BalanceService::default();
    let calls = AtomicUsize::new(0);
    let fetch = || async {
        calls.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        balance_snapshot(BalanceStatus::Available)
    };
    let (first, second) = tokio::join!(
        service.query_with("same".into(), Box::pin(fetch())),
        service.query_with("same".into(), Box::pin(fetch()))
    );
    assert_eq!(first.status, second.status);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn balance_profile_validation_and_cached_read_do_not_access_the_vault() {
    use aoidos_llm::providers::BalanceStatus;
    let mut frozen = aoidos_llm::config::FrozenProfile {
        profile: aoidos_llm::config::LlmProfile {
            profile_id: "fixture".into(),
            provider_id: "opencode-zen".into(),
            model: "deepseek-v4.1-flash".into(),
            mode: aoidos_llm::config::ProfileMode::Chat,
            thinking: false,
            sampling: aoidos_llm::sampling::Sampling {
                temperature: 0.7,
                max_tokens: 256,
            },
            proxy: aoidos_llm::config::ProxyConfig::None,
        },
        credential: None,
    };
    let service = BalanceService::default();
    let proxy = aoidos_llm::proxy::SystemProxySnapshot::default();
    struct FixtureProfiles<'a>(&'a aoidos_llm::config::FrozenProfile);
    impl aoidos_engine::game::product::ProfileSource for FixtureProfiles<'_> {
        fn freeze(&self, id: &str) -> Result<aoidos_engine::game::product::ResolvedProfile, Fault> {
            assert_eq!(id, self.0.profile.profile_id);
            Ok(aoidos_engine::game::product::ResolvedProfile {
                frozen: aoidos_llm::config::FrozenProfile {
                    profile: self.0.profile.clone(),
                    credential: self.0.credential.clone(),
                },
                proxy_auth: None,
            })
        }
    }
    assert_eq!(
        query_saved_balance(&service, &proxy, "fixture", &FixtureProfiles(&frozen))
            .await
            .unwrap_err()
            .code(),
        "app.bad-request"
    );

    assert_eq!(
        query_provider_balance(&service, &proxy, &frozen, None)
            .await
            .unwrap_err()
            .code(),
        "app.bad-request"
    );
    frozen.profile.provider_id = "deepseek".into();
    assert_eq!(
        query_provider_balance(&service, &proxy, &frozen, None)
            .await
            .unwrap_err()
            .code(),
        "llm.missing-key"
    );
    frozen.credential = Some(secrecy::SecretString::from("fixture-key".to_owned()));
    let key = balance_cache_key(&frozen.profile, b"fixture-key");
    let now = Instant::now();
    service.cache.lock().await.insert(
        key,
        CachedBalance {
            snapshot: balance_snapshot(BalanceStatus::Available),
            updated_at: now,
            retry_after: now + BALANCE_FRESH_FOR,
            failure_count: 0,
        },
    );
    assert_eq!(
        query_provider_balance(&service, &proxy, &frozen, None)
            .await
            .unwrap()
            .status,
        BalanceStatus::Available
    );
    frozen.profile.proxy = aoidos_llm::config::ProxyConfig::Manual {
        url: "http://[invalid".into(),
        auth_ref: None,
    };
    assert_eq!(
        query_provider_balance(&service, &proxy, &frozen, None)
            .await
            .unwrap_err()
            .code(),
        "app.bad-request"
    );
}
