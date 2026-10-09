use super::*;
use crate::turn_commands::WindowEvents;
use aoidos_engine::{
    fault::Fault,
    game::{
        domain::{FrozenRound, RoundFactory},
        state::PhaseEvents,
    },
    turn::Coordinator,
};
use std::sync::{Arc, Mutex};
use tauri::{
    Manager,
    test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets},
    webview::InvokeRequest,
};

struct Missing;
impl RoundFactory for Missing {
    fn freeze(&self) -> Result<FrozenRound, Fault> {
        Err(Fault::new("llm.missing-key", "测试配置无密钥"))
    }
}

#[tokio::test]
async fn complete_product_request_bodies_reject_overrides_wrong_types_and_unknown_sessions() {
    let events = Arc::new(WindowEvents::new(|_, _| Ok(())));
    let coordinator = Coordinator::new(events, 16).unwrap();
    let factory = Arc::new(Missing);
    assert_eq!(factory.freeze().err().unwrap().code, "llm.missing-key");
    let app = mock_builder()
        .manage(Service::new(coordinator, factory))
        .invoke_handler(tauri::generate_handler![
            crate::game_ipc::engine_submit_input,
            crate::game_ipc::engine_interrupt,
            crate::game_ipc::engine_cancel_round,
            crate::game_ipc::engine_resume,
            crate::game_ipc::engine_regenerate,
            crate::game_ipc::engine_rewind,
            crate::game_ipc::engine_submit_check,
            crate::game_ipc::engine_get_phase,
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
    };
    let id = "00000000-0000-4000-8000-000000000023";
    for (command, args) in [
        (
            "engine_submit_input",
            serde_json::json!({"sessionId":id,"text":"输入"}),
        ),
        (
            "engine_interrupt",
            serde_json::json!({"sessionId":id,"roundId":id,"text":"插话"}),
        ),
        (
            "engine_cancel_round",
            serde_json::json!({"sessionId":id,"roundId":id}),
        ),
        ("engine_resume", serde_json::json!({"sessionId":id})),
        (
            "engine_regenerate",
            serde_json::json!({"sessionId":id,"roundId":id}),
        ),
        (
            "engine_rewind",
            serde_json::json!({"sessionId":id,"targetSeq":1}),
        ),
        (
            "engine_submit_check",
            serde_json::json!({"sessionId":id,"roundId":id,"planId":id}),
        ),
        ("engine_get_phase", serde_json::json!({"sessionId":id})),
    ] {
        assert_eq!(
            call(command, tauri::ipc::InvokeBody::Json(args.clone())).unwrap_err()["code"],
            "app.not-found"
        );
        let mut overridden = args;
        overridden["seed"] = serde_json::json!("client-seed");
        assert_eq!(
            call(command, tauri::ipc::InvokeBody::Json(overridden)).unwrap_err()["code"],
            "app.bad-request"
        );
        assert_eq!(
            call(command, tauri::ipc::InvokeBody::Raw(vec![1, 2, 3])).unwrap_err()["code"],
            "app.bad-request"
        );
    }
    for target in [
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!("1"),
        serde_json::json!(null),
    ] {
        assert_eq!(
            call(
                "engine_rewind",
                tauri::ipc::InvokeBody::Json(
                    serde_json::json!({"sessionId":id,"targetSeq":target})
                )
            )
            .unwrap_err()["code"],
            "app.bad-request"
        );
    }
    app.state::<Service>().shutdown().await;
}

#[test]
fn phase_window_adapter_keeps_independent_sequences_and_closed_window_errors() {
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let output = delivered.clone();
    let events = WindowEvents::new(move |name, payload| {
        output.lock().unwrap().push((name.to_owned(), payload));
        Ok(())
    });
    let id = uuid::Uuid::new_v4().to_string();
    let state = PhaseState {
        session_id: id.clone(),
        state_epoch: uuid::Uuid::new_v4().to_string(),
        phase_revision: 1,
        history_revision: 0,
        phase: Phase::Idle,
        scene: None,
        in_flight: None,
        check: None,
        needs_recovery: false,
        resume_required: false,
        checkpoint: None,
        last_operation: None,
    };
    for event in [
        PhaseEvent::Changed(state.clone()),
        PhaseEvent::Scene {
            state: state.clone(),
            previous_scene_id: None,
        },
        PhaseEvent::Done {
            state: state.clone(),
            operation_id: id.clone(),
            outcome: aoidos_engine::ports::Outcome::Completed,
        },
        PhaseEvent::Failed {
            state: state.clone(),
            operation_id: id.clone(),
            code: "llm.bad-response".into(),
            message: "测试错误".into(),
        },
    ] {
        assert_eq!(PhaseEvents::prepare(&events, &event).unwrap(), 1);
        PhaseEvents::deliver(&events, 1, event).unwrap();
    }
    assert_eq!(delivered.lock().unwrap().len(), 4);
    PhaseEvents::retire(&events, &id);
    assert_eq!(
        PhaseEvents::prepare(&events, &PhaseEvent::Changed(state.clone())).unwrap(),
        1
    );
    events.close();
    assert_eq!(
        PhaseEvents::deliver(&events, 1, PhaseEvent::Changed(state))
            .unwrap_err()
            .code,
        "app.event-failed"
    );
    assert_eq!(
        super::events::prepare_error(CmdError::new("app.event-failed", "测试错误", None)).code,
        "app.event-failed"
    );
}
