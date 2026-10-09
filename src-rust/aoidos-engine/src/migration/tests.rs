use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
struct Events {
    seq: AtomicU64,
    fail: bool,
    retired: Mutex<Vec<String>>,
    invalid_seq: bool,
    regress: std::sync::atomic::AtomicBool,
}
impl MigrationEvents for Events {
    fn prepare(&self, event: &MigrationEvent) -> Result<u64, Fault> {
        let _ = event.id();
        let _ = event.name();
        if self.invalid_seq {
            return Ok(0);
        }
        if self.regress.load(Ordering::SeqCst) {
            return Ok(1);
        }
        Ok(self.seq.fetch_add(1, Ordering::SeqCst) + 1)
    }
    fn deliver(&self, _: u64, _: MigrationEvent) -> Result<(), Fault> {
        if self.fail {
            Err(Fault::event())
        } else {
            Ok(())
        }
    }
    fn retire(&self, id: &str) {
        self.retired.lock().unwrap().push(id.into());
    }
    fn delivery_error(&self, _: &str) -> Option<Fault> {
        self.fail.then(Fault::event)
    }
}
#[test]
fn committed_steps_noop_open_and_failures_preserve_real_versions() {
    let dir = std::env::temp_dir().join(format!("aoidos-migration-{}", uuid::Uuid::new_v4()));
    let path = dir.join("storage.sqlite");
    let events = Arc::new(Events {
        seq: AtomicU64::new(0),
        fail: true,
        retired: Mutex::new(Vec::new()),
        invalid_seq: false,
        regress: std::sync::atomic::AtomicBool::new(false),
    });
    let flow = Migration::idle(&path, events.clone()).unwrap();
    assert!(!path.exists());
    assert_eq!(flow.snapshot().phase, Phase::Idle);
    let migrations = [
        "CREATE TABLE first(id INTEGER PRIMARY KEY);",
        "CREATE TABLE second(id INTEGER PRIMARY KEY);",
    ];
    drop(flow.open(&path, &migrations).unwrap());
    let state = flow.snapshot();
    assert_eq!(state.phase, Phase::Completed);
    assert_eq!(state.current, Some(2));
    assert!(state.delivery_error.is_some());
    assert!(state.seq.done > 0);
    events.regress.store(true, Ordering::SeqCst);
    flow.publish(MigrationEvent::Progress {
        migration_id: state.migration_id.unwrap(),
        from: 0,
        to: 1,
        current: 1,
        target: 2,
    });
    assert_eq!(flow.snapshot().seq.progress, state.seq.progress);
    events.regress.store(false, Ordering::SeqCst);
    drop(flow.open(&path, &migrations).unwrap());
    assert!(flow.snapshot().last_step.is_none());
    assert_eq!(events.retired.lock().unwrap().len(), 1);
    let failure = flow
        .open(&path, &[migrations[0], migrations[1], "INVALID SQL;"])
        .unwrap_err();
    assert_eq!(failure.code, "store.migration");
    let state = flow.snapshot();
    assert_eq!(state.phase, Phase::Failed);
    assert_eq!(state.current, Some(2));
    assert_eq!(state.failed_step.unwrap().to, 3);
    assert_eq!(state.seq.done, 0);
    assert_eq!(db::current_version(&path).unwrap(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unknown_initial_versions_schema_failure_and_invalid_events_remain_queryable() {
    let root =
        std::env::temp_dir().join(format!("aoidos-migration-errors-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let events = Arc::new(Events {
        seq: AtomicU64::new(0),
        fail: false,
        invalid_seq: true,
        regress: std::sync::atomic::AtomicBool::new(false),
        retired: Mutex::new(Vec::new()),
    });
    let flow = Migration::new(events.clone());
    let blocked = root.join("bad.sqlite");
    std::fs::write(&blocked, b"not sqlite").unwrap();
    assert!(Migration::idle(&blocked, events.clone()).is_err());
    assert!(flow.open(&blocked, &[]).is_err());
    assert_eq!(flow.snapshot().phase, Phase::Failed);
    assert!(flow.snapshot().current.is_none());
    let path = root.join("empty.sqlite");
    drop(flow.open(&path, &[]).unwrap());
    assert_eq!(flow.snapshot().phase, Phase::Completed);
    assert!(flow.snapshot().delivery_error.is_some());
    let error = flow
        .open_verified(&path, &[], |_| {
            Err(StoreError::Corrupt("missing schema".into()))
        })
        .unwrap_err();
    assert_eq!(error.code, "store.corrupt");
    assert_eq!(flow.snapshot().phase, Phase::Failed);
    let storage = crate::storage::Storage::new(
        &root,
        events.clone(),
        Arc::new(crate::record::test_support::Events::default()),
    );
    storage.ready().unwrap();
    storage.close().unwrap();
    assert_eq!(storage.ready().unwrap_err().code, "app.not-ready");
    let conn = rusqlite::Connection::open(root.join("storage.sqlite")).unwrap();
    conn.execute("DROP TABLE store_applied", []).unwrap();
    drop(conn);
    let storage = crate::storage::Storage::new(
        &root,
        events,
        Arc::new(crate::record::test_support::Events::default()),
    );
    assert!(storage.ready().is_err());
    assert_eq!(
        storage.migration.snapshot().error.unwrap().code,
        "store.corrupt"
    );
    storage.close().unwrap();
    assert!(
        MigrationEvent::Progress {
            migration_id: "id".into(),
            from: 0,
            to: 1,
            current: 1,
            target: 1
        }
        .id()
            == "id"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ports_without_async_diagnostics_keep_the_default_snapshot_contract() {
    struct Plain(Events);
    impl MigrationEvents for Plain {
        fn prepare(&self, event: &MigrationEvent) -> Result<u64, Fault> {
            self.0.prepare(event)
        }
        fn deliver(&self, seq: u64, event: MigrationEvent) -> Result<(), Fault> {
            self.0.deliver(seq, event)
        }
        fn retire(&self, id: &str) {
            self.0.retire(id);
        }
    }
    let root =
        std::env::temp_dir().join(format!("aoidos-migration-plain-{}", uuid::Uuid::new_v4()));
    let path = root.join("storage.sqlite");
    let events = Arc::new(Plain(Events {
        seq: AtomicU64::new(0),
        fail: false,
        retired: Mutex::new(vec![]),
        invalid_seq: false,
        regress: std::sync::atomic::AtomicBool::new(false),
    }));
    let flow = Migration::new(events);
    drop(flow.open(&path, &[]).unwrap());
    drop(flow.open(&path, &[]).unwrap());
    assert!(flow.snapshot().delivery_error.is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn verified_open_keeps_all_failure_stages_and_retry_in_one_validator_contract() {
    let root = std::env::temp_dir().join(format!("aoidos-verified-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let events = Arc::new(Events {
        seq: AtomicU64::new(0),
        fail: false,
        invalid_seq: false,
        regress: std::sync::atomic::AtomicBool::new(false),
        retired: Mutex::new(Vec::new()),
    });
    let flow = Migration::new(events.clone());
    let bad = root.join("bad.sqlite");
    std::fs::write(&bad, b"not sqlite").unwrap();
    let good = root.join("good.sqlite");
    for (path, sql, reject, expected) in [
        (
            &bad,
            "CREATE TABLE test(id INTEGER);",
            false,
            Some("store.corrupt"),
        ),
        (&good, "INVALID SQL;", false, Some("store.migration")),
        (
            &good,
            "CREATE TABLE test(id INTEGER);",
            true,
            Some("store.corrupt"),
        ),
        (&good, "CREATE TABLE test(id INTEGER);", false, None),
    ] {
        let result = flow.open_verified(path, &[sql], |connection| {
            let count: i64 = connection
                .query_row("SELECT count(*) FROM test", [], |row| row.get(0))
                .map_err(db::sqlite_error)?;
            assert_eq!(count, 0);
            if reject {
                Err(StoreError::Corrupt("schema rejected".into()))
            } else {
                Ok(())
            }
        });
        if let Some(code) = expected {
            assert_eq!(result.unwrap_err().code, code);
            assert_eq!(flow.snapshot().phase, Phase::Failed);
            assert_eq!(flow.snapshot().seq.done, 0);
        } else {
            drop(result.unwrap());
            assert_eq!(flow.snapshot().phase, Phase::Completed);
            assert!(flow.snapshot().seq.done > 0);
        }
    }
    assert_eq!(events.retired.lock().unwrap().len(), 3);
    std::fs::remove_dir_all(root).unwrap();
}
