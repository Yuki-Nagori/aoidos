use super::*;

fn source(seq: u64) -> CycleSourceRef {
    CycleSourceRef {
        source: SourceRef {
            script_id: "mistbell".into(),
            run_id: "11111111-1111-4111-8111-111111111111".into(),
            session_id: "22222222-2222-4222-8222-222222222222".into(),
            record_seq: seq,
            body_hash: [u8::try_from(seq).unwrap(); 32],
            start_byte: 0,
            end_byte: 8,
            mode: None,
        },
        history_revision: seq,
    }
}

fn mark() -> CycleMarkV1 {
    let mut mark = CycleMarkV1 {
        version: 1,
        mark_id: String::new(),
        script_id: "mistbell".into(),
        cycle_gen: 1,
        rule_id: "npc-familiarity".into(),
        rule_version: 1,
        subject_ids: vec!["keeper".into(), "player".into()],
        imprint_key: "first-rescue".into(),
        source_refs: vec![source(4)],
        kind: CycleKind::Familiarity,
        detail_policy: DetailPolicy::Vague,
        priority: 500,
        summary: "对某个陌生人有模糊的熟悉感。".into(),
    };
    mark.mark_id = mark.stable_mark_id().unwrap();
    mark
}

#[test]
fn cycle_mark_accepts_bounded_typed_payload_and_stable_identity() {
    let forward = mark();
    forward.validate().unwrap();
    let mut reverse = forward.clone();
    reverse.subject_ids.reverse();
    reverse.summary = "换一种表达，不会产生新的刻痕身份。".into();

    assert_eq!(
        forward.identity_hash().unwrap(),
        reverse.identity_hash().unwrap()
    );
    assert_ne!(forward.identity_hash().unwrap(), {
        reverse.imprint_key = "second-rescue".into();
        reverse.identity_hash().unwrap()
    });
}

#[test]
fn cycle_mark_rejects_untrusted_identity_shape_and_source_scope() {
    let mut invalid = mark();
    invalid.subject_ids.push("keeper".into());
    assert!(invalid.validate().is_err());

    let mut invalid = mark();
    invalid.source_refs[0].source.script_id = "another-script".into();
    assert!(invalid.validate().is_err());

    let mut invalid = mark();
    invalid.summary = "x".repeat(MAX_CYCLE_SUMMARY_BYTES + 1);
    assert!(invalid.validate().is_err());
}

#[test]
fn cycle_mark_rejects_unknown_version_and_invalid_generation() {
    let mut invalid = mark();
    invalid.version = 2;
    assert!(matches!(
        invalid.validate(),
        Err(crate::error::Error::Rejected(
            crate::error::Reason::PolicyUnavailable
        ))
    ));

    let mut invalid = mark();
    invalid.cycle_gen = 0;
    assert!(invalid.validate().is_err());
}

#[test]
fn cycle_mark_source_identity_includes_exact_range_and_hash() {
    let mut invalid = mark();
    invalid.source_refs.push(source(4));
    assert!(invalid.validate().is_err());
}

#[test]
fn cycle_projection_is_bounded_and_uses_stable_priority_order() {
    let mut highest = mark();
    highest.priority = 1_000;
    highest.mark_id = highest.stable_mark_id().unwrap();
    let mut next = mark();
    next.cycle_gen = 2;
    next.priority = 500;
    next.mark_id = next.stable_mark_id().unwrap();
    let mut archived = mark();
    archived.imprint_key = "archived".into();
    archived.priority = u16::MAX;
    archived.mark_id = archived.stable_mark_id().unwrap();
    let projection = select_cycle_projection(&[
        StoredCycleMark {
            mark: next.clone(),
            status: CycleMarkStatus::Active,
        },
        StoredCycleMark {
            mark: archived,
            status: CycleMarkStatus::Archived,
        },
        StoredCycleMark {
            mark: highest.clone(),
            status: CycleMarkStatus::Active,
        },
    ]);

    assert_eq!(projection.marks.len(), 2);
    assert_eq!(projection.marks[0].mark_id, highest.mark_id);
    assert_eq!(projection.marks[1].mark_id, next.mark_id);
    assert_eq!(
        projection.summary_bytes,
        highest.summary.len() + next.summary.len()
    );
}
