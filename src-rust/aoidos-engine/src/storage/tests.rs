use super::*;
use crate::{migration::MigrationEvent, record::test_support};
use aoidos_memory::{
    error::{Error, Reason},
    model::{Algorithm, CausalPort, Policy},
};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct Events(AtomicU64);
impl MigrationEvents for Events {
    fn prepare(&self, _: &MigrationEvent) -> Result<u64, Fault> {
        Ok(self.0.fetch_add(1, Ordering::SeqCst) + 1)
    }
    fn deliver(&self, _: u64, _: MigrationEvent) -> Result<(), Fault> {
        Ok(())
    }
    fn retire(&self, _: &str) {}
}
fn policy() -> Policy {
    Policy {
        version: 1,
        algorithm_version: 1,
        config_version: 1,
        app_config_version: 1,
        algorithm: Algorithm::None,
        base_strength: 100_000,
        reinforcement_gain: 10_000,
        reinforcement_cap: 100_000,
        strength_cap: 500_000,
        half_life_rounds: 10,
        repeat_window_rounds: 1,
        repeat_limit: 1,
        revival_cooldown_rounds: 1,
        active_capacity: 100,
        rate_table_version: 1,
        decay_rate: 1,
    }
}
#[test]
fn memory_borrows_registered_session_and_shared_migrated_connection() {
    let root = std::env::temp_dir().join(format!("aoidos-memory-storage-{}", uuid::Uuid::new_v4()));
    let storage = Storage::new(
        &root,
        Arc::new(Events::default()),
        Arc::new(test_support::Events::default()),
    );
    let header = test_support::header();
    let session_id = header.session_id.clone();
    storage.records.create(header).unwrap();
    let run = storage
        .with_memory(&session_id, |repository, conn, port| {
            assert_eq!(port.boundary()?.session_id, session_id);
            repository.create_run(conn, port, policy())
        })
        .unwrap();
    assert_eq!(run.session_id, session_id);
    storage
        .with_database(|conn| {
            let count: i64 = conn
                .query_row("SELECT count(*) FROM memory_runs", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        storage
            .with_memory(&uuid::Uuid::new_v4().to_string(), |_, _, _| Ok(()))
            .unwrap_err()
            .code(),
        "app.not-found"
    );
    assert_eq!(
        storage
            .with_memory(&session_id, |_, _, _| Err::<(), _>(Error::Corrupt))
            .unwrap_err()
            .code(),
        "store.corrupt"
    );
    for reason in [
        Reason::StaleVersion,
        Reason::CapacityBlocked,
        Reason::PolicyUnavailable,
    ] {
        let error = storage
            .with_memory(&session_id, |_, _, _| Err::<(), _>(Error::Rejected(reason)))
            .unwrap_err();
        assert!(matches!(error, Error::Rejected(actual) if actual == reason));
    }
    storage.close().unwrap();
    assert!(matches!(
        storage.with_memory(&session_id, |_, _, _| Ok(())),
        Err(Error::Rejected(Reason::RecoveryRequired))
    ));
    drop(storage);
    let reopened = Storage::new(
        &root,
        Arc::new(Events::default()),
        Arc::new(test_support::Events::default()),
    );
    reopened.records.open(&run.script_id, &session_id).unwrap();
    let recovered = reopened
        .with_memory(&session_id, |repository, conn, port| {
            let mut latest = policy();
            latest.config_version = 2;
            latest.base_strength = 150_000;
            repository.create_run(conn, port, latest)
        })
        .unwrap();
    assert_eq!(recovered.run_id, run.run_id);
    assert_eq!(recovered.policy, run.policy);
    reopened.close().unwrap();
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn registration_faults_map_to_typed_memory_context_errors() {
    assert!(matches!(
        memory_context_error(Fault::new("app.not-found", "unknown")),
        Error::NotFound
    ));
    assert!(matches!(
        memory_context_error(Fault::bad_request()),
        Error::Rejected(Reason::InvalidSchema)
    ));
    assert!(matches!(
        memory_context_error(Fault::new("app.not-ready", "closed")),
        Error::Rejected(Reason::RecoveryRequired)
    ));
}
