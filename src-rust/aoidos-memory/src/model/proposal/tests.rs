use super::*;
use crate::model::{EntityCategory, InputMode, KnowledgeKind, new_id};

fn evidence_snapshot(subject_id: &str, source_id: &str) -> EvidenceSnapshot {
    EvidenceSnapshot {
        event_id: None,
        subject_id: subject_id.into(),
        source_ids: vec![source_id.into()],
        kind: KnowledgeKind::Told,
        uncertainty: None,
        informant_id: Some("player".into()),
        scope: "directDialogue".into(),
        rule_id: "directDialogueV1".into(),
        source: SourceRef {
            script_id: "mistbell".into(),
            run_id: new_id(),
            session_id: new_id(),
            record_seq: 1,
            body_hash: crate::model::hash(b"evidence"),
            start_byte: 0,
            end_byte: 8,
            mode: Some(InputMode::InCharacter),
        },
    }
}

fn context() -> ProposalContext {
    let subject = SubjectRef::Character { id: "guard".into() };
    let entry = new_id();
    ProposalContext {
        source_ids: ["s1".into(), "s2".into()].into(),
        entity_ids: ["guard".into(), "harbor".into()].into(),
        entities: [
            (
                "guard".into(),
                EntitySnapshot {
                    kind: EntityCategory::Character,
                    scope: "old-harbor".into(),
                    canonical_name: "守门人".into(),
                    aliases: Default::default(),
                },
            ),
            (
                "harbor".into(),
                EntitySnapshot {
                    kind: EntityCategory::Place,
                    scope: "global".into(),
                    canonical_name: "旧港".into(),
                    aliases: Default::default(),
                },
            ),
        ]
        .into(),
        subjects: [SubjectRef::PlayerPreference, subject.clone()].into(),
        evidence: [("e1".into(), evidence_snapshot("guard", "s1"))].into(),
        targets: [(
            entry.clone(),
            TargetSnapshot {
                entry_id: entry,
                version_id: new_id(),
                state_revision: 3,
                state: super::super::EntryState::Active,
                subject: Subject::Character { id: "guard".into() },
                dimension: Dimension::Story,
                summary: "在旧港相信守门人".into(),
                attitude: Attitude::Unexpressed,
                inferred: false,
            },
        )]
        .into(),
        target_search: None,
        cycle_rules: BTreeMap::new(),
        cycle_sources: BTreeMap::new(),
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
        cycle_marks: vec![],
    }
}

fn cycle_context() -> ProposalContext {
    let mut context = context();
    let run_id = new_id();
    let session_id = new_id();
    let source = SourceRef {
        script_id: "mistbell".into(),
        run_id: run_id.clone(),
        session_id,
        record_seq: 4,
        body_hash: crate::model::hash(b"a trusted source"),
        start_byte: 0,
        end_byte: 16,
        mode: Some(InputMode::InCharacter),
    };
    context.cycle_sources.insert(
        "s1".into(),
        CycleSourceRef {
            source,
            history_revision: 2,
        },
    );
    context.cycle_rules.insert(
        "rescue".into(),
        CycleMarkAuthorization {
            rule_id: "rescue".into(),
            script_id: "mistbell".into(),
            run_id,
            cycle_gen: 3,
            rule_version: 2,
            allowed_subject_ids: ["guard".into()].into(),
            imprint_key: "harbor-rescue".into(),
            kind: CycleKind::Affinity,
            detail_policy: DetailPolicy::Vague,
            priority: 700,
        },
    );
    context
}

fn cycle_mark_candidate() -> CycleMarkCandidate {
    CycleMarkCandidate {
        candidate_id: "mark_1".into(),
        rule_id: "rescue".into(),
        source_ids: vec!["s1".into()],
        subject_ids: vec!["guard".into()],
        summary: "守卫记得有人曾在港口伸出援手".into(),
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

    let mut inferred_preference = preference_candidate();
    inferred_preference.inferred = Some(true);
    assert!(proposal(inferred_preference).validate(&context).is_err());

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
fn cycle_mark_candidates_use_the_same_request_and_engine_owned_authorization() {
    let context = cycle_context();
    let mut proposal = proposal(preference_candidate());
    proposal.cycle_marks.push(cycle_mark_candidate());
    assert!(proposal.validate(&context).is_ok());

    let mark = proposal.cycle_marks[0].materialize(&context).unwrap();
    let authorization = context.cycle_rules.get("rescue").unwrap();
    assert_eq!(mark.script_id, authorization.script_id);
    assert_eq!(mark.cycle_gen, authorization.cycle_gen);
    assert_eq!(mark.rule_version, authorization.rule_version);
    assert_eq!(mark.imprint_key, authorization.imprint_key);
    assert_eq!(mark.kind, authorization.kind);
    assert_eq!(mark.detail_policy, authorization.detail_policy);
    assert_eq!(mark.priority, authorization.priority);
    assert_eq!(mark.summary, "守卫记得有人曾在港口伸出援手");
    assert_eq!(mark.source_refs.len(), 1);
    assert_eq!(mark.mark_id, mark.stable_mark_id().unwrap());

    let mut revised_summary = proposal.cycle_marks[0].clone();
    revised_summary.summary = "守卫对港口的援助仍有模糊印象".into();
    assert_eq!(
        revised_summary.materialize(&context).unwrap().mark_id,
        mark.mark_id,
        "改写摘要不能生成新的刻痕身份"
    );
}

#[test]
fn cycle_mark_candidates_reject_unregistered_rules_sources_subjects_and_duplicate_identity() {
    let mut context = cycle_context();
    let mut candidate = cycle_mark_candidate();
    candidate.rule_id = "unregistered".into();
    assert!(candidate.materialize(&context).is_err());

    let mut candidate = cycle_mark_candidate();
    candidate.source_ids = vec!["s2".into()];
    assert!(candidate.materialize(&context).is_err());

    let mut candidate = cycle_mark_candidate();
    candidate.subject_ids = vec!["harbor".into()];
    assert!(candidate.materialize(&context).is_err());

    context
        .cycle_rules
        .get_mut("rescue")
        .unwrap()
        .allowed_subject_ids
        .insert("harbor".into());
    let mut first = cycle_mark_candidate();
    first.subject_ids = vec!["guard".into(), "harbor".into()];
    let mut second = first.clone();
    second.candidate_id = "mark_2".into();
    second.subject_ids.reverse();

    let first_mark = first.materialize(&context).unwrap();
    let second_mark = second.materialize(&context).unwrap();
    assert_eq!(
        first_mark.mark_id, second_mark.mark_id,
        "主体顺序不应改变刻痕的稳定身份"
    );

    let mut proposal = proposal(preference_candidate());
    proposal.cycle_marks = vec![first, second];
    assert!(proposal.validate(&context).is_err());
}

#[test]
fn old_memory_proposal_without_cycle_marks_remains_valid() {
    let context = context();
    let parsed = Proposal::parse(br#"{"version":1,"candidates":[]}"#, &context).unwrap();
    assert!(parsed.cycle_marks.is_empty());
}

#[test]
fn cycle_mark_wire_cannot_override_engine_owned_authorization_fields() {
    let context = cycle_context();
    let bytes = br#"{"version":1,"candidates":[],"cycleMarks":[{"candidateId":"mark_1","ruleId":"rescue","sourceIds":["s1"],"subjectIds":["guard"],"summary":"A memory","priority":1000,"kind":"fragment"}]}"#;
    assert!(Proposal::parse(bytes, &context).is_err());
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
            candidates: vec![candidate, duplicate],
            cycle_marks: vec![],
        }
        .validate(&context)
        .is_err()
    );
}

#[test]
fn bounded_json_roundtrips_and_candidate_capacity_is_exact() {
    let context = context();
    let mut value = proposal(preference_candidate());
    assert_eq!(
        Proposal::parse(&serde_json::to_vec(&value).unwrap(), &context).unwrap(),
        value
    );
    value.candidates = (0..MAX_CANDIDATES)
        .map(|i| Candidate {
            candidate_id: format!("c{i}"),
            ..preference_candidate()
        })
        .collect();
    assert!(value.validate(&context).is_ok());
    value.candidates.push(Candidate {
        candidate_id: "overflow".into(),
        ..preference_candidate()
    });
    assert!(matches!(
        value.validate(&context),
        Err(Error::Rejected(Reason::CapacityBlocked))
    ));
}

#[test]
fn cycle_marks_share_the_total_candidate_capacity() {
    let proposal = Proposal {
        version: 1,
        candidates: (0..MAX_CANDIDATES - MAX_CYCLE_MARK_PROPOSALS)
            .map(|index| Candidate {
                candidate_id: format!("c{index}"),
                ..preference_candidate()
            })
            .collect(),
        cycle_marks: (0..MAX_CYCLE_MARK_PROPOSALS)
            .map(|index| CycleMarkCandidate {
                candidate_id: format!("m{index}"),
                ..cycle_mark_candidate()
            })
            .collect(),
    };
    assert!(crate::model::validate_proposal_identity(&proposal).is_ok());

    let mut over_capacity = proposal;
    over_capacity.candidates.push(Candidate {
        candidate_id: "overflow".into(),
        ..preference_candidate()
    });
    assert!(matches!(
        crate::model::validate_proposal_identity(&over_capacity),
        Err(Error::Rejected(Reason::CapacityBlocked))
    ));
}

#[test]
fn cycle_mark_summaries_share_the_projected_byte_budget() {
    let proposal = |first_summary_len: usize| Proposal {
        version: 1,
        candidates: vec![],
        cycle_marks: (0..MAX_CYCLE_MARK_PROPOSALS)
            .map(|index| CycleMarkCandidate {
                candidate_id: format!("m{index}"),
                summary: "x".repeat(if index == 0 { first_summary_len } else { 256 }),
                ..cycle_mark_candidate()
            })
            .collect(),
    };
    assert!(crate::model::validate_proposal_identity(&proposal(256)).is_ok());
    assert!(matches!(
        crate::model::validate_proposal_identity(&proposal(257)),
        Err(Error::Rejected(Reason::CapacityBlocked))
    ));
}

#[test]
fn evidence_must_match_subject_source_and_attitude_subset() {
    let mut context = context();
    context
        .evidence
        .insert("e2".into(), evidence_snapshot("guard", "s2"));
    let mut candidate = character_candidate();
    candidate.evidence_ids.clear();
    assert!(proposal(candidate.clone()).validate(&context).is_err());
    candidate.evidence_ids = vec!["e2".into()];
    assert!(proposal(candidate.clone()).validate(&context).is_err());
    candidate.source_ids.push("s2".into());
    assert!(proposal(candidate.clone()).validate(&context).is_ok());
    candidate.attitude_evidence_ids = vec!["e1".into()];
    assert!(proposal(candidate.clone()).validate(&context).is_err());
    candidate.attitude_evidence_ids.clear();
    context.evidence.get_mut("e2").unwrap().subject_id = "harbor".into();
    assert!(proposal(candidate).validate(&context).is_err());
    for attitude_only in [false, true] {
        let mut preference = preference_candidate();
        if attitude_only {
            preference.attitude_evidence_ids = vec!["e1".into()];
        } else {
            preference.evidence_ids = vec!["e1".into()];
        }
        assert!(proposal(preference).validate(&context).is_err());
    }
    let mut missing = character_candidate();
    missing.evidence_ids = vec!["missing".into()];
    assert!(validate_evidence(&missing, &context).is_err());
}

#[test]
fn target_references_require_valid_ids_registered_targets_and_matching_subjects() {
    let context = context();
    let mut candidate = character_candidate();
    candidate.classification = Classification::Correction;
    assert!(proposal(candidate.clone()).validate(&context).is_err());
    for reference in [
        TargetRef {
            entry_id: "invalid".into(),
            ..target_ref(&context)
        },
        TargetRef {
            expected_version_id: "invalid".into(),
            ..target_ref(&context)
        },
        TargetRef {
            expected_state_revision: MAX_INTEGER + 1,
            ..target_ref(&context)
        },
        TargetRef {
            entry_id: new_id(),
            ..target_ref(&context)
        },
    ] {
        candidate.target_ref = Some(reference);
        assert!(proposal(candidate.clone()).validate(&context).is_err());
    }
    candidate.target_ref = Some(target_ref(&context));
    candidate.subject_ref = SubjectRef::PlayerPreference;
    candidate.evidence_ids.clear();
    assert!(matches!(
        proposal(candidate).validate(&context),
        Err(Error::Rejected(Reason::StaleVersion))
    ));
    assert!(
        !SubjectRef::Character { id: "guard".into() }.matches_subject(&Subject::PlayerPreference)
    );
    assert!(SubjectRef::PlayerPreference.matches_subject(&Subject::PlayerPreference));
}

#[test]
fn proposal_context_accepts_128_targets_and_rejects_129() {
    let mut context = context();
    let template = context.targets.values().next().unwrap().clone();
    for _ in context.targets.len()..64 {
        let entry_id = new_id();
        let mut target = template.clone();
        target.entry_id = entry_id.clone();
        target.version_id = new_id();
        context.targets.insert(entry_id, target);
    }
    assert_eq!(context.targets.len(), 64);
    assert!(proposal(character_candidate()).validate(&context).is_ok());

    for _ in 64..65 {
        let entry_id = new_id();
        let mut target = template.clone();
        target.entry_id = entry_id.clone();
        target.version_id = new_id();
        context.targets.insert(entry_id, target);
    }
    assert_eq!(context.targets.len(), 65);
    assert!(proposal(character_candidate()).validate(&context).is_ok());

    for _ in 65..MAX_PROPOSAL_TARGETS {
        let entry_id = new_id();
        let mut target = template.clone();
        target.entry_id = entry_id.clone();
        target.version_id = new_id();
        context.targets.insert(entry_id, target);
    }
    assert_eq!(context.targets.len(), MAX_PROPOSAL_TARGETS);
    assert!(proposal(character_candidate()).validate(&context).is_ok());

    let entry_id = new_id();
    let mut target = template;
    target.entry_id = entry_id.clone();
    context.targets.insert(entry_id, target);
    assert!(matches!(
        proposal(character_candidate()).validate(&context),
        Err(Error::Rejected(Reason::CapacityBlocked))
    ));
}

#[test]
fn unchanged_attitude_reuses_target_evidence_and_invalid_shapes_are_rejected() {
    let mut context = context();
    context.targets.values_mut().next().unwrap().attitude = Attitude::Believed;
    let mut correction = character_candidate();
    correction.classification = Classification::Correction;
    correction.target_ref = Some(target_ref(&context));
    correction.attitude = Some(Attitude::Believed);
    assert!(proposal(correction.clone()).validate(&context).is_ok());
    correction.summary = None;
    assert!(proposal(correction).validate(&context).is_err());
    let mut uncertain = preference_candidate();
    uncertain.classification = Classification::Uncertain;
    uncertain.dimension = None;
    uncertain.inferred = None;
    assert!(proposal(uncertain.clone()).validate(&context).is_ok());
    uncertain.summary = None;
    assert!(proposal(uncertain).validate(&context).is_err());
}
