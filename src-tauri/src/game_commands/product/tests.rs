use super::*;
use aoidos_engine::{game::product::ProfileFactory, storage::Storage, turn::Coordinator};
use std::sync::{Arc, Mutex};
use tauri::{
    Manager,
    test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets},
    webview::InvokeRequest,
};
struct StorageEvents;
impl aoidos_engine::migration::MigrationEvents for StorageEvents {
    fn prepare(&self, _: &aoidos_engine::migration::MigrationEvent) -> Result<u64, Fault> {
        Ok(1)
    }
    fn deliver(&self, _: u64, _: aoidos_engine::migration::MigrationEvent) -> Result<(), Fault> {
        Ok(())
    }
    fn retire(&self, _: &str) {}
}
impl aoidos_engine::record::session::RecordEvents for StorageEvents {
    fn prepare(&self, _: &aoidos_engine::record::session::Appended) -> Result<u64, Fault> {
        Ok(1)
    }
    fn deliver(&self, _: u64, _: aoidos_engine::record::session::Appended) -> Result<(), Fault> {
        Ok(())
    }
}
struct Profiles;
impl ProfileSource for Profiles {
    fn freeze(&self, id: &str) -> Result<ResolvedProfile, Fault> {
        if id == "missing" {
            return Err(Fault::new("llm.missing-key", "请登记密钥"));
        }
        Ok(ResolvedProfile {
            frozen: aoidos_llm::config::FrozenProfile {
                profile: aoidos_llm::config::LlmProfile {
                    profile_id: id.into(),
                    provider_id: "deepseek".into(),
                    model: "deepseek-v4-pro".into(),
                    mode: aoidos_llm::config::ProfileMode::Completion,
                    thinking: false,
                    sampling: aoidos_llm::sampling::Sampling {
                        temperature: 1.0,
                        max_tokens: 128,
                    },
                    proxy: aoidos_llm::config::ProxyConfig::None,
                },
                credential: Some(secrecy::SecretString::from(
                    "local-only-unused-key".to_owned(),
                )),
            },
            proxy_auth: None,
        })
    }
}
#[test]
fn saved_profile_rejects_invalid_identity_before_touching_persistent_state() {
    assert_eq!(
        SavedProfiles.freeze("").err().unwrap().code,
        "app.bad-request"
    );
}
#[tokio::test]
async fn session_commands_accept_only_complete_selection_and_preserve_saved_identity() {
    let root =
        std::env::temp_dir().join(format!("aoidos-shell-selection-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let window_events = Arc::new(crate::turn_commands::WindowEvents::new(|_, _| Ok(())));
    let coordinator = Coordinator::new(window_events.clone(), 16).unwrap();
    let storage = Arc::new(Storage::new(
        &root,
        Arc::new(StorageEvents),
        Arc::new(StorageEvents),
    ));
    let factory = ProfileFactory::new(
        Arc::new(Profiles),
        storage.clone(),
        aoidos_llm::proxy::SystemProxySnapshot::default(),
        Arc::new(Mutex::new(Default::default())),
    );
    let product = Product::new(
        &root,
        storage.clone(),
        coordinator.clone(),
        window_events,
        factory.clone(),
    );
    let app = mock_builder()
        .manage(Service::new(coordinator, factory))
        .manage(product)
        .invoke_handler(tauri::generate_handler![
            crate::game_ipc::engine_list_scripts,
            crate::game_ipc::engine_open_session
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let call = |command: &str, body: tauri::ipc::InvokeBody| {
        get_ipc_response(
            &window,
            InvokeRequest {
                cmd: command.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: window.url().unwrap(),
                body,
                headers: Default::default(),
                invoke_key: INVOKE_KEY.into(),
            },
        )
    };
    let scripts = call(
        "engine_list_scripts",
        tauri::ipc::InvokeBody::Json(serde_json::json!({})),
    )
    .unwrap()
    .deserialize::<serde_json::Value>()
    .unwrap();
    assert_eq!(scripts[0]["scriptId"], "mistbell");
    assert_eq!(scripts[0]["displayNames"]["en"], "Mistbell Cellar");
    assert_eq!(scripts[0]["displayNames"]["zh-Hans"], "雾钟地窖");
    assert!(!scripts[0]["attributions"].as_str().unwrap().is_empty());
    for body in [
        serde_json::json!({}),
        serde_json::json!({"profileId":"p","scriptId":"mistbell"}),
        serde_json::json!({"profileId":"p","scriptId":"mistbell","startNew":1}),
        serde_json::json!({"profileId":"p","scriptId":"mistbell","startNew":false,"endpoint":"http://override"}),
    ] {
        assert_eq!(
            call("engine_open_session", tauri::ipc::InvokeBody::Json(body)).unwrap_err()["code"],
            "app.bad-request"
        );
    }
    assert_eq!(
        call("engine_open_session", tauri::ipc::InvokeBody::Raw(vec![1])).unwrap_err()["code"],
        "app.bad-request"
    );
    assert_eq!(
        call(
            "engine_open_session",
            tauri::ipc::InvokeBody::Json(
                serde_json::json!({"profileId":"missing","scriptId":"mistbell","startNew":false})
            )
        )
        .unwrap_err()["code"],
        "llm.missing-key"
    );
    assert!(!root.join("active-session.json").exists());
    let args = serde_json::json!({"profileId":"p","scriptId":"mistbell","startNew":false});
    let opened = call(
        "engine_open_session",
        tauri::ipc::InvokeBody::Json(args.clone()),
    )
    .unwrap()
    .deserialize::<serde_json::Value>()
    .unwrap();
    assert_eq!(opened["profileId"], "p");
    assert_eq!(opened["scriptId"], "mistbell");
    assert_eq!(opened["title"], "雾钟地窖");
    let reopened = call("engine_open_session", tauri::ipc::InvokeBody::Json(args))
        .unwrap()
        .deserialize::<serde_json::Value>()
        .unwrap();
    assert_eq!(reopened["sessionId"], opened["sessionId"]);
    app.state::<Service>().shutdown().await;
    storage.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
