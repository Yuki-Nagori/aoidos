use super::*;
use crate::{
    store_commands::{StorageEvents, StorageService},
    turn_commands::WindowEvents,
};
use aoidos_engine::billing::LedgerBudgetPort;
use std::sync::Arc;
use tauri::{
    Manager,
    test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets},
    webview::InvokeRequest,
};

fn price() -> serde_json::Value {
    serde_json::json!({
        "versionId":"price-v1", "providerId":"provider", "modelId":"model",
        "routePolicy":"direct", "currency":"CNY", "unitTokens":1000,
        "inputUncached":"1", "inputCached":"0.5", "output":"2",
        "usageMappingVersion":"v1", "sourceKind":"user-entered", "sourceUrl":null,
        "checkedAt":"2020-01-01T00:00:00Z", "effectiveFrom":"2020-01-01T00:00:00Z",
        "validUntil":null
    })
}

#[tokio::test]
async fn registered_billing_commands_cover_prices_limits_reports_and_request_pages() {
    let root = std::env::temp_dir().join(format!("aoidos-billing-ipc-{}", uuid::Uuid::new_v4()));
    let events = StorageEvents::new(Arc::new(WindowEvents::new(|_, _| Ok(()))));
    let app = mock_builder()
        .manage(StorageService::new(&root, events))
        .invoke_handler(tauri::generate_handler![
            budget_get_monthly,
            budget_get_period,
            budget_list_requests,
            budget_get_suggested_limits,
            budget_get_selected_price,
            budget_select_price,
            budget_get_settings,
            budget_set_settings,
            budget_get_price,
            budget_register_price
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
                reasoning_tokens: Some(2),
            }),
        },
    );
    app.state::<StorageService>()
        .storage
        .with_database(|connection| {
            for request_id in ["request-1", "request-2"] {
                BillingLedger::reserve(
                    connection,
                    &aoidos_engine::billing::Reservation {
                        request_id: request_id.into(),
                        run_id: "run-1".into(),
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
        (BillingError::NotFound, "app.not-found"),
        (BillingError::Conflict, "budget.conflict"),
        (BillingError::Storage, "store.database"),
    ] {
        assert_eq!(billing_fault(error).code, code);
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
