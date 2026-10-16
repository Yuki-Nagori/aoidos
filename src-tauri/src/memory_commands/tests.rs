use super::*;
use crate::{
    store_commands::{StorageEvents, StorageService},
    turn_commands::WindowEvents,
};
use aoidos_engine::storage::QuestionDraft;
use std::collections::BTreeMap;
use std::sync::Arc;
use tauri::{
    test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets},
    webview::InvokeRequest,
};

#[test]
fn question_ipc_uses_camel_case_without_changing_persisted_enum_encoding() {
    let answer: QuestionAnswerDto = serde_json::from_value(serde_json::json!({
        "kind":"selectCandidate",
        "candidate":{"kind":"entity","entityId":"npc-1"}
    }))
    .unwrap();
    assert_eq!(
        QuestionAnswer::from(answer),
        QuestionAnswer::SelectCandidate {
            candidate: QuestionCandidate::Entity {
                entity_id: "npc-1".into()
            }
        }
    );

    let answer = QuestionAnswerDto::ProposeEntity {
        display_name: "李雷".into(),
    };
    assert_eq!(
        serde_json::to_value(answer).unwrap(),
        serde_json::json!({"kind":"proposeEntity","displayName":"李雷"})
    );
    assert_eq!(
        serde_json::to_value(QuestionAnswer::ProposeEntity {
            display_name: "李雷".into()
        })
        .unwrap(),
        serde_json::json!({"kind":"proposeEntity","display_name":"李雷"})
    );
    assert_eq!(
        serde_json::to_value(QuestionCandidate::Entity {
            entity_id: "npc-1".into()
        })
        .unwrap(),
        serde_json::json!({"kind":"entity","entity_id":"npc-1"})
    );

    let question = MemoryQuestion {
        question_id: "00000000-0000-4000-8000-000000000001".into(),
        run_id: "00000000-0000-4000-8000-000000000002".into(),
        question_key: [7; 32],
        revision: 1,
        draft: QuestionDraft {
            version: 1,
            identity: QuestionIdentity {
                kind: aoidos_engine::storage::QuestionKind::EntityAssociation,
                event_slot: 3,
                sources: vec![],
            },
            history_revision: 0,
            entity_revision: 1,
            evidence_revision: 1,
            mentioned_name: None,
            candidates: vec![QuestionCandidate::Entity {
                entity_id: "npc-1".into(),
            }],
        },
        status: QuestionStatus::Resolved,
        answer: Some(QuestionAnswer::ProposeEntity {
            display_name: "李雷".into(),
        }),
    };
    let names = BTreeMap::from([("npc-1".into(), "李雷".into())]);
    let wire = serde_json::to_value(MemoryQuestionDto::from_model(question, &names)).unwrap();
    assert_eq!(wire["draft"]["candidates"][0]["entityId"], "npc-1");
    assert_eq!(wire["draft"]["candidates"][0]["displayName"], "李雷");
    assert_eq!(wire["answer"]["displayName"], "李雷");
    assert!(wire["draft"]["candidates"][0].get("entity_id").is_none());
    assert!(wire["answer"].get("display_name").is_none());
}

#[tokio::test]
async fn memory_configuration_commands_preserve_versions_and_return_effective_values() {
    let root =
        std::env::temp_dir().join(format!("aoidos-memory-config-ipc-{}", std::process::id()));
    let events = StorageEvents::new(Arc::new(WindowEvents::new(|_, _| Ok(()))));
    let app = mock_builder()
        .manage(StorageService::new(&root, events))
        .invoke_handler(tauri::generate_handler![
            memory_get_app_config,
            memory_save_app_config,
            memory_get_script_config,
            memory_save_script_config,
            memory_list_questions,
            memory_resolve_question,
            memory_list_entries,
            memory_get_entry,
            memory_get_batch_status
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

    let app_config = call("memory_get_app_config", serde_json::json!({})).unwrap();
    assert_eq!(app_config["configVersion"], 1);
    let mut parameters = app_config["parameters"].clone();
    parameters["algorithm"] = "none".into();
    assert_eq!(
        call(
            "memory_save_app_config",
            serde_json::json!({"expectedVersion":1,"parameters":parameters}),
        )
        .unwrap()["configVersion"],
        2
    );

    let initial = call(
        "memory_get_script_config",
        serde_json::json!({"scriptId":"mistbell"}),
    )
    .unwrap();
    assert_eq!(initial["configVersion"], 0);
    assert_eq!(initial["effective"]["algorithm"], "none");
    assert_eq!(
        call(
            "memory_get_script_config",
            serde_json::json!({"scriptId":"unregistered-script"}),
        )
        .unwrap_err()["code"],
        "app.not-found"
    );
    let saved = call(
        "memory_save_script_config",
        serde_json::json!({
            "scriptId":"mistbell",
            "expectedVersion":0,
            "expectedAppVersion":2,
            "overrides":{"reinforcementGain":0}
        }),
    )
    .unwrap();
    assert_eq!(saved["configVersion"], 1);
    let overridden = call(
        "memory_get_script_config",
        serde_json::json!({"scriptId":"mistbell"}),
    )
    .unwrap();
    assert_eq!(overridden["effective"]["reinforcementGain"], 0);
    assert_eq!(overridden["effective"]["algorithm"], "none");
    assert_eq!(
        call(
            "memory_save_script_config",
            serde_json::json!({
                "scriptId":"mistbell",
                "expectedVersion":1,
                "expectedAppVersion":2,
                "overrides":{}
            }),
        )
        .unwrap()["configVersion"],
        2
    );
    assert_eq!(
        call(
            "memory_save_script_config",
            serde_json::json!({
                "scriptId":"mistbell",
                "expectedVersion":1,
                "expectedAppVersion":2,
                "overrides":{}
            }),
        )
        .unwrap_err()["code"],
        "app.bad-request"
    );
    assert_eq!(
        call("memory_save_app_config", serde_json::json!({"extra":true})).unwrap_err()["code"],
        "app.bad-request"
    );
    let unknown_run = "00000000-0000-4000-8000-000000000001";
    for (command, body) in [
        (
            "memory_list_questions",
            serde_json::json!({"runId":unknown_run,"cursor":null,"limit":1}),
        ),
        (
            "memory_resolve_question",
            serde_json::json!({
                "runId":unknown_run,
                "resolution":{
                    "questionId":"00000000-0000-4000-8000-000000000002",
                    "expectedRevision":1,
                    "expectedHistoryRevision":0,
                    "answer":{"kind":"none"}
                }
            }),
        ),
        (
            "memory_resolve_question",
            serde_json::json!({
                "runId":unknown_run,
                "resolution":{
                    "questionId":"00000000-0000-4000-8000-000000000002",
                    "expectedRevision":1,
                    "expectedHistoryRevision":0,
                    "answer":{
                        "kind":"selectCandidate",
                        "candidate":{"kind":"entity","entityId":"npc-1"}
                    }
                }
            }),
        ),
        (
            "memory_resolve_question",
            serde_json::json!({
                "runId":unknown_run,
                "resolution":{
                    "questionId":"00000000-0000-4000-8000-000000000002",
                    "expectedRevision":1,
                    "expectedHistoryRevision":0,
                    "answer":{"kind":"proposeEntity","displayName":"李雷"}
                }
            }),
        ),
        (
            "memory_list_entries",
            serde_json::json!({"runId":unknown_run,"cursor":null,"limit":1}),
        ),
        (
            "memory_get_entry",
            serde_json::json!({
                "runId":unknown_run,
                "entryId":"00000000-0000-4000-8000-000000000003",
                "expectedVersionId":"00000000-0000-4000-8000-000000000004"
            }),
        ),
        (
            "memory_get_batch_status",
            serde_json::json!({"runId":unknown_run,"batchId":unknown_run}),
        ),
    ] {
        assert_eq!(call(command, body).unwrap_err()["code"], "app.not-found");
    }
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}
