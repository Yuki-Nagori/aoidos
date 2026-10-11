use super::*;

fn archive_candidate(id: &str, required: bool, strength: u64, position: u64) -> ArchiveCandidate {
    ArchiveCandidate {
        entry_id: id.into(),
        required,
        strength,
        last_logical_position: position,
    }
}

fn candidate_work(id: &str, position: u64, materials: &[&str], targets: &[&str]) -> CandidateWork {
    CandidateWork {
        candidate_id: id.into(),
        earliest_source_position: position,
        material_ids: materials.iter().map(|id| (*id).into()).collect(),
        target_entry_ids: targets.iter().map(|id| (*id).into()).collect(),
    }
}

fn policy(algorithm: Algorithm) -> Policy {
    Policy {
        version: 1,
        algorithm_version: 1,
        config_version: 1,
        app_config_version: 1,
        algorithm,
        base_strength: 200_000,
        reinforcement_gain: 50_000,
        reinforcement_cap: 500_000,
        strength_cap: 1_000_000,
        half_life_rounds: 100,
        repeat_window_rounds: 20,
        repeat_limit: 2,
        revival_cooldown_rounds: 20,
        active_capacity: 1_000,
        rate_table_version: 1,
        decay_rate: 900_000,
    }
}

#[test]
fn archive_selection_uses_required_score_recency_and_byte_order() {
    let candidates = [
        archive_candidate("00000000-0000-4000-8000-000000000004", false, 900_000, 1),
        archive_candidate("00000000-0000-4000-8000-000000000005", true, 100_000, 0),
        archive_candidate("00000000-0000-4000-8000-000000000003", false, 900_000, 2),
        archive_candidate("00000000-0000-4000-8000-000000000002", false, 900_000, 2),
    ];
    assert_eq!(
        select_archive(&candidates, 2).unwrap(),
        [
            "00000000-0000-4000-8000-000000000003",
            "00000000-0000-4000-8000-000000000004"
        ]
    );
}

#[test]
fn archive_selection_rejects_capacity_overflow_and_invalid_candidates() {
    let candidates = [
        archive_candidate("00000000-0000-4000-8000-000000000001", true, 10, 0),
        archive_candidate("00000000-0000-4000-8000-000000000002", true, 20, 0),
    ];
    assert!(matches!(
        select_archive(&candidates, 1),
        Err(Error::Rejected(Reason::CapacityBlocked))
    ));
    assert!(matches!(
        select_archive(&candidates, 0),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));

    let duplicate = [
        archive_candidate("00000000-0000-4000-8000-000000000003", false, 10, 0),
        archive_candidate("00000000-0000-4000-8000-000000000003", false, 20, 0),
    ];
    assert!(matches!(
        select_archive(&duplicate, 1),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));

    let invalid_score = [archive_candidate(
        "00000000-0000-4000-8000-000000000004",
        false,
        1_000_001,
        0,
    )];
    assert!(matches!(
        select_archive(&invalid_score, 1),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));

    let invalid_position = [archive_candidate(
        "00000000-0000-4000-8000-000000000005",
        false,
        1,
        MAX_INTEGER + 1,
    )];
    assert!(matches!(
        select_archive(&invalid_position, 1),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
    let invalid_id = [archive_candidate("invalid", false, 1, 0)];
    assert!(matches!(
        select_archive(&invalid_id, 1),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
    assert!(
        select_archive(&[], MAX_ARCHIVE_CANDIDATES)
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        select_archive(&[], MAX_ARCHIVE_CANDIDATES + 1),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));

    let oversized = vec![
        archive_candidate("00000000-0000-4000-8000-000000000006", false, 1, 0);
        MAX_ARCHIVE_CANDIDATES + 1
    ];
    assert!(matches!(
        select_archive(&oversized, 1),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
}

#[test]
fn dependency_groups_join_shared_sources_and_targets_in_stable_order() {
    let material_a = "00000000-0000-4000-8000-000000000011";
    let material_b = "00000000-0000-4000-8000-000000000012";
    let material_c = "00000000-0000-4000-8000-000000000013";
    let target_a = "00000000-0000-4000-8000-000000000021";
    let target_b = "00000000-0000-4000-8000-000000000022";
    let target_c = "00000000-0000-4000-8000-000000000023";
    let candidates = [
        candidate_work("bridge", 3, &[material_b], &[target_b]),
        candidate_work("second", 2, &[material_b], &[target_a]),
        candidate_work("independent", 0, &[material_c], &[target_c]),
        candidate_work("first", 1, &[material_a], &[target_a]),
    ];

    assert_eq!(
        dependency_groups(&candidates).unwrap(),
        [
            vec!["independent".to_owned()],
            vec!["first".to_owned(), "second".to_owned(), "bridge".to_owned()]
        ]
    );
}

#[test]
fn dependency_groups_break_equal_source_positions_by_candidate_id_bytes() {
    let material_a = "00000000-0000-4000-8000-000000000051";
    let material_z = "00000000-0000-4000-8000-000000000052";
    let candidates = [
        candidate_work("zeta", 7, &[material_z], &[]),
        candidate_work("alpha", 7, &[material_a], &[]),
    ];

    assert_eq!(
        dependency_groups(&candidates).unwrap(),
        [vec!["alpha".to_owned()], vec!["zeta".to_owned()]]
    );
}

#[test]
fn dependency_groups_sort_shared_members_with_equal_positions_by_candidate_id_bytes() {
    let shared_material = "00000000-0000-4000-8000-000000000053";
    let candidates = [
        candidate_work("zeta", 7, &[shared_material], &[]),
        candidate_work("beta", 7, &[shared_material], &[]),
        candidate_work("alpha", 7, &[shared_material], &[]),
    ];

    assert_eq!(
        dependency_groups(&candidates).unwrap(),
        [vec![
            "alpha".to_owned(),
            "beta".to_owned(),
            "zeta".to_owned()
        ]]
    );
}

#[test]
fn dependency_groups_reject_malformed_or_oversized_plans() {
    let material = "00000000-0000-4000-8000-000000000031";
    let bad_id = [candidate_work("bad id", 0, &[material], &[])];
    let bad_position = [candidate_work("c1", MAX_INTEGER + 1, &[material], &[])];
    let empty_sources = [candidate_work("c1", 0, &[], &[])];
    let malformed_material = [candidate_work("c1", 0, &["not-a-uuid"], &[])];
    let duplicate_material = [candidate_work("c1", 0, &[material, material], &[])];
    let duplicate_targets = [candidate_work(
        "c1",
        0,
        &[material],
        &[
            "00000000-0000-4000-8000-000000000041",
            "00000000-0000-4000-8000-000000000041",
        ],
    )];
    for invalid in [
        bad_id.as_slice(),
        bad_position.as_slice(),
        empty_sources.as_slice(),
        malformed_material.as_slice(),
        duplicate_material.as_slice(),
        duplicate_targets.as_slice(),
    ] {
        assert!(matches!(
            dependency_groups(invalid),
            Err(Error::Rejected(Reason::InvalidSchema))
        ));
    }

    let shared_material = "00000000-0000-4000-8000-000000000032";
    let targets = (0..=MAX_OPERATION_TARGETS)
        .map(|index| {
            let candidate_id = format!("c{index}");
            let target_id = format!("00000000-0000-4000-8000-{index:012}");
            candidate_work(
                &candidate_id,
                index as u64,
                &[shared_material],
                &[&target_id],
            )
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        dependency_groups(&targets),
        Err(Error::Rejected(Reason::CapacityBlocked))
    ));

    let oversized = (0..=MAX_CANDIDATES)
        .map(|index| candidate_work(&format!("c{index}"), index as u64, &[material], &[]))
        .collect::<Vec<_>>();
    assert!(matches!(
        dependency_groups(&oversized),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
}

#[test]
fn exponential_score_uses_exact_fixed_point_steps_and_effective_gains() {
    let policy = policy(Algorithm::Exponential);
    assert_eq!(strength_score(&policy, 10, 10, 100_000).unwrap(), 300_000);
    assert_eq!(strength_score(&policy, 11, 10, 100_000).unwrap(), 270_000);
    assert_eq!(strength_score(&policy, 13, 10, 100_000).unwrap(), 218_700);
    assert_eq!(fixed_power(900_000, 2), 810_000);
}

#[test]
fn reinforcement_and_strength_caps_apply_before_algorithm() {
    let mut policy = policy(Algorithm::None);
    policy.base_strength = 700_000;
    policy.reinforcement_cap = 200_000;
    policy.strength_cap = 900_000;
    assert_eq!(strength_score(&policy, 0, 0, 800_000).unwrap(), 900_000);
}

#[test]
fn segmented_score_reaches_half_strength_and_then_zero() {
    let policy = policy(Algorithm::Segmented);
    assert_eq!(strength_score(&policy, 100, 0, 0).unwrap(), 100_000);
    assert_eq!(strength_score(&policy, 200, 0, 0).unwrap(), 0);
    assert_eq!(strength_score(&policy, 250, 0, 0).unwrap(), 0);
}

#[test]
fn no_decay_is_a_bounded_baseline() {
    let policy = policy(Algorithm::None);
    assert_eq!(
        strength_score(&policy, 10_000, 0, 1_000_000).unwrap(),
        700_000
    );
}

#[test]
fn score_rejects_time_regression_unsafe_inputs_and_unknown_policy_versions() {
    let policy = policy(Algorithm::Exponential);
    assert!(matches!(
        strength_score(&policy, 9, 10, 0),
        Err(Error::Rejected(Reason::StaleVersion))
    ));
    assert!(matches!(
        strength_score(&policy, MAX_INTEGER + 1, 0, 0),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
    let mut unsupported = policy;
    unsupported.algorithm_version = 2;
    assert!(matches!(
        strength_score(&unsupported, 0, 0, 0),
        Err(Error::Rejected(Reason::PolicyUnavailable))
    ));
}
