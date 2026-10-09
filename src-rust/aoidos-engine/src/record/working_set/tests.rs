use super::*;
use crate::record::{
    format::RecapOrigin,
    projection::{self, Budget, Shape, WorldView},
    session::Target,
    test_support::{Fixture, candidate},
};

#[test]
fn working_set_has_hard_bounds_and_keeps_current_player() {
    let mut fixture = Fixture::new();
    fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "p".into(),
            text: "当前".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    for _ in 0..260 {
        fixture
            .session
            .append(Body::Narration {
                text: "旧正文".repeat(10),
                turn_id: uuid::Uuid::new_v4().to_string(),
                terminal: crate::ports::Terminal::Completed {
                    finish_reason: aoidos_llm::schedule::FinishReason::Stop,
                },
            })
            .unwrap();
    }
    let working = fixture.session.projection_working_set().unwrap();
    assert_eq!(working.records().len(), MAX_WORKSET_RECORDS);
    assert!(working.records().iter().any(|r| r.seq == 1));
    assert!(working.records().iter().map(|r| r.raw.len()).sum::<usize>() <= MAX_WORKSET_BYTES);
    assert!(!working.view().omitted_ranges.is_empty());
}

#[test]
fn verified_recap_can_project_without_loading_its_old_sources() {
    let mut fixture = Fixture::new();
    let frozen = candidate(&mut fixture.session);
    fixture
        .session
        .commit_recap(frozen, "旧情节摘要".into(), RecapOrigin::Manual)
        .unwrap();
    let mut working = fixture.session.projection_working_set().unwrap();
    working.records.retain(|r| r.seq != 1);
    working.omitted = vec![SeqRange {
        from: 1,
        through: 1,
    }];
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
        Shape::Chat,
    )
    .unwrap();
    assert!(plan.included_sequences().contains(&3));
    assert!(plan.folded_sequences().is_empty());
    assert!(plan.omitted_ranges.is_empty());
    assert!(
        projection::project(
            &working.view(),
            &WorldView {
                context: "",
                needs_recovery: true
            },
            &Budget::default(),
            &Target::Narration,
            Shape::Chat
        )
        .is_err()
    );
}

#[test]
fn newest_recap_survives_long_sparse_history_and_omission_metadata_is_bounded() {
    let mut fixture = Fixture::new();
    let frozen = candidate(&mut fixture.session);
    let recap = fixture
        .session
        .commit_recap(frozen, "小型已确认摘要".into(), RecapOrigin::Manual)
        .unwrap();
    for _ in 0..300 {
        fixture.session.next_seq += 1;
        fixture
            .session
            .append(Body::Narration {
                text: "后续".repeat(500),
                turn_id: uuid::Uuid::new_v4().to_string(),
                terminal: crate::ports::Terminal::Completed {
                    finish_reason: aoidos_llm::schedule::FinishReason::Stop,
                },
            })
            .unwrap();
    }
    let working = fixture.session.projection_working_set().unwrap();
    assert!(working.records().iter().any(|record| record.seq == recap));
    assert!(working.omitted.len() <= MAX_WORKSET_RECORDS + 1);
    let plan = projection::project(
        &working.view(),
        &WorldView {
            context: "",
            needs_recovery: false,
        },
        &Budget {
            context_limit: Some(65536),
            tail: 2048,
            ..Budget::default()
        },
        &Target::Narration,
        Shape::Chat,
    )
    .unwrap();
    assert!(plan.included_sequences().contains(&recap));
    fixture.session.read_only = true;
    assert!(
        fixture
            .session
            .projection_working_set()
            .unwrap()
            .needs_recovery
    );
}

#[test]
fn recap_quota_bounds_old_summary_memory_without_discarding_physical_rows() {
    let mut fixture = Fixture::new();
    for _ in 0..17 {
        let frozen = candidate(&mut fixture.session);
        fixture
            .session
            .commit_recap(frozen, "摘要".into(), RecapOrigin::Manual)
            .unwrap();
    }
    let working = fixture.session.projection_working_set().unwrap();
    assert_eq!(
        working
            .records()
            .iter()
            .filter(|r| r.kind == "recap")
            .count(),
        16
    );
    assert_eq!(
        fixture
            .session
            .index
            .values()
            .filter(|r| r.kind == "recap")
            .count(),
        17
    );
    assert!(!working.omitted.is_empty());
}
