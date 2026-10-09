use super::*;
use crate::turn_commands::WindowEvents;
use tauri::{
    Manager,
    test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets},
    webview::InvokeRequest,
};

#[tokio::test]
async fn real_command_decoding_preferences_and_frozen_storage_diagnostics() {
    let dir = std::env::temp_dir().join(format!("aoidos-store-commands-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let events = StorageEvents::new(Arc::new(WindowEvents::new(|_, _| Ok(()))));
    let service = StorageService::new(&dir, events.clone());
    let header = aoidos_engine::record::format::Header {
        kind: "header".into(),
        format_version: 1,
        grammar_version: 1,
        projection_version: 1,
        script_id: "demo".into(),
        session_id: "00000000-0000-4000-8000-000000000022".into(),
        created_at: aoidos_engine::record::format::now(),
        static_prefix: "[AOIDOS:STATIC]\nx\n[/AOIDOS:STATIC]\n".into(),
        static_prefix_hash: aoidos_engine::record::format::hash(
            b"[AOIDOS:STATIC]\nx\n[/AOIDOS:STATIC]\n",
        ),
        script_revision: aoidos_engine::record::format::hash(b"script"),
    };
    service.storage.records.create(header.clone()).unwrap();
    let app = mock_builder()
        .manage(service)
        .invoke_handler(tauri::generate_handler![
            crate::store_ipc::store_get_migration,
            crate::store_ipc::store_get_ui_preferences,
            crate::store_ipc::store_set_ui_preferences,
            crate::store_ipc::engine_get_record_page,
            crate::store_ipc::engine_get_record_view,
            crate::store_ipc::engine_get_record_body
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let call = |name: &str, body: serde_json::Value| {
        get_ipc_response(
            &window,
            InvokeRequest {
                cmd: name.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: window.url().unwrap(),
                body: tauri::ipc::InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.into(),
            },
        )
        .map(|body| body.deserialize::<serde_json::Value>().unwrap())
    };
    assert_eq!(
        call("store_get_migration", serde_json::json!({})).unwrap()["phase"],
        "completed"
    );
    assert_eq!(
        call("store_get_ui_preferences", serde_json::json!({})).unwrap()["panelPinned"],
        false
    );
    assert_eq!(
        call(
            "store_set_ui_preferences",
            serde_json::json!({"panelPinned":true,"diceMode":"auto"})
        )
        .unwrap()["diceMode"],
        "auto"
    );
    assert_eq!(
        call(
            "store_set_ui_preferences",
            serde_json::json!({"panelPinned":true,"diceMode":"auto","secret":"ignored"})
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    assert_eq!(
        call(
            "store_set_ui_preferences",
            serde_json::json!({"panelPinned":true,"diceMode":"wrong"})
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    assert_eq!(
        call(
            "engine_get_record_page",
            serde_json::json!({"sessionId":header.session_id})
        )
        .unwrap()["items"],
        serde_json::json!([])
    );
    assert_eq!(
        call(
            "engine_get_record_view",
            serde_json::json!({"sessionId":header.session_id})
        )
        .unwrap()["needsRecovery"],
        false
    );
    assert_eq!(
        call(
            "engine_get_record_body",
            serde_json::json!({"sessionId":header.session_id,"bodyRef":"invalid"})
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    let session = app
        .state::<StorageService>()
        .storage
        .records
        .get(&header.session_id)
        .unwrap();
    assert_eq!(
        call(
            "engine_get_record_view",
            serde_json::json!({"sessionId":"invalid"})
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    drop(session);
    for name in [
        "engine_get_record_page",
        "engine_get_record_view",
        "engine_get_record_body",
    ] {
        assert_eq!(
            call(name, serde_json::json!({"sessionId":42})).unwrap_err()["code"],
            "app.bad-request"
        );
        assert_eq!(
            call(
                name,
                serde_json::json!({"sessionId":header.session_id,"extra":true})
            )
            .unwrap_err()["code"],
            "app.bad-request"
        );
    }
    let raw = get_ipc_response(
        &window,
        InvokeRequest {
            cmd: "store_set_ui_preferences".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: window.url().unwrap(),
            body: tauri::ipc::InvokeBody::Raw(vec![]),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.into(),
        },
    );
    assert_eq!(raw.err().unwrap()["code"], "app.bad-request");
    app.state::<StorageService>().storage.close().unwrap();
    for (name, params) in [
        ("store_get_ui_preferences", serde_json::json!({})),
        (
            "store_set_ui_preferences",
            serde_json::json!({"panelPinned":false,"diceMode":"manual"}),
        ),
        (
            "engine_get_record_page",
            serde_json::json!({"sessionId":header.session_id}),
        ),
        (
            "engine_get_record_view",
            serde_json::json!({"sessionId":header.session_id}),
        ),
        (
            "engine_get_record_body",
            serde_json::json!({"sessionId":header.session_id,"bodyRef":"opaque"}),
        ),
    ] {
        assert_eq!(call(name, params).unwrap_err()["code"], "app.not-ready");
    }
    assert_eq!(
        call("store_get_migration", serde_json::json!({})).unwrap()["phase"],
        "completed"
    );
    events.shutdown().await;
    drop(window);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
