use super::*;
use crate::model::new_id;

fn context() -> ProposalContext {
    let subject = SubjectRef::Character { id: "guard".into() };
    let entry = new_id();
    ProposalContext {
        source_ids: ["s1".into(), "s2".into()].into(),
        entity_ids: ["guard".into(), "harbor".into()].into(),
        subjects: [SubjectRef::PlayerPreference, subject.clone()].into(),
        evidence: [(
            "e1".into(),
            EvidenceSnapshot {
                subject_id: "guard".into(),
                source_id: "s1".into(),
            },
        )]
        .into(),
        targets: [(
            entry.clone(),
            TargetSnapshot {
                entry_id: entry,
                version_id: new_id(),
                state_revision: 3,
                subject: Subject::Character { id: "guard".into() },
                dimension: Dimension::Story,
                attitude: Attitude::Unexpressed,
            },
        )]
        .into(),
    }
}

fn preference_candidate() -> Candidate {
    Candidate {
        candidate_id: "c1".into(),
        classification: Classification::New,
        source_ids: vec!["s1".into()],
        subject_ref: SubjectRef::PlayerPreference,
        entity_ids: vec![],
        evidence_ids: vec![],
        target_ref: None,
        dimension: Some(Dimension::PlayerPreference),
        summary: Some("喜欢探索旧港".into()),
        attitude: None,
        attitude_evidence_ids: vec![],
        inferred: Some(false),
        reason: "玩家直接表达".into(),
    }
}

fn character_candidate() -> Candidate {
    Candidate {
        candidate_id: "c1".into(),
        classification: Classification::New,
        source_ids: vec!["s1".into()],
        subject_ref: SubjectRef::Character { id: "guard".into() },
        entity_ids: vec!["harbor".into()],
        evidence_ids: vec!["e1".into()],
        target_ref: None,
        dimension: Some(Dimension::Story),
        summary: Some("守卫听到了钟声".into()),
        attitude: Some(Attitude::Unexpressed),
        attitude_evidence_ids: vec![],
        inferred: Some(false),
        reason: "本批可信证据".into(),
    }
}

fn proposal(candidate: Candidate) -> Proposal {
    Proposal {
        version: 1,
        candidates: vec![candidate],
    }
}

fn target_ref(context: &ProposalContext) -> TargetRef {
    let target = context.targets.values().next().unwrap();
    TargetRef {
        entry_id: target.entry_id.clone(),
        expected_version_id: target.version_id.clone(),
        expected_state_revision: target.state_revision,
    }
}

#[test]
fn validates_player_preferences_and_character_evidence_without_granting_extra_fields() {
    let context = context();
    assert!(proposal(preference_candidate()).validate(&context).is_ok());
    assert!(proposal(character_candidate()).validate(&context).is_ok());

    let mut update = character_candidate();
    update.classification = Classification::Supplement;
    update.target_ref = Some(target_ref(&context));
    update.attitude = Some(Attitude::Believed);
    update.attitude_evidence_ids = vec!["e1".into()];
    assert!(proposal(update).validate(&context).is_ok());
}

#[test]
fn parse_rejects_duplicate_keys_unknown_fields_and_oversize_documents() {
    let context = context();
    assert!(Proposal::parse(br#"{"version":1,"version":1,"candidates":[]}"#, &context).is_err());
    assert!(
        Proposal::parse(
            br#"{"version":1,"candidates":[],"debug":"untrusted"}"#,
            &context
        )
        .is_err()
    );
    assert!(Proposal::parse(&vec![b' '; MAX_PROPOSAL_BYTES + 1], &context).is_err());
    let mut unsupported = proposal(preference_candidate());
    unsupported.version = 2;
    assert!(matches!(
        unsupported.validate(&context),
        Err(crate::error::Error::Rejected(Reason::PolicyUnavailable))
    ));
}

#[test]
fn references_are_unique_and_must_belong_to_the_frozen_batch() {
    let context = context();
    let mut candidate = preference_candidate();
    candidate.source_ids.push("s1".into());
    assert!(proposal(candidate.clone()).validate(&context).is_err());
    candidate.source_ids = vec!["s3".into()];
    assert!(proposal(candidate).validate(&context).is_err());

    let mut candidate = character_candidate();
    candidate.evidence_ids = vec!["forged".into()];
    assert!(proposal(candidate.clone()).validate(&context).is_err());
    candidate = character_candidate();
    candidate.evidence_ids = vec!["e1".into(), "e1".into()];
    assert!(proposal(candidate.clone()).validate(&context).is_err());
    candidate = character_candidate();
    candidate.subject_ref = SubjectRef::Character { id: "other".into() };
    assert!(proposal(candidate).validate(&context).is_err());
}

#[test]
fn target_cas_and_character_attitude_require_current_evidence() {
    let context = context();
    let mut candidate = character_candidate();
    candidate.classification = Classification::Correction;
    candidate.target_ref = Some(target_ref(&context));
    candidate.attitude = Some(Attitude::Believed);
    assert!(proposal(candidate.clone()).validate(&context).is_err());
    candidate.attitude_evidence_ids = vec!["e1".into()];
    assert!(proposal(candidate.clone()).validate(&context).is_ok());

    candidate
        .target_ref
        .as_mut()
        .unwrap()
        .expected_state_revision += 1;
    assert!(matches!(
        proposal(candidate).validate(&context),
        Err(crate::error::Error::Rejected(Reason::StaleVersion))
    ));
}

#[test]
fn repeat_not_worthy_and_uncertain_cannot_smuggle_content_changes() {
    let context = context();
    let target = target_ref(&context);
    let mut repeat = character_candidate();
    repeat.classification = Classification::Repeat;
    repeat.target_ref = Some(target);
    repeat.dimension = None;
    repeat.summary = None;
    repeat.attitude = None;
    repeat.inferred = None;
    assert!(proposal(repeat.clone()).validate(&context).is_ok());
    repeat.summary = Some("覆盖旧正文".into());
    assert!(proposal(repeat).validate(&context).is_err());

    for classification in [Classification::NotWorthy, Classification::Uncertain] {
        let mut candidate = character_candidate();
        candidate.classification = classification;
        candidate.evidence_ids.clear();
        candidate.entity_ids.clear();
        candidate.dimension = None;
        candidate.attitude = None;
        candidate.inferred = None;
        candidate.summary =
            (classification == Classification::Uncertain).then(|| "不确定是否指向旧港".into());
        assert!(proposal(candidate.clone()).validate(&context).is_ok());
        candidate.target_ref = Some(target_ref(&context));
        if classification == Classification::NotWorthy {
            assert!(proposal(candidate).validate(&context).is_err());
        } else {
            assert!(proposal(candidate).validate(&context).is_ok());
        }
    }
}

#[test]
fn candidate_ids_and_dimensions_are_bounded_and_subject_scoped() {
    let context = context();
    let mut candidate = preference_candidate();
    candidate.candidate_id = "bad id".into();
    assert!(proposal(candidate.clone()).validate(&context).is_err());
    candidate.candidate_id = "c1".into();
    candidate.dimension = Some(Dimension::Story);
    assert!(proposal(candidate.clone()).validate(&context).is_err());
    candidate = preference_candidate();
    candidate.summary = Some(" ".into());
    assert!(proposal(candidate).validate(&context).is_err());

    let candidate = preference_candidate();
    let mut duplicate = candidate.clone();
    duplicate.candidate_id = candidate.candidate_id.clone();
    assert!(
        Proposal {
            version: 1,
            candidates: vec![candidate, duplicate]
        }
        .validate(&context)
        .is_err()
    );
}
