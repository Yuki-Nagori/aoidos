use super::*;
use crate::repository::test_support::Fixture;

const ENTITY_ID: &str = "keeper";
const OTHER_ENTITY_ID: &str = "visitor";
const ALIAS_NAME: &str = "钟";
const SCOPE: &str = "mistbell";

fn entity_context(entity_id: &str, canonical_name: &str) -> EntityAliasContext {
    EntityAliasContext {
        entity_id: entity_id.into(),
        category: EntityCategory::Character,
        scope: SCOPE.into(),
        canonical_name: canonical_name.into(),
    }
}

fn entity_question(fixture: &Fixture, event_slot: u32, entity_id: &str) -> QuestionDraft {
    QuestionDraft {
        version: 1,
        identity: QuestionIdentity {
            kind: QuestionKind::EntityAssociation,
            event_slot,
            sources: vec![fixture.source()],
        },
        history_revision: fixture.port.view.history_revision,
        entity_revision: fixture.port.view.entity_revision,
        evidence_revision: fixture.port.view.evidence_revision,
        mentioned_name: Some(ALIAS_NAME.into()),
        candidates: vec![QuestionCandidate::Entity {
            entity_id: entity_id.into(),
        }],
    }
}

fn select_question(draft: &QuestionDraft) -> QuestionAnswer {
    QuestionAnswer::SelectCandidate {
        candidate: draft.candidates[0].clone(),
    }
}

fn confirm_alias(fixture: &mut Fixture, event_slot: u32, entity_id: &str) -> EntityAlias {
    fixture
        .port
        .alias_entities
        .borrow_mut()
        .insert(entity_id.into(), entity_context(entity_id, "守夜人"));
    let draft = entity_question(fixture, event_slot, entity_id);
    let question = fixture
        .repo
        .open_question(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            &draft,
        )
        .unwrap();
    let resolution = QuestionResolution {
        question_id: question.question_id,
        expected_revision: question.revision,
        expected_history_revision: question.draft.history_revision,
        answer: select_question(&draft),
    };
    fixture
        .repo
        .resolve_question(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            resolution,
        )
        .unwrap();
    fixture
        .repo
        .resolve_alias(
            &fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            EntityCategory::Character,
            SCOPE,
            ALIAS_NAME,
        )
        .unwrap()
        .unwrap()
}

#[test]
fn explicit_entity_confirmation_registers_exact_alias_and_sources_atomically() {
    let mut fixture = Fixture::new();
    let alias = confirm_alias(&mut fixture, 1, ENTITY_ID);
    assert_eq!(alias.entity_id, ENTITY_ID);
    assert_eq!(alias.name, ALIAS_NAME);
    assert_eq!(alias.category, EntityCategory::Character);
    assert_eq!(alias.scope, SCOPE);
    assert_eq!(alias.status, AliasStatus::Active);
    assert_eq!(alias.revision, 1);
    assert_eq!(alias.sources, vec![fixture.source()]);
    assert!(
        fixture
            .repo
            .resolve_alias(
                &fixture.conn,
                &fixture.port,
                &fixture.run.run_id,
                EntityCategory::Place,
                SCOPE,
                ALIAS_NAME,
            )
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .repo
            .resolve_alias(
                &fixture.conn,
                &fixture.port,
                &fixture.run.run_id,
                EntityCategory::Character,
                SCOPE,
                "钟声",
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn separate_confirmations_add_distinct_support_without_duplicate_retries() {
    let mut fixture = Fixture::new();
    let first = confirm_alias(&mut fixture, 20, ENTITY_ID);
    let second = confirm_alias(&mut fixture, 21, ENTITY_ID);

    assert_eq!(first.alias_id, second.alias_id);
    assert_eq!(second.revision, first.revision + 1);
    assert_eq!(second.sources, vec![fixture.source(), fixture.source()]);
    assert_eq!(
        fixture
            .conn
            .query_row(
                "SELECT count(*) FROM memory_alias_events WHERE alias_id=?1",
                [&second.alias_id],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        2
    );
}

#[test]
fn canonical_name_conflict_keeps_question_pending_and_does_not_write_alias() {
    let mut fixture = Fixture::new();
    fixture
        .port
        .alias_entities
        .borrow_mut()
        .insert(ENTITY_ID.into(), entity_context(ENTITY_ID, "守夜人"));
    fixture.port.canonical_names.borrow_mut().insert(
        (EntityCategory::Character, SCOPE.into(), ALIAS_NAME.into()),
        OTHER_ENTITY_ID.into(),
    );
    let draft = entity_question(&fixture, 2, ENTITY_ID);
    let question = fixture
        .repo
        .open_question(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            &draft,
        )
        .unwrap();
    let answer = fixture.repo.resolve_question(
        &mut fixture.conn,
        &fixture.port,
        &fixture.run.run_id,
        QuestionResolution {
            question_id: question.question_id.clone(),
            expected_revision: question.revision,
            expected_history_revision: question.draft.history_revision,
            answer: select_question(&draft),
        },
    );
    assert!(matches!(
        answer,
        Err(Error::Rejected(Reason::AmbiguousEntity))
    ));
    assert_eq!(
        fixture
            .repo
            .question(&fixture.conn, &fixture.run.run_id, &question.question_id)
            .unwrap()
            .status,
        QuestionStatus::Pending
    );
    assert!(
        fixture
            .repo
            .resolve_alias(
                &fixture.conn,
                &fixture.port,
                &fixture.run.run_id,
                EntityCategory::Character,
                SCOPE,
                ALIAS_NAME,
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_new_branch_can_confirm_a_name_after_the_old_alias_sources_are_withdrawn() {
    let mut fixture = Fixture::new();
    let old_alias = confirm_alias(&mut fixture, 25, ENTITY_ID);
    fixture
        .port
        .invalid_records
        .borrow_mut()
        .insert(fixture.source().record_seq);
    fixture.port.alias_entities.borrow_mut().insert(
        OTHER_ENTITY_ID.into(),
        entity_context(OTHER_ENTITY_ID, "访客"),
    );

    let mut draft = entity_question(&fixture, 26, OTHER_ENTITY_ID);
    let mut new_branch_source = fixture.source();
    new_branch_source.record_seq = 3;
    draft.identity.sources = vec![new_branch_source.clone()];
    let question = fixture
        .repo
        .open_question(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            &draft,
        )
        .unwrap();
    let resolved = fixture
        .repo
        .resolve_question(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            QuestionResolution {
                question_id: question.question_id.clone(),
                expected_revision: question.revision,
                expected_history_revision: question.draft.history_revision,
                answer: select_question(&draft),
            },
        )
        .unwrap();

    assert_eq!(resolved.status, QuestionStatus::Resolved);
    let current = fixture
        .repo
        .resolve_alias(
            &fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            EntityCategory::Character,
            SCOPE,
            ALIAS_NAME,
        )
        .unwrap()
        .unwrap();
    assert_eq!(current.entity_id, OTHER_ENTITY_ID);
    assert_eq!(current.sources, vec![new_branch_source]);
    let old_status: (String, u64) = fixture
        .conn
        .query_row(
            "SELECT status,revision FROM memory_aliases WHERE alias_id=?1",
            [&old_alias.alias_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(old_status, ("revoked".into(), old_alias.revision + 1));
}

#[test]
fn unrelated_question_sources_do_not_keep_a_withdrawn_name_alias_alive() {
    let mut fixture = Fixture::new();
    fixture
        .port
        .alias_entities
        .borrow_mut()
        .insert(ENTITY_ID.into(), entity_context(ENTITY_ID, "守夜人"));
    fixture
        .port
        .source_text_overrides
        .borrow_mut()
        .insert(2, "雨声".into());
    let mut draft = entity_question(&fixture, 27, ENTITY_ID);
    let mut unrelated = fixture.source();
    unrelated.record_seq = 2;
    unrelated.body_hash = hash("雨声".as_bytes());
    draft.identity.sources.push(unrelated.clone());
    let question = fixture
        .repo
        .open_question(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            &draft,
        )
        .unwrap();
    fixture
        .repo
        .resolve_question(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            QuestionResolution {
                question_id: question.question_id,
                expected_revision: question.revision,
                expected_history_revision: question.draft.history_revision,
                answer: select_question(&draft),
            },
        )
        .unwrap();

    let alias = fixture
        .repo
        .resolve_alias(
            &fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            EntityCategory::Character,
            SCOPE,
            ALIAS_NAME,
        )
        .unwrap()
        .unwrap();
    assert_eq!(alias.sources, vec![fixture.source()]);
    fixture
        .port
        .invalid_records
        .borrow_mut()
        .insert(fixture.source().record_seq);
    assert!(
        fixture
            .repo
            .resolve_alias(
                &fixture.conn,
                &fixture.port,
                &fixture.run.run_id,
                EntityCategory::Character,
                SCOPE,
                ALIAS_NAME,
            )
            .unwrap()
            .is_none()
    );
    assert!(fixture.port.valid_source(&unrelated).unwrap());
}

#[test]
fn confirming_an_existing_canonical_name_resolves_without_creating_an_alias() {
    let mut fixture = Fixture::new();
    fixture
        .port
        .alias_entities
        .borrow_mut()
        .insert(ENTITY_ID.into(), entity_context(ENTITY_ID, ALIAS_NAME));
    fixture.port.canonical_names.borrow_mut().insert(
        (EntityCategory::Character, SCOPE.into(), ALIAS_NAME.into()),
        ENTITY_ID.into(),
    );
    let draft = entity_question(&fixture, 22, ENTITY_ID);
    let question = fixture
        .repo
        .open_question(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            &draft,
        )
        .unwrap();

    let resolved = fixture
        .repo
        .resolve_question(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            QuestionResolution {
                question_id: question.question_id.clone(),
                expected_revision: question.revision,
                expected_history_revision: question.draft.history_revision,
                answer: select_question(&draft),
            },
        )
        .unwrap();

    assert_eq!(resolved.status, QuestionStatus::Resolved);
    assert!(
        fixture
            .repo
            .resolve_alias(
                &fixture.conn,
                &fixture.port,
                &fixture.run.run_id,
                EntityCategory::Character,
                SCOPE,
                ALIAS_NAME,
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .conn
            .query_row("SELECT count(*) FROM memory_aliases", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn an_active_alias_cannot_be_reassigned_to_a_different_entity() {
    let mut fixture = Fixture::new();
    confirm_alias(&mut fixture, 3, ENTITY_ID);
    fixture.port.alias_entities.borrow_mut().insert(
        OTHER_ENTITY_ID.into(),
        entity_context(OTHER_ENTITY_ID, "访客"),
    );
    let draft = entity_question(&fixture, 4, OTHER_ENTITY_ID);
    let question = fixture
        .repo
        .open_question(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            &draft,
        )
        .unwrap();
    let result = fixture.repo.resolve_question(
        &mut fixture.conn,
        &fixture.port,
        &fixture.run.run_id,
        QuestionResolution {
            question_id: question.question_id.clone(),
            expected_revision: question.revision,
            expected_history_revision: question.draft.history_revision,
            answer: select_question(&draft),
        },
    );
    assert!(matches!(
        result,
        Err(Error::Rejected(Reason::AmbiguousEntity))
    ));
    assert_eq!(
        fixture
            .repo
            .resolve_alias(
                &fixture.conn,
                &fixture.port,
                &fixture.run.run_id,
                EntityCategory::Character,
                SCOPE,
                ALIAS_NAME,
            )
            .unwrap()
            .unwrap()
            .entity_id,
        ENTITY_ID
    );
}

#[test]
fn revocation_is_cas_idempotent_and_confirmation_can_reactivate_alias() {
    let mut fixture = Fixture::new();
    let alias = confirm_alias(&mut fixture, 5, ENTITY_ID);
    let revoke = AliasRevocation {
        alias_id: alias.alias_id.clone(),
        expected_revision: alias.revision,
        expected_entity_revision: fixture.port.view.entity_revision,
    };
    let revoked = fixture
        .repo
        .revoke_alias(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            revoke.clone(),
        )
        .unwrap();
    assert_eq!(revoked.status, AliasStatus::Revoked);
    assert_eq!(revoked.revision, 2);
    assert_eq!(
        fixture
            .repo
            .revoke_alias(
                &mut fixture.conn,
                &fixture.port,
                &fixture.run.run_id,
                revoke
            )
            .unwrap(),
        revoked
    );
    assert!(
        fixture
            .repo
            .resolve_alias(
                &fixture.conn,
                &fixture.port,
                &fixture.run.run_id,
                EntityCategory::Character,
                SCOPE,
                ALIAS_NAME,
            )
            .unwrap()
            .is_none()
    );
    let active_again = confirm_alias(&mut fixture, 6, ENTITY_ID);
    assert_eq!(active_again.alias_id, alias.alias_id);
    assert_eq!(active_again.revision, 3);
    assert_eq!(active_again.status, AliasStatus::Active);
}

#[test]
fn alias_lookup_rebuilds_from_any_current_source_after_history_rollback() {
    let mut fixture = Fixture::new();
    confirm_alias(&mut fixture, 7, ENTITY_ID);
    fixture.port.invalid.set(true);
    assert!(
        fixture
            .repo
            .resolve_alias(
                &fixture.conn,
                &fixture.port,
                &fixture.run.run_id,
                EntityCategory::Character,
                SCOPE,
                ALIAS_NAME,
            )
            .unwrap()
            .is_none()
    );
    fixture.port.invalid.set(false);
    assert!(
        fixture
            .repo
            .resolve_alias(
                &fixture.conn,
                &fixture.port,
                &fixture.run.run_id,
                EntityCategory::Character,
                SCOPE,
                ALIAS_NAME,
            )
            .unwrap()
            .is_some()
    );
}

#[test]
fn alias_lookup_rejects_source_rows_with_a_mismatched_hash() {
    let mut fixture = Fixture::new();
    let alias = confirm_alias(&mut fixture, 23, ENTITY_ID);
    fixture
        .conn
        .execute(
            "UPDATE memory_alias_sources SET source_hash=zeroblob(32) WHERE alias_id=?1",
            [&alias.alias_id],
        )
        .unwrap();

    assert!(matches!(
        fixture.repo.resolve_alias(
            &fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            EntityCategory::Character,
            SCOPE,
            ALIAS_NAME,
        ),
        Err(Error::Corrupt)
    ));
}

#[test]
fn source_less_or_unregistered_entity_confirmations_cannot_create_aliases() {
    let mut fixture = Fixture::new();
    let draft = entity_question(&fixture, 8, ENTITY_ID);
    let question = fixture
        .repo
        .open_question(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            &draft,
        )
        .unwrap();
    let result = fixture.repo.resolve_question(
        &mut fixture.conn,
        &fixture.port,
        &fixture.run.run_id,
        QuestionResolution {
            question_id: question.question_id.clone(),
            expected_revision: question.revision,
            expected_history_revision: question.draft.history_revision,
            answer: select_question(&draft),
        },
    );
    assert!(matches!(
        result,
        Err(Error::Rejected(Reason::PolicyUnavailable))
    ));
    assert_eq!(
        fixture
            .repo
            .question(&fixture.conn, &fixture.run.run_id, &question.question_id)
            .unwrap()
            .status,
        QuestionStatus::Pending
    );
}

fn apply_question(fixture: &mut Fixture, question: &MemoryQuestion, entity_id: &str) -> Result<()> {
    let tx = fixture.conn.transaction().unwrap();
    apply_confirmed_alias(
        &tx,
        &fixture.run.run_id,
        &fixture.run,
        question,
        entity_id,
        &fixture.port.view,
        &fixture.port,
    )?;
    tx.commit().map_err(sql)
}

fn opened_question(fixture: &mut Fixture, event_slot: u32) -> MemoryQuestion {
    let draft = entity_question(fixture, event_slot, ENTITY_ID);
    fixture
        .repo
        .open_question(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            &draft,
        )
        .unwrap()
}

fn lookup(fixture: &Fixture) -> Result<Option<EntityAlias>> {
    fixture.repo.resolve_alias(
        &fixture.conn,
        &fixture.port,
        &fixture.run.run_id,
        EntityCategory::Character,
        SCOPE,
        ALIAS_NAME,
    )
}

fn revocation(fixture: &Fixture, alias: &EntityAlias) -> AliasRevocation {
    AliasRevocation {
        alias_id: alias.alias_id.clone(),
        expected_revision: alias.revision,
        expected_entity_revision: fixture.port.view.entity_revision,
    }
}

#[test]
fn revocation_rejects_invalid_unknown_stale_and_exhausted_versions() {
    let mut fixture = Fixture::new();
    let alias = confirm_alias(&mut fixture, 30, ENTITY_ID);
    let valid = revocation(&fixture, &alias);
    for invalid in [
        AliasRevocation {
            alias_id: "invalid".into(),
            ..valid.clone()
        },
        AliasRevocation {
            expected_revision: 0,
            ..valid.clone()
        },
        AliasRevocation {
            expected_revision: MAX_INTEGER + 1,
            ..valid.clone()
        },
        AliasRevocation {
            expected_entity_revision: MAX_INTEGER + 1,
            ..valid.clone()
        },
    ] {
        assert!(matches!(
            fixture.repo.revoke_alias(
                &mut fixture.conn,
                &fixture.port,
                &fixture.run.run_id,
                invalid
            ),
            Err(Error::Rejected(Reason::InvalidSchema))
        ));
    }
    for stale in [
        AliasRevocation {
            expected_revision: 2,
            ..valid.clone()
        },
        AliasRevocation {
            expected_entity_revision: valid.expected_entity_revision + 1,
            ..valid.clone()
        },
    ] {
        assert!(matches!(
            fixture
                .repo
                .revoke_alias(&mut fixture.conn, &fixture.port, &fixture.run.run_id, stale),
            Err(Error::Rejected(Reason::StaleVersion))
        ));
    }
    assert!(matches!(
        fixture.repo.revoke_alias(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            AliasRevocation {
                alias_id: new_id(),
                ..valid.clone()
            }
        ),
        Err(Error::NotFound)
    ));
    fixture
        .repo
        .revoke_alias(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            valid.clone(),
        )
        .unwrap();
    assert!(matches!(
        fixture.repo.revoke_alias(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            AliasRevocation {
                expected_revision: 4,
                ..valid.clone()
            }
        ),
        Err(Error::Rejected(Reason::StaleVersion))
    ));
    fixture
        .conn
        .execute(
            "UPDATE memory_aliases SET status='active',revision=?1",
            [MAX_INTEGER],
        )
        .unwrap();
    assert!(matches!(
        fixture.repo.revoke_alias(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            AliasRevocation {
                expected_revision: MAX_INTEGER,
                ..valid
            }
        ),
        Err(Error::Rejected(Reason::CapacityBlocked))
    ));
}

#[test]
fn revocation_cas_does_not_acknowledge_an_ignored_storage_update() {
    let mut fixture = Fixture::new();
    let alias = confirm_alias(&mut fixture, 31, ENTITY_ID);
    fixture.conn.execute_batch("CREATE TRIGGER ignore_alias_update BEFORE UPDATE ON memory_aliases BEGIN SELECT RAISE(IGNORE); END").unwrap();
    let revoke = revocation(&fixture, &alias);
    assert!(matches!(
        fixture.repo.revoke_alias(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            revoke
        ),
        Err(Error::Rejected(Reason::StaleVersion))
    ));
    assert_eq!(
        lookup(&fixture).unwrap().unwrap().status,
        AliasStatus::Active
    );
}

#[test]
fn confirmations_require_valid_context_and_name_containing_current_source() {
    let mut fixture = Fixture::new();
    let question = opened_question(&mut fixture, 32);
    let context = entity_context(ENTITY_ID, "守夜人");
    for invalid in [
        EntityAliasContext {
            entity_id: OTHER_ENTITY_ID.into(),
            ..context.clone()
        },
        EntityAliasContext {
            scope: " ".into(),
            ..context.clone()
        },
        EntityAliasContext {
            canonical_name: " ".into(),
            ..context.clone()
        },
    ] {
        fixture
            .port
            .alias_entities
            .borrow_mut()
            .insert(ENTITY_ID.into(), invalid);
        assert!(matches!(
            apply_question(&mut fixture, &question, ENTITY_ID),
            Err(Error::Rejected(Reason::InvalidSchema))
        ));
    }
    fixture
        .port
        .alias_entities
        .borrow_mut()
        .insert(ENTITY_ID.into(), context);
    fixture.port.invalid.set(true);
    assert!(matches!(
        apply_question(&mut fixture, &question, ENTITY_ID),
        Err(Error::Rejected(Reason::InvalidSource))
    ));
    fixture.port.invalid.set(false);
    let mut not_mentioned = question.clone();
    not_mentioned.draft.mentioned_name = Some("未提及".into());
    assert!(matches!(
        apply_question(&mut fixture, &not_mentioned, ENTITY_ID),
        Err(Error::Rejected(Reason::InvalidSource))
    ));
    let mut no_name = question;
    no_name.draft.mentioned_name = None;
    apply_question(&mut fixture, &no_name, ENTITY_ID).unwrap();
    assert!(lookup(&fixture).unwrap().is_none());
}

#[test]
fn duplicate_alias_application_is_idempotent_without_extra_audit_event() {
    let mut fixture = Fixture::new();
    let question = opened_question(&mut fixture, 33);
    fixture
        .port
        .alias_entities
        .borrow_mut()
        .insert(ENTITY_ID.into(), entity_context(ENTITY_ID, "守夜人"));
    apply_question(&mut fixture, &question, ENTITY_ID).unwrap();
    let before = lookup(&fixture).unwrap().unwrap();
    apply_question(&mut fixture, &question, ENTITY_ID).unwrap();
    assert_eq!(lookup(&fixture).unwrap().unwrap(), before);
    assert_eq!(
        fixture
            .conn
            .query_row("SELECT count(*) FROM memory_alias_events", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn alias_resolution_requires_current_directory_and_registered_revision() {
    let mut fixture = Fixture::new();
    let alias = confirm_alias(&mut fixture, 34, ENTITY_ID);
    fixture.port.view.entity_revision = alias.registered_entity_revision - 1;
    assert!(matches!(
        lookup(&fixture),
        Err(Error::Rejected(Reason::StaleVersion))
    ));
    fixture.port.view.entity_revision = alias.registered_entity_revision;
    fixture.port.alias_entities.borrow_mut().clear();
    assert!(lookup(&fixture).unwrap().is_none());
    fixture
        .port
        .alias_entities
        .borrow_mut()
        .insert(ENTITY_ID.into(), entity_context(OTHER_ENTITY_ID, "访客"));
    assert!(lookup(&fixture).unwrap().is_none());
    fixture
        .port
        .alias_entities
        .borrow_mut()
        .insert(ENTITY_ID.into(), entity_context(ENTITY_ID, "守夜人"));
    fixture.port.canonical_names.borrow_mut().insert(
        (EntityCategory::Character, SCOPE.into(), ALIAS_NAME.into()),
        OTHER_ENTITY_ID.into(),
    );
    assert!(lookup(&fixture).unwrap().is_none());
    assert!(matches!(
        fixture.repo.resolve_alias(
            &fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            EntityCategory::Character,
            "",
            ALIAS_NAME
        ),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
}

#[test]
fn persisted_alias_rows_reject_unknown_tags_and_invalid_identity() {
    let valid: AliasRow = (
        new_id(),
        new_id(),
        ENTITY_ID.into(),
        "character".into(),
        SCOPE.into(),
        ALIAS_NAME.into(),
        "active".into(),
        1,
        0,
    );
    assert!(decode_alias(valid.clone()).is_ok());
    for index in 0..9 {
        let mut row = valid.clone();
        match index {
            0 => row.0 = "invalid".into(),
            1 => row.1 = "invalid".into(),
            2 => row.2 = " ".into(),
            3 => row.3 = "future".into(),
            4 => row.4 = " ".into(),
            5 => row.5 = " ".into(),
            6 => row.6 = "future".into(),
            7 => row.7 = 0,
            _ => row.8 = MAX_INTEGER + 1,
        }
        assert!(matches!(decode_alias(row), Err(Error::Corrupt)));
    }
}

#[test]
fn alias_source_capacity_is_bounded_and_corrupt_sources_fail_closed() {
    // 此用例验证来源数量策略与损坏拒绝，不验证 SQLite 耐久性。
    let mut fixture = Fixture::new_in_memory();
    let alias = confirm_alias(&mut fixture, 35, ENTITY_ID);
    let question = opened_question(&mut fixture, 36);
    // 不同问题可贡献同一来源；满额必须拒绝整笔新增确认。
    for index in 1..MAX_ALIAS_SOURCES {
        let support = opened_question(&mut fixture, 100 + index as u32);
        fixture.conn.execute("INSERT INTO memory_alias_sources(alias_id,question_id,source_hash,source_json) SELECT alias_id,?1,source_hash,source_json FROM memory_alias_sources WHERE alias_id=?2 LIMIT 1", params![support.question_id, alias.alias_id]).unwrap();
    }
    assert!(matches!(
        apply_question(&mut fixture, &question, ENTITY_ID),
        Err(Error::Rejected(Reason::CapacityBlocked))
    ));
    let excess = opened_question(&mut fixture, 300);
    fixture.conn.execute("INSERT INTO memory_alias_sources(alias_id,question_id,source_hash,source_json) SELECT alias_id,?1,source_hash,source_json FROM memory_alias_sources WHERE alias_id=?2 LIMIT 1", params![excess.question_id, alias.alias_id]).unwrap();
    assert!(matches!(lookup(&fixture), Err(Error::Corrupt)));
    assert!(matches!(
        apply_question(&mut fixture, &question, ENTITY_ID),
        Err(Error::Corrupt)
    ));
}

#[test]
fn alias_count_and_revision_capacity_reject_without_partial_submission() {
    let mut fixture = Fixture::new();
    let alias = confirm_alias(&mut fixture, 37, ENTITY_ID);
    let question = opened_question(&mut fixture, 38);
    fixture
        .conn
        .execute("UPDATE memory_aliases SET revision=?1", [MAX_INTEGER])
        .unwrap();
    assert!(matches!(
        apply_question(&mut fixture, &question, ENTITY_ID),
        Err(Error::Rejected(Reason::CapacityBlocked))
    ));
    fixture
        .conn
        .execute("UPDATE memory_aliases SET status='revoked'", [])
        .unwrap();
    fixture.port.alias_entities.borrow_mut().insert(
        OTHER_ENTITY_ID.into(),
        entity_context(OTHER_ENTITY_ID, "访客"),
    );
    fixture
        .conn
        .execute("UPDATE memory_aliases SET status='active'", [])
        .unwrap();
    fixture
        .port
        .invalid_records
        .borrow_mut()
        .insert(fixture.source().record_seq);
    let mut new_source_question = question.clone();
    new_source_question.draft.identity.sources[0].record_seq = 3;
    assert!(matches!(
        apply_question(&mut fixture, &new_source_question, OTHER_ENTITY_ID),
        Err(Error::Rejected(Reason::CapacityBlocked))
    ));
    fixture.port.invalid_records.borrow_mut().clear();
    fixture
        .conn
        .execute("UPDATE memory_aliases SET revision=1", [])
        .unwrap();
    fixture.conn.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<4095) INSERT INTO memory_aliases SELECT printf('%08x-0000-4000-8000-000000000000',i),run_id,entity_id,category,scope,printf('extra-%d',i),'revoked',1,registered_entity_revision FROM memory_aliases,n WHERE alias_id=?1", [&alias.alias_id]).unwrap();
    assert!(matches!(
        apply_question(&mut fixture, &question, OTHER_ENTITY_ID),
        Err(Error::Rejected(Reason::AmbiguousEntity))
    ));
    let tx = fixture.conn.transaction().unwrap();
    tx.execute(
        "UPDATE memory_aliases SET status='revoked' WHERE alias_id=?1",
        [&alias.alias_id],
    )
    .unwrap();
    let result = apply_confirmed_alias(
        &tx,
        &fixture.run.run_id,
        &fixture.run,
        &question,
        OTHER_ENTITY_ID,
        &fixture.port.view,
        &fixture.port,
    );
    assert!(matches!(
        result,
        Err(Error::Rejected(Reason::CapacityBlocked))
    ));
}

#[test]
fn stale_alias_reassignment_checks_revision_and_requires_successful_revocation() {
    let mut fixture = Fixture::new();
    let alias = confirm_alias(&mut fixture, 39, ENTITY_ID);
    fixture.port.alias_entities.borrow_mut().insert(
        OTHER_ENTITY_ID.into(),
        entity_context(OTHER_ENTITY_ID, "访客"),
    );
    let question = opened_question(&mut fixture, 40);
    fixture.port.view.entity_revision = alias.registered_entity_revision - 1;
    assert!(matches!(
        apply_question(&mut fixture, &question, OTHER_ENTITY_ID),
        Err(Error::Rejected(Reason::StaleVersion))
    ));
    fixture.port.view.entity_revision = alias.registered_entity_revision;
    let tx = fixture.conn.transaction().unwrap();
    tx.execute_batch("CREATE TRIGGER ignore_alias_update BEFORE UPDATE ON memory_aliases BEGIN SELECT RAISE(IGNORE); END").unwrap();
    assert!(matches!(
        revoke_stale_alias(&tx, &alias, alias.registered_entity_revision),
        Err(Error::Rejected(Reason::StaleVersion))
    ));
}

#[test]
fn audit_write_failure_rolls_back_alias_registration_and_revocation() {
    let mut fixture = Fixture::new();
    let question = opened_question(&mut fixture, 41);
    fixture
        .port
        .alias_entities
        .borrow_mut()
        .insert(ENTITY_ID.into(), entity_context(ENTITY_ID, "守夜人"));
    fixture.conn.execute_batch("CREATE TRIGGER fail_alias_event BEFORE INSERT ON memory_alias_events BEGIN SELECT RAISE(ABORT,'audit unavailable'); END").unwrap();
    assert!(matches!(
        apply_question(&mut fixture, &question, ENTITY_ID),
        Err(Error::Store(_))
    ));
    assert!(lookup(&fixture).unwrap().is_none());
    fixture
        .conn
        .execute_batch("DROP TRIGGER fail_alias_event")
        .unwrap();
    let alias = confirm_alias(&mut fixture, 42, ENTITY_ID);
    let revoke = revocation(&fixture, &alias);
    fixture.conn.execute_batch("CREATE TRIGGER fail_alias_event BEFORE INSERT ON memory_alias_events BEGIN SELECT RAISE(ABORT,'audit unavailable'); END").unwrap();
    assert!(matches!(
        fixture.repo.revoke_alias(
            &mut fixture.conn,
            &fixture.port,
            &fixture.run.run_id,
            revoke
        ),
        Err(Error::Store(_))
    ));
    assert_eq!(lookup(&fixture).unwrap().unwrap(), alias);
}

#[test]
fn reconfirming_a_corrupt_nonactive_alias_does_not_repair_or_overwrite_it() {
    let mut fixture = Fixture::new();
    let alias = confirm_alias(&mut fixture, 43, ENTITY_ID);
    let question = opened_question(&mut fixture, 44);
    fixture
        .conn
        .execute_batch("PRAGMA ignore_check_constraints=ON")
        .unwrap();
    fixture
        .conn
        .execute(
            "UPDATE memory_aliases SET status='future' WHERE alias_id=?1",
            [&alias.alias_id],
        )
        .unwrap();
    assert!(matches!(
        apply_question(&mut fixture, &question, ENTITY_ID),
        Err(Error::Corrupt)
    ));
    assert_eq!(
        fixture
            .conn
            .query_row(
                "SELECT status FROM memory_aliases WHERE alias_id=?1",
                [&alias.alias_id],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "future"
    );
}
