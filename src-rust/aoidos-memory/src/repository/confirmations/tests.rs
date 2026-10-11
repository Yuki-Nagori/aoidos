use super::super::test_support::*;
use super::*;

fn draft(f: &Fixture, candidates: &[&str]) -> QuestionDraft {
    QuestionDraft {
        version: 1,
        identity: QuestionIdentity {
            kind: QuestionKind::EntityAssociation,
            event_slot: 2,
            sources: vec![f.source()],
        },
        history_revision: f.port.view.history_revision,
        entity_revision: f.port.view.entity_revision,
        evidence_revision: f.port.view.evidence_revision,
        candidates: candidates
            .iter()
            .map(|candidate| QuestionCandidate::Entity {
                entity_id: (*candidate).into(),
            })
            .collect(),
    }
}

fn open(f: &mut Fixture, candidates: &[&str]) -> Result<MemoryQuestion> {
    let draft = draft(f, candidates);
    open_draft(f, &draft)
}

fn open_draft(f: &mut Fixture, draft: &QuestionDraft) -> Result<MemoryQuestion> {
    let run_id = f.run.run_id.clone();
    f.repo.open_question(&mut f.conn, &f.port, &run_id, draft)
}

fn resolution(question: &MemoryQuestion, answer: QuestionAnswer) -> QuestionResolution {
    QuestionResolution {
        question_id: question.question_id.clone(),
        expected_revision: question.revision,
        expected_history_revision: question.draft.history_revision,
        answer,
    }
}

#[test]
fn repeated_question_is_idempotent_and_candidate_changes_advance_revision() {
    let mut f = Fixture::new();
    let first = open(&mut f, &["keeper", "guard"]).unwrap();
    let repeated = open(&mut f, &["keeper", "guard"]).unwrap();
    assert_eq!(first, repeated);

    let changed = open(&mut f, &["keeper"]).unwrap();
    assert_eq!(changed.question_id, first.question_id);
    assert_eq!(changed.question_key, first.question_key);
    assert_eq!(changed.revision, first.revision + 1);
    assert_eq!(
        changed.draft.candidates,
        [QuestionCandidate::Entity {
            entity_id: "keeper".into()
        }]
    );
}

#[test]
fn source_order_does_not_create_a_second_identity_for_the_same_event() {
    let mut f = Fixture::new();
    let first_source = f.source();
    let mut second_source = first_source.clone();
    second_source.record_seq += 1;
    let mut first = draft(&f, &["keeper"]);
    first.identity.sources = vec![first_source.clone(), second_source.clone()];
    let mut reordered = first.clone();
    reordered.identity.sources = vec![second_source, first_source];
    let first_question = open_draft(&mut f, &first).unwrap();
    let repeated = open_draft(&mut f, &reordered).unwrap();
    assert_eq!(first_question.question_id, repeated.question_id);
    assert_eq!(first_question.revision, repeated.revision);
}

#[test]
fn resolution_is_cas_guarded_and_cannot_select_an_unoffered_entity() {
    let mut f = Fixture::new();
    let question = open(&mut f, &["keeper", "guard"]).unwrap();
    rejected(
        f.repo.resolve_question(
            &mut f.conn,
            &f.port,
            &f.run.run_id,
            resolution(
                &question,
                QuestionAnswer::SelectCandidate {
                    candidate: QuestionCandidate::Entity {
                        entity_id: "outsider".into(),
                    },
                },
            ),
        ),
        Reason::InvalidSchema,
    );
    let resolved = f
        .repo
        .resolve_question(
            &mut f.conn,
            &f.port,
            &f.run.run_id,
            resolution(
                &question,
                QuestionAnswer::SelectCandidate {
                    candidate: QuestionCandidate::Entity {
                        entity_id: "keeper".into(),
                    },
                },
            ),
        )
        .unwrap();
    assert_eq!(resolved.status, QuestionStatus::Resolved);
    assert_eq!(
        f.repo
            .question(&f.conn, &f.run.run_id, &question.question_id)
            .unwrap(),
        resolved
    );
    rejected(
        f.repo.resolve_question(
            &mut f.conn,
            &f.port,
            &f.run.run_id,
            resolution(&question, QuestionAnswer::Skip),
        ),
        Reason::StaleVersion,
    );
}

#[test]
fn changed_history_rejects_a_stale_answer_without_resolving_the_question() {
    let mut f = Fixture::new();
    let question = open(&mut f, &["keeper"]).unwrap();
    f.port.view.history_revision += 1;
    assert!(matches!(
        f.repo.resolve_question(
            &mut f.conn,
            &f.port,
            &f.run.run_id,
            resolution(&question, QuestionAnswer::Skip),
        ),
        Err(Error::Rejected(Reason::StaleVersion))
    ));
    assert_eq!(
        f.repo
            .question(&f.conn, &f.run.run_id, &question.question_id)
            .unwrap()
            .status,
        QuestionStatus::Pending
    );
}

#[test]
fn proposed_names_remain_suggestions_instead_of_creating_entities() {
    let mut f = Fixture::new();
    let question = open(&mut f, &["keeper"]).unwrap();
    let resolved = f
        .repo
        .resolve_question(
            &mut f.conn,
            &f.port,
            &f.run.run_id,
            resolution(
                &question,
                QuestionAnswer::ProposeEntity {
                    display_name: "新角色".into(),
                },
            ),
        )
        .unwrap();
    assert_eq!(
        resolved.answer,
        Some(QuestionAnswer::ProposeEntity {
            display_name: "新角色".into()
        })
    );
    let table_count: u32 = f
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name LIKE '%entity%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_count, 0);
}

#[test]
fn invalid_sources_and_candidate_shapes_are_rejected() {
    let mut f = Fixture::new();
    f.port.invalid.set(true);
    assert!(matches!(
        open(&mut f, &["keeper"]),
        Err(Error::Rejected(Reason::InvalidSource))
    ));
    f.port.invalid.set(false);
    let run_id = f.run.run_id.clone();
    let duplicate = draft(&f, &["keeper", "keeper"]);
    assert!(matches!(
        f.repo
            .open_question(&mut f.conn, &f.port, &run_id, &duplicate),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
    let too_long = QuestionDraft {
        candidates: vec![QuestionCandidate::Entity {
            entity_id: "x".repeat(65),
        }],
        ..draft(&f, &["keeper"])
    };
    assert!(matches!(
        f.repo
            .open_question(&mut f.conn, &f.port, &run_id, &too_long),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
}

#[test]
fn pending_question_capacity_is_enforced_atomically_per_run() {
    let mut f = Fixture::new();
    for event_slot in 0..MAX_PENDING_QUESTIONS_PER_RUN as u32 {
        let mut draft = draft(&f, &["keeper"]);
        draft.identity.event_slot = event_slot;
        open_draft(&mut f, &draft).unwrap();
    }
    let mut overflow = draft(&f, &["keeper"]);
    overflow.identity.event_slot = MAX_PENDING_QUESTIONS_PER_RUN as u32;
    assert!(matches!(
        open_draft(&mut f, &overflow),
        Err(Error::Rejected(Reason::CapacityBlocked))
    ));
    let pending: u64 = f
        .conn
        .query_row(
            "SELECT count(*) FROM memory_questions WHERE run_id=?1 AND status='pending'",
            [&f.run.run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, MAX_PENDING_QUESTIONS_PER_RUN);
}

#[test]
fn classification_questions_only_accept_classification_candidates() {
    let mut f = Fixture::new();
    let mut invalid = draft(&f, &["keeper"]);
    invalid.identity.kind = QuestionKind::MemoryClassification;
    assert!(matches!(
        open_draft(&mut f, &invalid),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
    let mut valid = invalid;
    valid.candidates = vec![QuestionCandidate::MemoryClassification {
        classification: Classification::Uncertain,
    }];
    let question = open_draft(&mut f, &valid).unwrap();
    assert!(matches!(
        f.repo.resolve_question(
            &mut f.conn,
            &f.port,
            &f.run.run_id,
            resolution(
                &question,
                QuestionAnswer::ProposeEntity {
                    display_name: "不适用".into(),
                },
            ),
        ),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
    let resolved = f
        .repo
        .resolve_question(
            &mut f.conn,
            &f.port,
            &f.run.run_id,
            resolution(
                &question,
                QuestionAnswer::SelectCandidate {
                    candidate: QuestionCandidate::MemoryClassification {
                        classification: Classification::Uncertain,
                    },
                },
            ),
        )
        .unwrap();
    assert_eq!(resolved.status, QuestionStatus::Resolved);
}

#[test]
fn skipped_question_stays_pending_and_can_be_answered_later() {
    let mut f = Fixture::new();
    let question = open(&mut f, &["keeper"]).unwrap();
    let skipped = f
        .repo
        .resolve_question(
            &mut f.conn,
            &f.port,
            &f.run.run_id,
            resolution(&question, QuestionAnswer::Skip),
        )
        .unwrap();
    assert_eq!(skipped, question);
    assert_eq!(
        f.repo
            .question(&f.conn, &f.run.run_id, &question.question_id)
            .unwrap(),
        question
    );
    let answered = f
        .repo
        .resolve_question(
            &mut f.conn,
            &f.port,
            &f.run.run_id,
            resolution(&question, QuestionAnswer::None),
        )
        .unwrap();
    assert_eq!(answered.status, QuestionStatus::Resolved);
    assert_eq!(answered.answer, Some(QuestionAnswer::None));
}

#[test]
fn invalid_resolution_ids_and_revoked_sources_leave_questions_pending() {
    let mut f = Fixture::new();
    let question = open(&mut f, &["keeper"]).unwrap();
    let mut invalid = resolution(&question, QuestionAnswer::None);
    invalid.question_id = "invalid".into();
    rejected(
        f.repo
            .resolve_question(&mut f.conn, &f.port, &f.run.run_id, invalid),
        Reason::InvalidSchema,
    );
    rejected(
        f.repo.question(&f.conn, "invalid", &question.question_id),
        Reason::InvalidSchema,
    );
    assert!(matches!(
        f.repo.question(&f.conn, &f.run.run_id, &new_id()),
        Err(Error::NotFound)
    ));
    f.port.invalid.set(true);
    rejected(
        f.repo.resolve_question(
            &mut f.conn,
            &f.port,
            &f.run.run_id,
            resolution(&question, QuestionAnswer::None),
        ),
        Reason::InvalidSource,
    );
    assert_eq!(
        f.repo
            .question(&f.conn, &f.run.run_id, &question.question_id)
            .unwrap(),
        question
    );
}

#[test]
fn draft_revision_source_and_candidate_boundaries_reject_before_persistence() {
    let mut f = Fixture::new();
    let valid = draft(&f, &["keeper"]);
    let mut invalids = Vec::new();
    let mut empty = valid.clone();
    empty.identity.sources.clear();
    invalids.push(empty);
    let mut duplicates = valid.clone();
    duplicates.identity.sources.push(f.source());
    invalids.push(duplicates);
    let mut malformed = valid.clone();
    malformed.identity.sources[0].session_id = "invalid".into();
    invalids.push(malformed);
    let mut oversized = valid.clone();
    oversized.identity.event_slot = 1_000_001;
    invalids.push(oversized);
    let mut outdated = valid.clone();
    outdated.entity_revision += 1;
    invalids.push(outdated);
    let mut overflow = valid.clone();
    overflow.candidates = (0..=MAX_QUESTION_CANDIDATES)
        .map(|i| QuestionCandidate::Entity {
            entity_id: format!("entity-{i}"),
        })
        .collect();
    invalids.push(overflow);
    for invalid in invalids {
        rejected(open_draft(&mut f, &invalid), Reason::InvalidSchema);
    }
    let count: u64 = f
        .conn
        .query_row("SELECT count(*) FROM memory_questions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn revision_overflow_and_sqlite_update_races_preserve_existing_question() {
    let mut f = Fixture::new();
    let question = open(&mut f, &["keeper", "guard"]).unwrap();
    f.conn
        .execute(
            "UPDATE memory_questions SET revision=?1 WHERE question_id=?2",
            params![MAX_INTEGER, question.question_id],
        )
        .unwrap();
    rejected(open(&mut f, &["keeper"]), Reason::CapacityBlocked);
    f.conn
        .execute(
            "UPDATE memory_questions SET revision=1 WHERE question_id=?1",
            [&question.question_id],
        )
        .unwrap();
    f.conn.execute_batch("CREATE TEMP TRIGGER reject_question_update BEFORE UPDATE ON memory_questions BEGIN SELECT RAISE(IGNORE); END;").unwrap();
    rejected(open(&mut f, &["keeper"]), Reason::StaleVersion);
    rejected(
        f.repo.resolve_question(
            &mut f.conn,
            &f.port,
            &f.run.run_id,
            resolution(&question, QuestionAnswer::None),
        ),
        Reason::StaleVersion,
    );
    assert_eq!(
        f.repo
            .question(&f.conn, &f.run.run_id, &question.question_id)
            .unwrap(),
        question
    );
}

#[test]
fn persisted_question_key_draft_and_answer_corruption_are_rejected() {
    let mut f = Fixture::new();
    let question = open(&mut f, &["keeper"]).unwrap();
    let original_draft = canonical(&question.draft).unwrap();
    f.conn
        .execute(
            "UPDATE memory_questions SET question_key=zeroblob(32) WHERE question_id=?1",
            [&question.question_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo
            .question(&f.conn, &f.run.run_id, &question.question_id),
        Err(Error::Corrupt)
    ));
    f.conn
        .execute(
            "UPDATE memory_questions SET question_key=?1 WHERE question_id=?2",
            params![question.question_key.as_slice(), question.question_id],
        )
        .unwrap();
    let mut invalid = question.draft.clone();
    invalid.candidates.clear();
    f.conn
        .execute(
            "UPDATE memory_questions SET draft_json=?1 WHERE question_id=?2",
            params![canonical(&invalid).unwrap(), question.question_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo
            .question(&f.conn, &f.run.run_id, &question.question_id),
        Err(Error::Corrupt)
    ));
    f.conn.execute("UPDATE memory_questions SET draft_json=?1,status='resolved',answer_json=?2 WHERE question_id=?3",
        params![original_draft, canonical(&QuestionAnswer::SelectCandidate { candidate: QuestionCandidate::Entity { entity_id: "outsider".into() } }).unwrap(), question.question_id]).unwrap();
    assert!(matches!(
        f.repo
            .question(&f.conn, &f.run.run_id, &question.question_id),
        Err(Error::Corrupt)
    ));
    f.conn
        .execute(
            "UPDATE memory_questions SET answer_json='wrong-type' WHERE question_id=?1",
            [&question.question_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo
            .question(&f.conn, &f.run.run_id, &question.question_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn optional_answer_blob_requires_matching_type_length_and_limit() {
    let conn = Connection::open_in_memory().unwrap();
    for (length, bytes, expected) in [
        (None, None, None),
        (Some(2_i64), Some(vec![1_u8, 2]), Some(vec![1, 2])),
        (Some(-1), Some(vec![1]), Some(vec![])),
        (Some(1), Some(vec![1, 2]), Some(vec![])),
        (Some(3), Some(vec![1, 2, 3]), Some(vec![])),
    ] {
        let actual = conn
            .query_row("SELECT ?1, ?2", params![length, bytes], |row| {
                read_optional_bounded_blob(row, 0, 1, 2)
            })
            .unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn persisted_draft_revisions_must_match_columns_and_empty_sources_are_corrupt() {
    let mut f = Fixture::new();
    let question = open(&mut f, &["keeper"]).unwrap();
    for column in ["history_revision", "entity_revision", "evidence_revision"] {
        f.conn
            .execute(
                &format!("UPDATE memory_questions SET {column}={column}+1 WHERE question_id=?1"),
                [&question.question_id],
            )
            .unwrap();
        assert!(matches!(
            f.repo
                .question(&f.conn, &f.run.run_id, &question.question_id),
            Err(Error::Corrupt)
        ));
        f.conn
            .execute(
                &format!("UPDATE memory_questions SET {column}={column}-1 WHERE question_id=?1"),
                [&question.question_id],
            )
            .unwrap();
    }
    let mut invalid = question.draft.clone();
    invalid.identity.sources.clear();
    let key = build_question_key(&f.run.run_id, &invalid.identity).unwrap();
    f.conn
        .execute(
            "UPDATE memory_questions SET draft_json=?1,question_key=?2 WHERE question_id=?3",
            params![
                canonical(&invalid).unwrap(),
                key.as_slice(),
                question.question_id
            ],
        )
        .unwrap();
    assert!(matches!(
        f.repo
            .question(&f.conn, &f.run.run_id, &question.question_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn opening_question_rejects_noncanonical_persisted_source_order() {
    let mut f = Fixture::new();
    let mut input = draft(&f, &["keeper"]);
    let mut second = f.source();
    second.record_seq += 1;
    input.identity.sources.push(second);
    let question = open_draft(&mut f, &input).unwrap();
    let mut altered = question.draft.clone();
    altered.identity.sources.reverse();
    assert_ne!(altered.identity, question.draft.identity);
    assert_eq!(
        build_question_key(&f.run.run_id, &altered.identity).unwrap(),
        question.question_key
    );
    f.conn
        .execute(
            "UPDATE memory_questions SET draft_json=?1 WHERE question_id=?2",
            params![canonical(&altered).unwrap(), question.question_id],
        )
        .unwrap();
    assert!(matches!(open_draft(&mut f, &input), Err(Error::Corrupt)));
}
