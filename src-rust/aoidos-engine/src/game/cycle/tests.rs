use super::*;
use crate::game::catalog::EvidenceRef;

fn scene() -> SceneRuleView {
    SceneRuleView {
        scene_id: "archive".into(),
        catalog_revision: "catalog-v1".into(),
        advance_rule_ids: vec![],
        progress_rule_ids: vec!["found-key".into(), "opened-gate".into()],
        hint_rules: vec![
            HintRule {
                hint_id: "gate-hint".into(),
                when_rule_id: "opened-gate".into(),
                text: "门边有一道未曾留意的刻痕。".into(),
                priority: 10,
                target_scope: "scene".into(),
            },
            HintRule {
                hint_id: "key-hint".into(),
                when_rule_id: "found-key".into(),
                text: "钥匙的齿纹似乎与柜门相合。".into(),
                priority: 20,
                target_scope: "scene".into(),
            },
        ],
        exit_rule_ids: vec![],
    }
}

fn matched(rule_id: &str, value_hash: &str, revision: u64, matched: bool) -> SceneRuleMatch {
    SceneRuleMatch {
        rule_id: rule_id.into(),
        matched,
        catalog_revision: "catalog-v1".into(),
        world_revision: "world-v1".into(),
        history_revision: revision,
        evidence_refs: vec![EvidenceRef::Script {
            script_revision: "script-v1".into(),
            rule_version: 1,
            key: rule_id.into(),
            value_hash: value_hash.into(),
        }],
    }
}

#[test]
fn progress_evidence_is_stable_and_excludes_unmatched_rules() {
    let scene = scene();
    let matches = vec![
        matched("opened-gate", "b", 7, false),
        matched("found-key", "a", 7, true),
    ];
    let evidence = progress_evidence(&scene, &matches, 7, "world-v1").unwrap();
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].rule_id, "found-key");
    assert_eq!(
        evidence,
        progress_evidence(&scene, &matches, 7, "world-v1").unwrap()
    );
}

#[test]
fn progress_evidence_rejects_stale_or_unregistered_rule_results() {
    let scene = scene();
    assert!(
        progress_evidence(&scene, &[matched("found-key", "a", 6, true)], 7, "world-v1").is_err()
    );
    assert!(progress_evidence(&scene, &[matched("unknown", "a", 7, true)], 7, "world-v1").is_err());
    assert!(
        progress_evidence(
            &scene,
            &[matched("found-key", "a", 7, true)],
            7,
            "stale-world"
        )
        .is_err()
    );
}

#[test]
fn eligible_hints_require_stagnation_and_sort_fixed_text_by_priority_then_id() {
    let scene = scene();
    let progress = SceneProgress {
        run_id: "run".into(),
        scene_id: "archive".into(),
        catalog_revision: "catalog-v1".into(),
        entry_history_revision: 0,
        entry_record_seq: 0,
        revision: 1,
        progress_count: 0,
        stagnation_rounds: aoidos_memory::model::NODE_HINT_AFTER_STAGNANT_ROUNDS,
        completed_rounds: 8,
        last_completed_record_seq: 8,
        last_round_id: None,
        last_evidence_hash: None,
        hints_shown: 0,
        last_hint_round: None,
        last_hint_id: None,
    };
    let matches = vec![
        matched("opened-gate", "b", 0, true),
        matched("found-key", "a", 0, true),
    ];
    let hints = eligible_hints(&scene, &matches, &progress, 0, "world-v1").unwrap();
    assert_eq!(hints[0].hint_id, "key-hint");
    assert_eq!(hints[1].hint_id, "gate-hint");

    let mut not_stagnant = progress.clone();
    not_stagnant.stagnation_rounds -= 1;
    assert!(
        eligible_hints(&scene, &matches, &not_stagnant, 0, "world-v1")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn eligible_hints_rejects_an_incomplete_rule_result_set() {
    let scene = scene();
    let progress = SceneProgress {
        run_id: "run".into(),
        scene_id: "archive".into(),
        catalog_revision: "catalog-v1".into(),
        entry_history_revision: 0,
        entry_record_seq: 0,
        revision: 1,
        progress_count: 0,
        stagnation_rounds: aoidos_memory::model::NODE_HINT_AFTER_STAGNANT_ROUNDS,
        completed_rounds: 8,
        last_completed_record_seq: 8,
        last_round_id: None,
        last_evidence_hash: None,
        hints_shown: 0,
        last_hint_round: None,
        last_hint_id: None,
    };

    assert!(
        eligible_hints(
            &scene,
            &[matched("found-key", "a", 0, true)],
            &progress,
            0,
            "world-v1",
        )
        .is_err()
    );
}

#[test]
fn eligible_hints_allow_multiple_hints_to_share_one_condition() {
    let mut scene = scene();
    scene.hint_rules[1].when_rule_id = "opened-gate".into();
    let progress = SceneProgress {
        run_id: "run".into(),
        scene_id: "archive".into(),
        catalog_revision: "catalog-v1".into(),
        entry_history_revision: 0,
        entry_record_seq: 0,
        revision: 1,
        progress_count: 0,
        stagnation_rounds: aoidos_memory::model::NODE_HINT_AFTER_STAGNANT_ROUNDS,
        completed_rounds: 8,
        last_completed_record_seq: 8,
        last_round_id: None,
        last_evidence_hash: None,
        hints_shown: 0,
        last_hint_round: None,
        last_hint_id: None,
    };

    let hints = eligible_hints(
        &scene,
        &[matched("opened-gate", "gate", 0, true)],
        &progress,
        0,
        "world-v1",
    )
    .unwrap();

    assert_eq!(hints.len(), 2);
}
