use super::*;
use crate::{
    store_commands::{StorageEvents, StorageService},
    turn_commands::WindowEvents,
};
use std::sync::Arc;
use tauri::{
    Manager,
    test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets},
    webview::InvokeRequest,
};

#[test]
fn missing_theme_catalog_returns_not_ready() {
    let error = themes_result(None).unwrap_err();
    assert_eq!(error.code(), "app.not-ready");
}

#[tokio::test]
async fn preference_commands_reject_missing_storage_state() {
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![
            theme_get_preference,
            theme_set_preference
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    for (cmd, body) in [
        ("theme_get_preference", serde_json::json!({})),
        ("theme_set_preference", serde_json::json!({"theme":"dark"})),
    ] {
        let response = get_ipc_response(
            &window,
            InvokeRequest {
                cmd: cmd.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: window.url().unwrap(),
                body: tauri::ipc::InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.into(),
            },
        );
        assert!(response.is_err(), "{cmd} must reject absent storage state");
    }
}

#[tokio::test]
async fn registered_theme_commands_share_storage_and_return_safe_skin_payloads() {
    let dir = std::env::temp_dir().join(format!("aoidos-theme-commands-{}", std::process::id()));
    let scripts = dir.join("scripts");
    std::fs::create_dir_all(scripts.join("mistbell")).unwrap();
    std::fs::write(
        scripts.join("mistbell/theme.css"),
        ":root { --accent: #abc; }",
    )
    .unwrap();
    let events = StorageEvents::new(Arc::new(WindowEvents::new(|_, _| Ok(()))));
    let storage = StorageService::new(&dir, events.clone());
    let app = mock_builder()
        .manage(storage)
        .manage(ThemeService::new(scripts.clone()))
        .invoke_handler(tauri::generate_handler![
            theme_list,
            theme_get_preference,
            theme_set_preference,
            theme_skin_load
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
        call("theme_list", serde_json::json!({})).unwrap()[0]["id"],
        "dark"
    );
    assert_eq!(
        call("theme_get_preference", serde_json::json!({})).unwrap()["theme"],
        "dark"
    );
    assert_eq!(
        call("theme_set_preference", serde_json::json!({"theme":"light"})).unwrap()["theme"],
        "light"
    );
    assert_eq!(
        call("theme_get_preference", serde_json::json!({})).unwrap()["theme"],
        "light"
    );
    let skin = call(
        "theme_skin_load",
        serde_json::json!({"scriptId":"mistbell"}),
    )
    .unwrap();
    assert_eq!(skin["tokens"]["--accent"], "#aabbccff");
    assert!(skin.get("sourceHash").is_some());
    assert_eq!(
        call("theme_skin_load", serde_json::json!({"scriptId":"unknown"})).unwrap_err()["code"],
        "app.not-found"
    );
    assert_eq!(
        call("theme_skin_load", serde_json::json!({})).unwrap_err()["code"],
        "app.bad-request"
    );
    assert_eq!(
        call(
            "theme_set_preference",
            serde_json::json!({"theme":"system"})
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    assert_eq!(
        call(
            "theme_set_preference",
            serde_json::json!({"theme":"dark","extra":true})
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    std::fs::write(
        scripts.join("mistbell/theme.css"),
        "@import url(https://example.test/x);",
    )
    .unwrap();
    assert_eq!(
        call(
            "theme_skin_load",
            serde_json::json!({"scriptId":"mistbell"})
        )
        .unwrap_err()["code"],
        "theme.invalid-skin"
    );
    let raw = get_ipc_response(
        &window,
        InvokeRequest {
            cmd: "theme_set_preference".into(),
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
    assert_eq!(
        call("theme_get_preference", serde_json::json!({})).unwrap_err()["code"],
        "app.not-ready"
    );
    assert_eq!(
        call("theme_set_preference", serde_json::json!({"theme":"dark"})).unwrap_err()["code"],
        "app.not-ready"
    );
    events.shutdown().await;
    drop(window);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn skin_failures_map_to_contract_codes_without_internal_context() {
    let cases = [
        (skin::SkinError::UnknownScript, "app.not-found"),
        (skin::SkinError::InvalidSkin, "theme.invalid-skin"),
        (skin::SkinError::Storage, "store.io"),
    ];
    for (error, expected) in cases {
        let mapped = map_skin_error(error);
        let value = aoidos_json::to_value(&mapped).unwrap();
        assert_eq!(value["code"], expected);
        assert!(value["message"].as_str().is_some());
        assert!(value.get("detail").is_none());
    }
}

#[test]
fn skin_service_holds_only_the_registered_resource_root() {
    let service = ThemeService::new("/trusted/resources/scripts".into());
    assert_eq!(
        service.resource_root,
        PathBuf::from("/trusted/resources/scripts")
    );
}
