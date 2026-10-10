use super::*;

#[test]
fn native_titles_are_loaded_from_the_shared_locale_resources() {
    assert_eq!(native_message(Locale::ZhHans, "windowTitle"), "Aoidos");
    assert_eq!(native_message(Locale::En, "windowTitle"), "Aoidos");
    assert_eq!(native_message(Locale::ZhHans, "credentialSave"), "保存");
    assert_eq!(native_message(Locale::En, "credentialSave"), "Save");
}

use crate::{store_commands::StorageEvents, turn_commands::WindowEvents};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tauri::{
    Manager,
    test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets},
    webview::InvokeRequest,
};

struct TestNative(Arc<AtomicBool>);
impl NativeLocale for TestNative {
    fn apply(&self, _: Locale) -> NativeStatus {
        if self.0.load(Ordering::SeqCst) {
            NativeStatus::Applied
        } else {
            NativeStatus::Pending
        }
    }
    fn window_menu(&self) -> Menu<tauri::Wry> {
        panic!("test adapter has no OS menu")
    }
}

#[test]
fn committed_locale_survives_pending_native_updates_and_poisoned_lock() {
    let applied = Arc::new(AtomicBool::new(false));
    let service = LocaleService::new(Locale::En, Box::new(TestNative(applied.clone())));
    let preference = LocalePreference {
        version: 1,
        locale: aoidos_locale::preference::LocaleChoice::ZhHans,
    };
    assert_eq!(
        result(&service, preference).native_status,
        NativeStatus::Pending
    );
    assert_eq!(service.current(), Locale::ZhHans);
    applied.store(true, Ordering::SeqCst);
    assert_eq!(
        result(&service, preference).native_status,
        NativeStatus::Applied
    );
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = service.current.lock().unwrap();
        panic!("poison lock");
    }));
    assert_eq!(service.current(), Locale::ZhHans);
    service.confirm(Locale::En);
    assert_eq!(service.current(), Locale::En);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| service.window_menu())).is_err()
    );
    assert_eq!(native_message(Locale::En, "missing"), "Aoidos");
    assert_eq!(native_prompt_text(Locale::ZhHans).cancel, "取消");
    assert_eq!(native_prompt_text(Locale::En).save, "Save");
}

#[tokio::test]
async fn registered_locale_commands_share_storage_and_preserve_commit_on_native_failure() {
    let dir = std::env::temp_dir().join(format!("aoidos-locale-ipc-{}", std::process::id()));
    let events = StorageEvents::new(Arc::new(WindowEvents::new(|_, _| Ok(()))));
    let applied = Arc::new(AtomicBool::new(true));
    let app = mock_builder()
        .manage(StorageService::new(&dir, events.clone()))
        .manage(LocaleService::new(
            Locale::En,
            Box::new(TestNative(applied.clone())),
        ))
        .invoke_handler(tauri::generate_handler![
            locale_get_preference,
            locale_set_preference
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
    assert_eq!(
        call("locale_get_preference", serde_json::json!({}).into()).unwrap()["preference"]["locale"],
        "system"
    );
    for body in [
        serde_json::json!({}),
        serde_json::json!({"preference":{"version":2,"locale":"en"}}),
        serde_json::json!({"preference":{"version":1,"locale":"fr"}}),
        serde_json::json!({"preference":{"version":1,"locale":"en"},"extra":true}),
    ] {
        assert_eq!(
            call("locale_set_preference", body.into()).unwrap_err()["code"],
            "app.bad-request"
        );
    }
    assert_eq!(
        call("locale_set_preference", tauri::ipc::InvokeBody::Raw(vec![])).unwrap_err()["code"],
        "app.bad-request"
    );
    applied.store(false, Ordering::SeqCst);
    let response = call(
        "locale_set_preference",
        serde_json::json!({"preference":{"version":1,"locale":"zh-Hans"}}).into(),
    )
    .unwrap();
    assert_eq!(response["nativeStatus"], "pending");
    assert_eq!(response["resolvedLocale"], "zh-Hans");
    applied.store(true, Ordering::SeqCst);
    assert_eq!(
        call("locale_get_preference", serde_json::json!({}).into()).unwrap()["nativeStatus"],
        "applied"
    );
    app.state::<StorageService>()
        .storage
        .with_database(|connection| {
            connection
                .execute_batch("DROP TABLE locale_preference")
                .unwrap();
            Ok(())
        })
        .unwrap();
    for (name, body) in [
        ("locale_get_preference", serde_json::json!({})),
        (
            "locale_set_preference",
            serde_json::json!({"preference":{"version":1,"locale":"en"}}),
        ),
    ] {
        assert!(
            call(name, body.into()).unwrap_err()["code"]
                .as_str()
                .unwrap()
                .starts_with("store.")
        );
    }
    assert_eq!(app.state::<LocaleService>().current(), Locale::ZhHans);
    app.state::<StorageService>().storage.close().unwrap();
    for (name, body) in [
        ("locale_get_preference", serde_json::json!({})),
        (
            "locale_set_preference",
            serde_json::json!({"preference":{"version":1,"locale":"en"}}),
        ),
    ] {
        assert_eq!(
            call(name, body.into()).unwrap_err()["code"],
            "app.not-ready"
        );
    }
    events.shutdown().await;
    drop(window);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn locale_commands_reject_absent_services() {
    for has_locale in [false, true] {
        let builder = mock_builder();
        let builder = if has_locale {
            builder.manage(LocaleService::new(
                Locale::En,
                Box::new(TestNative(Arc::new(AtomicBool::new(true)))),
            ))
        } else {
            builder
        };
        let app = builder
            .invoke_handler(tauri::generate_handler![
                locale_get_preference,
                locale_set_preference
            ])
            .build(mock_context(noop_assets()))
            .unwrap();
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        for (name, body) in [
            ("locale_get_preference", serde_json::json!({})),
            (
                "locale_set_preference",
                serde_json::json!({"preference":{"version":1,"locale":"en"}}),
            ),
        ] {
            assert!(
                get_ipc_response(
                    &window,
                    InvokeRequest {
                        cmd: name.into(),
                        callback: tauri::ipc::CallbackFn(0),
                        error: tauri::ipc::CallbackFn(1),
                        url: window.url().unwrap(),
                        body: body.into(),
                        headers: Default::default(),
                        invoke_key: INVOKE_KEY.into(),
                    }
                )
                .is_err()
            );
        }
    }
}

#[test]
fn every_native_surface_is_required_for_applied_status() {
    assert_eq!(native_status([true; 7]), NativeStatus::Applied);
    for index in 0..7 {
        let mut updated = [true; 7];
        updated[index] = false;
        assert_eq!(native_status(updated), NativeStatus::Pending);
    }
}
