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
