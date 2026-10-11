use super::*;

fn archive_candidate(id: &str, required: bool, strength: u64, position: u64) -> ArchiveCandidate {
    ArchiveCandidate {
        entry_id: id.into(),
        required,
        strength,
        last_logical_position: position,
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
