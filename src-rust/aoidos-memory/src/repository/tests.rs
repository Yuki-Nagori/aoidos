//! 仓储读取与周目冻结契约。

use super::test_support::*;
use super::*;

#[test]
fn run_reopen_freezes_policy_and_rejects_changed_identity_or_corruption() {
    let mut f = Fixture::new();
    let mut changed = policy();
    changed.config_version = 2;
    assert_eq!(
        f.repo.create_run(&mut f.conn, &f.port, changed).unwrap(),
        f.run
    );
    let original = f.port.view.script_id.clone();
    f.port.view.script_id = "elsewhere".into();
    assert!(matches!(
        f.repo.create_run(&mut f.conn, &f.port, policy()),
        Err(Error::Corrupt)
    ));
    f.port.view.script_id = original;
    f.conn
        .execute("UPDATE memory_runs SET policy_hash=zeroblob(32)", [])
        .unwrap();
    assert!(matches!(
        f.repo.run(&f.conn, &f.run.run_id),
        Err(Error::Corrupt)
    ));
    assert!(matches!(
        f.repo.run(&f.conn, &new_id()),
        Err(Error::NotFound)
    ));
    rejected(f.repo.run(&f.conn, "../escape"), Reason::InvalidSchema);
}

#[test]
fn cycle_generation_advances_per_script_only_for_a_new_session() {
    let mut f = Fixture::new();
    assert_eq!(f.run.cycle_gen, 1);
    assert_eq!(
        f.repo.create_run(&mut f.conn, &f.port, policy()).unwrap(),
        f.run,
        "同一会话恢复不得增加周目代数"
    );

    let original_session = f.port.view.session_id.clone();
    f.port.view.session_id = new_id();
    let second = f.repo.create_run(&mut f.conn, &f.port, policy()).unwrap();
    assert_eq!(second.cycle_gen, 2);
    assert_eq!(f.repo.run(&f.conn, &second.run_id).unwrap(), second);

    f.port.view.session_id = new_id();
    f.port.view.script_id = "other-script".into();
    let other_script = f.repo.create_run(&mut f.conn, &f.port, policy()).unwrap();
    assert_eq!(other_script.cycle_gen, 1, "不同剧本使用独立代数");

    f.port.view.script_id = f.run.script_id.clone();
    f.port.view.session_id = original_session;
    assert_eq!(
        f.repo.create_run(&mut f.conn, &f.port, policy()).unwrap(),
        f.run
    );
    f.port.view.session_id = new_id();
    let third = f.repo.create_run(&mut f.conn, &f.port, policy()).unwrap();
    assert_eq!(third.cycle_gen, 3, "恢复旧会话后新周目仍单调递增");
}

#[test]
fn cycle_generation_counter_corruption_and_exhaustion_fail_closed() {
    let mut f = Fixture::new();
    f.port.view.session_id = new_id();
    let second = f.repo.create_run(&mut f.conn, &f.port, policy()).unwrap();
    assert_eq!(second.cycle_gen, 2);
    f.conn
        .execute(
            "UPDATE memory_script_cycle_generations SET cycle_gen=1 WHERE script_id=?1",
            [&f.run.script_id],
        )
        .unwrap();
    f.port.view.session_id = new_id();
    assert!(matches!(
        f.repo.create_run(&mut f.conn, &f.port, policy()),
        Err(Error::Corrupt)
    ));

    f.conn
        .execute(
            "UPDATE memory_script_cycle_generations SET cycle_gen=?1 WHERE script_id=?2",
            params![MAX_INTEGER, f.run.script_id],
        )
        .unwrap();
    rejected(
        f.repo.create_run(&mut f.conn, &f.port, policy()),
        Reason::CapacityBlocked,
    );
}

#[test]
fn cycle_generation_migration_backfills_existing_runs_without_advancing_twice() {
    let mut f = Fixture::new();
    f.conn
        .execute_batch(
            "DROP TABLE memory_script_cycle_generations; DROP TABLE memory_run_cycle_generations;",
        )
        .unwrap();
    f.conn
        .execute_batch(crate::schema::CYCLE_GENERATION_MIGRATION)
        .unwrap();
    f.conn
        .execute_batch(crate::schema::CYCLE_GENERATION_BACKFILL)
        .unwrap();
    f.conn
        .execute_batch(crate::schema::CYCLE_GENERATION_BACKFILL)
        .unwrap();

    assert_eq!(f.repo.run(&f.conn, &f.run.run_id).unwrap(), f.run);
    f.port.view.session_id = new_id();
    let next = f.repo.create_run(&mut f.conn, &f.port, policy()).unwrap();
    assert_eq!(next.cycle_gen, 2);
}

#[test]
fn run_without_persisted_cycle_generation_is_corrupt() {
    let f = Fixture::new();
    f.conn
        .execute(
            "DELETE FROM memory_run_cycle_generations WHERE run_id=?1",
            [&f.run.run_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.run(&f.conn, &f.run.run_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn pagination_is_bounded_and_returns_each_committed_entry_once() {
    let mut f = Fixture::new();
    let mut ids = BTreeSet::new();
    for _ in 0..3 {
        let v = f.version();
        ids.insert(v.entry_id.clone());
        f.commit(&f.plan(v));
    }
    let first = f
        .repo
        .entries(&f.conn, &f.port, &f.run.run_id, 0, None, 2)
        .unwrap();
    assert_eq!(first.len(), 2);
    let second = f
        .repo
        .entries(
            &f.conn,
            &f.port,
            &f.run.run_id,
            0,
            Some(&first[1].entry_id),
            2,
        )
        .unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(
        first
            .into_iter()
            .chain(second)
            .map(|entry| entry.entry_id)
            .collect::<BTreeSet<_>>(),
        ids
    );
}

#[test]
fn page_cursor_is_omitted_when_the_last_page_is_full() {
    let mut f = Fixture::new();
    for _ in 0..2 {
        let version = f.version();
        f.commit(&f.plan(version));
    }
    let (entries, snapshot_max_sequence, next) = f
        .repo
        .entries_page(&f.conn, &f.port, &f.run.run_id, 0, None, 2)
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert!(snapshot_max_sequence > 0);
    assert_eq!(next, None);
}

#[test]
fn entries_for_subjects_returns_active_and_archived_targets_and_checks_revision() {
    let mut f = Fixture::new();
    let character_id = "bellkeeper";
    let mut character = f.version();
    character.subject = Subject::Character {
        id: character_id.into(),
    };
    character.dimension = Dimension::Story;
    character.evidence.push(KnowledgeEvidence {
        evidence_id: new_id(),
        subject_id: character_id.into(),
        source: f.source(),
        kind: KnowledgeKind::Observed,
        informant_id: None,
        scope: "public".into(),
        rule_id: "observation".into(),
    });
    character.evidence_set_hash = set_hash(&character.evidence).unwrap();
    let character_entry = character.entry_id.clone();
    f.commit(&f.plan(character));

    let preference = f.version();
    let preference_entry = preference.entry_id.clone();
    f.commit(&f.plan(preference));

    let archived = f.version();
    let archived_entry = archived.entry_id.clone();
    let mut archived_plan = f.plan(archived);
    archived_plan.targets[0].state = EntryState::Archived;
    f.commit(&archived_plan);

    let subjects = [
        Subject::Character {
            id: character_id.into(),
        },
        Subject::PlayerPreference,
    ];
    let scan = f
        .repo
        .entries_for_subjects(&f.conn, &f.port, &f.run.run_id, 0, &subjects, 65)
        .unwrap();
    assert!(scan.scanned_entries >= scan.entries.len());
    assert_eq!(
        scan.entries
            .iter()
            .map(|entry| entry.entry_id.as_str())
            .collect::<BTreeSet<_>>(),
        [
            character_entry.as_str(),
            preference_entry.as_str(),
            archived_entry.as_str()
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(
        scan.entries
            .iter()
            .find(|entry| entry.entry_id == archived_entry)
            .unwrap()
            .state,
        EntryState::Archived
    );

    rejected(
        f.repo.entries_for_subjects(
            &f.conn,
            &f.port,
            &f.run.run_id,
            0,
            &subjects,
            MAX_PROPOSAL_TARGET_SCAN + 2,
        ),
        Reason::InvalidSchema,
    );
    f.port.view.history_revision = 1;
    rejected(
        f.repo
            .entries_for_subjects(&f.conn, &f.port, &f.run.run_id, 0, &subjects, 65),
        Reason::StaleVersion,
    );
}

#[test]
fn entries_for_subjects_returns_more_than_the_old_64_target_limit() {
    let mut f = Fixture::new_in_memory();
    for _ in 0..65 {
        let version = f.version();
        let plan = f.plan(version);
        f.commit(&plan);
    }

    let scan = f
        .repo
        .entries_for_subjects(
            &f.conn,
            &f.port,
            &f.run.run_id,
            0,
            &[Subject::PlayerPreference],
            MAX_PROPOSAL_TARGET_SCAN + 1,
        )
        .unwrap();

    assert_eq!(scan.entries.len(), 65);
    assert_eq!(scan.scanned_entries, 65);
}

#[test]
fn entries_for_subjects_accepts_exact_scan_budget_and_rejects_the_next_entry() {
    let mut f = Fixture::new_in_memory();
    let run_id = f.run.run_id.clone();
    let tx = f.conn.transaction().unwrap();
    {
        let mut insert = tx
            .prepare("INSERT INTO memory_entries(entry_id,run_id,current_version_id,state_revision,status) VALUES(?1,?2,NULL,0,'active')")
            .unwrap();
        for _ in 0..MAX_PROPOSAL_TARGET_SCAN {
            insert.execute(params![new_id(), run_id]).unwrap();
        }
    }
    tx.commit().unwrap();

    let scan = f
        .repo
        .entries_for_subjects(
            &f.conn,
            &f.port,
            &f.run.run_id,
            0,
            &[Subject::PlayerPreference],
            MAX_PROPOSAL_TARGETS + 1,
        )
        .unwrap();
    assert!(scan.entries.is_empty());
    assert_eq!(scan.scanned_entries, MAX_PROPOSAL_TARGET_SCAN);

    f.conn
        .execute(
            "INSERT INTO memory_entries(entry_id,run_id,current_version_id,state_revision,status) VALUES(?1,?2,NULL,0,'active')",
            params![new_id(), f.run.run_id],
        )
        .unwrap();

    rejected(
        f.repo.entries_for_subjects(
            &f.conn,
            &f.port,
            &f.run.run_id,
            0,
            &[Subject::PlayerPreference],
            MAX_PROPOSAL_TARGETS + 1,
        ),
        Reason::CapacityBlocked,
    );
}

#[test]
fn entries_for_subjects_keeps_a_matching_target_after_unrelated_entries() {
    let mut f = Fixture::new_in_memory();
    let target = f.version();
    let target_id = target.entry_id.clone();
    f.commit(&f.plan(target));

    let tx = f.conn.transaction().unwrap();
    {
        let mut insert = tx
            .prepare("INSERT INTO memory_entries(entry_id,run_id,current_version_id,state_revision,status) VALUES(?1,?2,NULL,0,'active')")
            .unwrap();
        for index in 0..1_000 {
            let entry_id = format!("00000000-0000-7000-8000-{index:012x}");
            insert.execute(params![entry_id, f.run.run_id]).unwrap();
        }
    }
    tx.commit().unwrap();

    let scan = f
        .repo
        .entries_for_subjects(
            &f.conn,
            &f.port,
            &f.run.run_id,
            0,
            &[Subject::PlayerPreference],
            MAX_PROPOSAL_TARGET_SCAN + 1,
        )
        .unwrap();

    assert_eq!(scan.scanned_entries, 1_001);
    assert_eq!(scan.entries.len(), 1);
    assert_eq!(scan.entries[0].entry_id, target_id);
}

#[test]
fn entries_for_subjects_accepts_exact_byte_budget_and_rejects_overflow() {
    let mut f = Fixture::new_in_memory();
    let subject = canonical(&Subject::PlayerPreference).unwrap();
    let tx = f.conn.transaction().unwrap();
    for _ in 0..128 {
        let entry_id = new_id();
        let version_id = new_id();
        tx.execute(
            "INSERT INTO memory_entries(entry_id,run_id,current_version_id,state_revision,status) VALUES(?1,?2,NULL,0,'active')",
            params![entry_id, f.run.run_id],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO memory_versions(version_id,entry_id,body_hash,body_length,source_set_hash,evidence_set_hash,policy_hash,subject_json,change_kind) \
             VALUES(?1,?2,zeroblob(32),32768,zeroblob(32),zeroblob(32),zeroblob(32),?3,'new')",
            params![version_id, entry_id, subject],
        )
        .unwrap();
        tx.execute(
            "UPDATE memory_entries SET current_version_id=?1 WHERE entry_id=?2",
            params![version_id, entry_id],
        )
        .unwrap();
    }
    tx.commit().unwrap();

    let scan = f
        .repo
        .entries_for_subjects(
            &f.conn,
            &f.port,
            &f.run.run_id,
            0,
            &[Subject::PlayerPreference],
            MAX_PROPOSAL_TARGET_SCAN + 1,
        )
        .unwrap();
    assert_eq!(scan.entries.len(), 128);
    assert_eq!(scan.scanned_entries, 128);

    let entry_id = new_id();
    let version_id = new_id();
    let tx = f.conn.transaction().unwrap();
    tx.execute(
        "INSERT INTO memory_entries(entry_id,run_id,current_version_id,state_revision,status) VALUES(?1,?2,NULL,0,'active')",
        params![entry_id, f.run.run_id],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO memory_versions(version_id,entry_id,body_hash,body_length,source_set_hash,evidence_set_hash,policy_hash,subject_json,change_kind) \
         VALUES(?1,?2,zeroblob(32),32768,zeroblob(32),zeroblob(32),zeroblob(32),?3,'new')",
        params![version_id, entry_id, subject],
    )
    .unwrap();
    tx.execute(
        "UPDATE memory_entries SET current_version_id=?1 WHERE entry_id=?2",
        params![version_id, entry_id],
    )
    .unwrap();
    tx.commit().unwrap();

    rejected(
        f.repo.entries_for_subjects(
            &f.conn,
            &f.port,
            &f.run.run_id,
            0,
            &[Subject::PlayerPreference],
            MAX_PROPOSAL_TARGET_SCAN + 1,
        ),
        Reason::CapacityBlocked,
    );
}

#[test]
fn entries_for_subjects_budget_ignores_history_from_other_runs() {
    let mut f = Fixture::new_in_memory();
    let version = f.version();
    let entry_id = version.entry_id.clone();
    f.commit(&f.plan(version));
    let subject_json = canonical(&Subject::PlayerPreference).unwrap();
    {
        let tx = f.conn.transaction().unwrap();
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO memory_versions(version_id,entry_id,body_hash,body_length,source_set_hash,evidence_set_hash,policy_hash,subject_json,change_kind) \
                     VALUES(?1,?2,zeroblob(32),1,zeroblob(32),zeroblob(32),zeroblob(32),?3,'new')",
                )
                .unwrap();
            for _ in 0..=MAX_PROPOSAL_TARGET_SCAN {
                insert
                    .execute(params![new_id(), entry_id, subject_json])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
    }

    f.port.view.session_id = new_id();
    let other_run = f.repo.create_run(&mut f.conn, &f.port, policy()).unwrap();
    assert!(
        f.repo
            .entries_for_subjects(
                &f.conn,
                &f.port,
                &other_run.run_id,
                0,
                &[Subject::PlayerPreference],
                MAX_PROPOSAL_TARGETS + 1,
            )
            .unwrap()
            .entries
            .is_empty()
    );
}

#[test]
fn source_and_evidence_checks_apply_to_character_versions() {
    let mut f = Fixture::new();
    let mut version = f.version();
    version.subject = Subject::Character {
        id: "bellkeeper".into(),
    };
    version.dimension = Dimension::Story;
    version.evidence.push(KnowledgeEvidence {
        evidence_id: new_id(),
        subject_id: "bellkeeper".into(),
        source: f.source(),
        kind: KnowledgeKind::Observed,
        informant_id: None,
        scope: "public".into(),
        rule_id: "observation".into(),
    });
    version.evidence_set_hash = set_hash(&version.evidence).unwrap();
    f.commit(&f.plan(version.clone()));
    assert_eq!(
        f.repo
            .current(&f.conn, &f.port, &f.run.run_id, &version.entry_id)
            .unwrap(),
        version
    );
    assert_eq!(
        f.repo
            .version(
                &f.conn,
                &f.port,
                &f.run.run_id,
                &version.entry_id,
                &version.version_id,
            )
            .unwrap(),
        version
    );
    assert!(matches!(
        f.repo.version(
            &f.conn,
            &f.port,
            &f.run.run_id,
            &version.entry_id,
            &new_id(),
        ),
        Err(Error::NotFound)
    ));
    rejected(
        f.repo.version(
            &f.conn,
            &f.port,
            &f.run.run_id,
            &version.entry_id,
            "not-a-uuid",
        ),
        Reason::InvalidSchema,
    );
    f.conn
        .execute("DELETE FROM memory_version_evidence", [])
        .unwrap();
    assert!(matches!(
        f.repo
            .current(&f.conn, &f.port, &f.run.run_id, &version.entry_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn unknown_frozen_policy_remains_unavailable_instead_of_using_defaults() {
    let f = Fixture::new();
    let mut unknown = policy();
    unknown.version = 2;
    let raw = canonical(&unknown).unwrap();
    f.conn
        .execute(
            "UPDATE memory_runs SET policy_json=?1,policy_hash=?2",
            params![raw, hash(&raw).as_slice()],
        )
        .unwrap();
    rejected(
        f.repo.run(&f.conn, &f.run.run_id),
        Reason::PolicyUnavailable,
    );
}

#[test]
fn corrupt_untrusted_rows_keep_store_errors_separate_from_invalid_schema() {
    let f = Fixture::new();
    rejected(
        f.repo.entry(&f.conn, "bad", &new_id()),
        Reason::InvalidSchema,
    );
    assert!(matches!(
        f.repo.material_source(&f.conn, &f.run.run_id, &new_id()),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        f.repo.version_path(&f.run, "../entry", &new_id()),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
    assert!(matches!(as_hash(vec![1]), Err(Error::Corrupt)));
    assert!(matches!(
        decode_enum::<EntryState>("unknown"),
        Err(Error::Corrupt)
    ));
    assert!(matches!(enum_text(42), Err(Error::Corrupt)));
    f.conn
        .execute_batch("DROP TABLE memory_run_sessions")
        .unwrap();
    assert!(matches!(
        f.repo.run(&f.conn, &f.run.run_id),
        Err(Error::Store(_))
    ));
}

#[test]
fn oversized_index_metadata_fails_before_body_consumption() {
    let mut f = Fixture::new();
    let v = f.version();
    f.commit(&f.plan(v.clone()));
    f.conn
        .execute(
            "UPDATE memory_version_sources SET source_json=?1",
            [vec![b'x'; MAX_SCAN_BYTES + 1]],
        )
        .unwrap();
    assert!(matches!(
        f.repo.current(&f.conn, &f.port, &f.run.run_id, &v.entry_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn oversized_persisted_policy_is_rejected_before_blob_copy() {
    let f = Fixture::new();
    f.conn
        .execute(
            "UPDATE memory_runs SET policy_json=zeroblob(?1) WHERE run_id=?2",
            params![32 * 1024 + 1, f.run.run_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.run(&f.conn, &f.run.run_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn unknown_body_version_is_not_restored_with_current_defaults() {
    let mut f = Fixture::new();
    let v = f.version();
    let mut future_plan = f.plan(v.clone());
    f.commit(&future_plan);
    let mut future = v.clone();
    future.version = 2;
    let bytes = canonical(&future).unwrap();
    std::fs::write(f.path(&v), &bytes).unwrap();
    f.conn
        .execute(
            "UPDATE memory_versions SET body_hash=?1,body_length=?2",
            params![hash(&bytes).as_slice(), bytes.len()],
        )
        .unwrap();
    // 模拟新版程序曾合法发布 v2，随后旧版程序重新打开；审计关系保持一致。
    future_plan.targets[0].next = Some(future);
    let plan_bytes = canonical(&future_plan).unwrap();
    let digest = hash(&plan_bytes);
    let receipt = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    f.conn
        .execute(
            "UPDATE memory_operations SET plan_json=?1,plan_hash=?2 WHERE operation_id=?3",
            params![plan_bytes, digest.as_slice(), future_plan.operation_id],
        )
        .unwrap();
    f.conn
        .execute(
            "UPDATE store_applied SET content_hash=?1 WHERE id=?2",
            params![receipt, format!("memory:{}", future_plan.operation_id)],
        )
        .unwrap();
    let changes = f.conn.total_changes();
    rejected(
        f.repo.current(&f.conn, &f.port, &f.run.run_id, &v.entry_id),
        Reason::PolicyUnavailable,
    );
    rejected(
        f.repo.recover(&mut f.conn, &f.port, &f.run.run_id, None),
        Reason::PolicyUnavailable,
    );
    assert_eq!(
        f.repo
            .entry(&f.conn, &f.run.run_id, &v.entry_id)
            .unwrap()
            .state,
        EntryState::Active
    );
    assert_eq!(f.conn.total_changes(), changes);
}

#[test]
fn unregistered_boundary_cannot_create_another_run() {
    let mut f = Fixture::new();
    for field in ["script", "session", "history"] {
        let saved = f.port.view.clone();
        match field {
            "script" => f.port.view.script_id = "../outside".into(),
            "session" => f.port.view.session_id = "not-a-session".into(),
            _ => f.port.view.history_revision = MAX_INTEGER + 1,
        }
        rejected(
            f.repo.create_run(&mut f.conn, &f.port, policy()),
            Reason::InvalidSchema,
        );
        f.port.view = saved;
    }
    let count: u64 = f
        .conn
        .query_row("SELECT count(*) FROM memory_runs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn revoked_character_evidence_does_not_reuse_still_valid_public_source() {
    let mut f = Fixture::new();
    let mut version = f.version();
    version.subject = Subject::Character {
        id: "bellkeeper".into(),
    };
    version.dimension = Dimension::Story;
    version.evidence.push(KnowledgeEvidence {
        evidence_id: new_id(),
        subject_id: "bellkeeper".into(),
        source: f.source(),
        kind: KnowledgeKind::Observed,
        informant_id: None,
        scope: "public".into(),
        rule_id: "observation".into(),
    });
    version.evidence_set_hash = set_hash(&version.evidence).unwrap();
    f.commit(&f.plan(version.clone()));
    f.port.evidence_invalid.set(true);
    assert!(f.port.valid_source(&f.source()).unwrap());
    rejected(
        f.repo
            .current(&f.conn, &f.port, &f.run.run_id, &version.entry_id),
        Reason::InvalidSource,
    );
}

#[test]
fn body_io_failure_is_not_mistaken_for_missing_content_or_recovery_eligibility() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    let path = f.path(&version);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(matches!(
        f.repo
            .current(&f.conn, &f.port, &f.run.run_id, &version.entry_id),
        Err(Error::Store(_))
    ));
    assert_eq!(
        f.repo
            .entry(&f.conn, &f.run.run_id, &version.entry_id)
            .unwrap()
            .state,
        EntryState::Active
    );
    let mut malformed_run = f.run.clone();
    malformed_run.script_id = "../outside".into();
    assert!(matches!(
        f.repo
            .version_path(&malformed_run, &version.entry_id, &version.version_id),
        Err(Error::Store(_))
    ));
}

#[test]
fn source_index_preserves_body_order_even_when_set_hash_is_unchanged() {
    let mut f = Fixture::new();
    let mut version = f.version();
    let mut second = f.source();
    second.record_seq = 2;
    version.sources.push(second);
    version.source_set_hash = set_hash(&version.sources).unwrap();
    f.commit(&f.plan(version.clone()));
    for (ordinal, source) in version.sources.iter().rev().enumerate() {
        f.conn.execute("UPDATE memory_version_sources SET source_json=?1 WHERE version_id=?2 AND ordinal=?3", params![canonical(source).unwrap(), version.version_id, ordinal]).unwrap();
    }
    assert!(matches!(
        f.repo
            .current(&f.conn, &f.port, &f.run.run_id, &version.entry_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn source_index_rejects_structurally_invalid_rows_with_matching_set_hash() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    let mut invalid_source = f.source();
    invalid_source.record_seq = 0;
    let sources_hash = set_hash(&[invalid_source.clone()]).unwrap();
    f.conn
        .execute(
            "UPDATE memory_version_sources SET source_json=?1 WHERE version_id=?2",
            params![canonical(&invalid_source).unwrap(), version.version_id],
        )
        .unwrap();
    f.conn
        .execute(
            "UPDATE memory_versions SET source_set_hash=?1 WHERE version_id=?2",
            params![sources_hash.as_slice(), version.version_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.version_sources(&f.conn, &version.version_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn unknown_algorithm_version_keeps_frozen_run_read_only() {
    let f = Fixture::new();
    let mut future = policy();
    future.algorithm_version = 2;
    let raw = canonical(&future).unwrap();
    f.conn
        .execute(
            "UPDATE memory_runs SET policy_json=?1,policy_hash=?2",
            params![raw, hash(&raw).as_slice()],
        )
        .unwrap();
    let changes = f.conn.total_changes();
    rejected(
        f.repo.run(&f.conn, &f.run.run_id),
        Reason::PolicyUnavailable,
    );
    assert_eq!(f.conn.total_changes(), changes);
}

#[test]
fn missing_algorithm_version_in_frozen_policy_is_corrupt_without_defaulting() {
    let f = Fixture::new();
    let mut incomplete = aoidos_json::to_value(policy()).unwrap();
    incomplete
        .as_object_mut()
        .unwrap()
        .remove("algorithmVersion");
    let raw = canonical(&incomplete).unwrap();
    f.conn
        .execute(
            "UPDATE memory_runs SET policy_json=?1,policy_hash=?2",
            params![raw, hash(&raw).as_slice()],
        )
        .unwrap();
    let changes = f.conn.total_changes();
    assert!(matches!(
        f.repo.run(&f.conn, &f.run.run_id),
        Err(Error::Corrupt)
    ));
    assert_eq!(f.conn.total_changes(), changes);
}

#[test]
fn sqlite_text_values_do_not_masquerade_as_binary_hashes_or_evidence() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    f.conn
        .execute(
            "INSERT INTO memory_version_evidence VALUES(?1,?2,?3)",
            params![version.version_id, new_id(), "{}"],
        )
        .unwrap();
    assert!(matches!(
        f.repo
            .current(&f.conn, &f.port, &f.run.run_id, &version.entry_id),
        Err(Error::Corrupt)
    ));
    f.conn
        .execute("DELETE FROM memory_version_evidence", [])
        .unwrap();
    f.conn
        .execute("UPDATE memory_runs SET policy_hash=?1", ["x".repeat(32)])
        .unwrap();
    assert!(matches!(
        f.repo.run(&f.conn, &f.run.run_id),
        Err(Error::Corrupt)
    ));
}

#[test]
fn inconsistent_declared_blob_length_is_rejected_before_deserializing() {
    let f = Fixture::new();
    let raw = canonical(&f.source()).unwrap();
    f.conn.execute_batch("CREATE TEMP TABLE blob_fixture(id TEXT PRIMARY KEY,declared_length INTEGER NOT NULL,payload BLOB NOT NULL)").unwrap();
    f.conn
        .execute(
            "INSERT INTO blob_fixture VALUES(?1,?2,?3)",
            params![f.run.run_id, raw.len() + 1, raw],
        )
        .unwrap();
    assert!(matches!(
        read_blobs(
            &f.conn,
            "SELECT declared_length,payload FROM blob_fixture WHERE id=?1",
            &f.run.run_id,
            1
        ),
        Err(Error::Corrupt)
    ));
    let value = f
        .conn
        .query_row(
            "SELECT declared_length,payload FROM blob_fixture WHERE id=?1",
            [&f.run.run_id],
            |row| read_bounded_blob(row, 0, 1, 32768),
        )
        .unwrap();
    assert!(matches!(value.into_result(), Err(Error::Corrupt)));
}

#[test]
fn oversized_relation_index_cannot_allocate_beyond_recovery_budget() {
    let mut f = Fixture::new();
    let version = f.version();
    f.commit(&f.plan(version.clone()));
    f.conn
        .execute(
            "UPDATE memory_version_sources SET source_json=?1 WHERE version_id=?2",
            params![vec![b' '; MAX_SCAN_BYTES + 1], version.version_id],
        )
        .unwrap();
    assert!(matches!(
        f.repo.version_relation_bytes(&f.conn, &version.version_id),
        Err(Error::Corrupt)
    ));
}
