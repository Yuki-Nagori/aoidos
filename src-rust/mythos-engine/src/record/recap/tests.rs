use super::*;
use crate::record::{
    projection::{self, Budget, Shape, WorldView},
    session::Target,
    test_support::{Fixture, candidate},
};

#[test]
fn candidates_are_single_snapshot_and_single_session() {
    let mut fixture = Fixture::new();
    let frozen = candidate(&mut fixture.session);
    assert!(frozen.source().contains("前文"));
    fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "p".into(),
            text: "新输入".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    assert_eq!(
        fixture
            .session
            .commit_recap(frozen, "过期".into(), RecapOrigin::Manual)
            .unwrap_err()
            .code,
        "engine.invalid-phase"
    );
    let frozen = candidate(&mut fixture.session);
    let mut other = Fixture::new();
    assert!(
        other
            .session
            .commit_recap(frozen, "跨会话".into(), RecapOrigin::Manual)
            .is_err()
    );
    let frozen = candidate(&mut fixture.session);
    fixture.session.view_epoch = uuid::Uuid::new_v4().to_string();
    assert!(
        fixture
            .session
            .commit_recap(frozen, "旧 epoch".into(), RecapOrigin::Manual)
            .is_err()
    );
    let frozen = candidate(&mut fixture.session);
    let seq = fixture
        .session
        .commit_recap(frozen, "已确认旧情节".into(), RecapOrigin::Manual)
        .unwrap();
    assert!(matches!(
        fixture.session.read(seq).unwrap().body.unwrap().body,
        Body::Recap {
            estimator_version: 2,
            ..
        }
    ));
}

#[test]
fn invalid_ranges_current_tail_and_stale_plans_are_rejected_before_requests() {
    let mut fixture = Fixture::new();
    let _ = candidate(&mut fixture.session);
    let working = fixture.session.projection_working_set().unwrap();
    let plan = projection::project(
        &working.view(),
        &WorldView {
            context: "",
            needs_recovery: false,
        },
        &Budget {
            context_limit: Some(65536),
            tail: 100,
            ..Budget::default()
        },
        &Target::Narration,
        Shape::Completion,
    )
    .unwrap();
    for (from, through) in [(0, 1), (2, 1), (2, 2), (1, 999)] {
        assert!(
            fixture
                .session
                .prepare_recap(&working, &plan, from, through)
                .is_err()
        );
    }
    fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "p".into(),
            text: "新输入".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    assert!(
        fixture
            .session
            .prepare_recap(&working, &plan, 1, 1)
            .is_err()
    );
    let current = fixture.session.projection_working_set().unwrap();
    assert!(
        fixture
            .session
            .prepare_recap(&current, &plan, 1, 1)
            .is_err()
    );
    let mut plan = projection::project(
        &current.view(),
        &WorldView {
            context: "",
            needs_recovery: false,
        },
        &Budget {
            context_limit: Some(65536),
            tail: 100,
            ..Budget::default()
        },
        &Target::Narration,
        Shape::Completion,
    )
    .unwrap();
    plan.estimator_version = 99;
    assert!(
        fixture
            .session
            .prepare_recap(&current, &plan, 1, 1)
            .is_err()
    );
}

#[test]
fn candidate_sources_are_bounded_and_active_generation_prevents_freezing() {
    let mut fixture = Fixture::new();
    let frozen = candidate(&mut fixture.session);
    fixture
        .session
        .commit_recap(frozen, "旧摘要".into(), RecapOrigin::Manual)
        .unwrap();
    for _ in 0..65 {
        fixture
            .session
            .append(Body::Narration {
                text: "旧".repeat(100),
                turn_id: uuid::Uuid::new_v4().to_string(),
                terminal: crate::ports::Terminal::Completed {
                    finish_reason: mythos_llm::schedule::FinishReason::Stop,
                },
            })
            .unwrap();
    }
    fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "p".into(),
            text: "当前".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    let working = fixture.session.projection_working_set().unwrap();
    let mut plan = projection::project(
        &working.view(),
        &WorldView {
            context: "",
            needs_recovery: false,
        },
        &Budget {
            context_limit: Some(65536),
            tail: 100,
            ..Budget::default()
        },
        &Target::Narration,
        Shape::Completion,
    )
    .unwrap();
    plan.included.clear();
    assert!(
        fixture
            .session
            .prepare_recap(&working, &plan, 1, 3)
            .is_err()
    );
    assert!(
        fixture
            .session
            .prepare_recap(&working, &plan, 4, 68)
            .is_err()
    );
    let turn = uuid::Uuid::new_v4().to_string();
    fixture
        .session
        .delta(&turn, &Target::Narration, 1, "开始", true)
        .unwrap();
    assert_eq!(
        fixture
            .session
            .prepare_recap(&working, &plan, 4, 4)
            .err()
            .unwrap()
            .code,
        "app.busy"
    );
}
