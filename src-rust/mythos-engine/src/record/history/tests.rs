use super::*;
use crate::record::{session::Target, test_support::Fixture};

struct SqlHistory {
    database: rusqlite::Connection,
    fail: bool,
    rebuilds: usize,
}
impl SqlHistory {
    fn new() -> Self {
        let database = rusqlite::Connection::open_in_memory().unwrap();
        database
            .execute_batch(mythos_store::applied::SCHEMA)
            .unwrap();
        database
            .execute_batch("CREATE TABLE fixture_path(seq INTEGER PRIMARY KEY)")
            .unwrap();
        Self {
            database,
            fail: false,
            rebuilds: 0,
        }
    }
}
impl HistoryPort for SqlHistory {
    fn verify_target(&self, session: &Session, target: u64) -> Result<(), Fault> {
        session.read(target).map(drop).map_err(store_fault)
    }
    fn applied(&self, operation: &str, hash: &str) -> Result<bool, Fault> {
        mythos_store::applied::contains(&self.database, operation, hash).map_err(store_fault)
    }
    fn rebuild(
        &mut self,
        _: &Session,
        path: &[u64],
        operation: &str,
        hash: &str,
    ) -> Result<(), Fault> {
        if self.fail {
            return Err(Fault::new("store.io", "合成重建故障"));
        }
        mythos_store::applied::apply_once(
            &mut self.database,
            operation,
            hash,
            &mut |transaction| {
                transaction
                    .execute("DELETE FROM fixture_path", [])
                    .map_err(mythos_store::db::sqlite_error)?;
                for seq in path {
                    transaction
                        .execute("INSERT INTO fixture_path VALUES(?1)", [seq])
                        .map_err(mythos_store::db::sqlite_error)?;
                }
                Ok(())
            },
        )
        .map_err(store_fault)?;
        self.rebuilds += 1;
        Ok(())
    }
}
fn player(text: &str) -> Body {
    Body::PlayerSpeech {
        player_id: "p".into(),
        text: text.into(),
        mode: None,
        content_range: None,
    }
}
fn fork(session: &Session, target: u64) -> Body {
    let path = session
        .path_at(session.history_revision)
        .unwrap()
        .into_iter()
        .filter(|seq| *seq <= target)
        .collect::<Vec<_>>();
    Body::System {
        code: "historyFork".into(),
        message: "恢复因果分支".into(),
        related_seq: None,
        turn_id: None,
        data: serde_json::json!({"version":1,"operationId":uuid::Uuid::new_v4().to_string(),"mode":"rewind","parentControlSeq":session.history_revision,"targetSeq":target,"prefixHash":session.prefix_hash(&path).unwrap()}),
    }
}
#[test]
fn pending_fork_recovery_confirms_once_and_keeps_control_out_of_paths() {
    let mut fixture = Fixture::new();
    let mut port = SqlHistory::new();
    fixture.session.append(player("根节点")).unwrap();
    let body = fork(&fixture.session, 1);
    port.fail = true;
    assert!(
        fixture
            .session
            .commit_history_fork(body, &mut port)
            .is_err()
    );
    assert_eq!(fixture.session.last_event_seq(), 1);
    assert!(fixture.session.needs_recovery());
    assert!(fixture.session.append(player("不得推进")).is_err());
    port.fail = false;
    fixture.session.recover_history(&mut port).unwrap();
    assert_eq!(fixture.session.last_event_seq(), 2);
    assert_eq!(
        fixture
            .events
            .delivered
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .view_epoch,
        fixture.session.view_epoch()
    );
    assert_eq!(fixture.session.path_at(2).unwrap(), vec![1]);
    fixture.session.append(player("第一分支")).unwrap();
    let old_page = fixture.session.page(None, Some(1)).unwrap();
    let body = fork(&fixture.session, 3);
    fixture
        .session
        .commit_history_fork(body, &mut port)
        .unwrap();
    fixture.session.append(player("第二分支")).unwrap();
    assert_eq!(fixture.session.path_at(0).unwrap(), vec![1]);
    assert_eq!(fixture.session.path_at(2).unwrap(), vec![1, 3]);
    assert_eq!(fixture.session.path_at(4).unwrap(), vec![1, 3, 5]);
    assert_eq!(
        fixture
            .session
            .projection_working_set()
            .unwrap()
            .records()
            .iter()
            .map(|record| record.seq)
            .collect::<Vec<_>>(),
        vec![1, 3, 5]
    );
    assert!(
        fixture
            .session
            .page(old_page.next_cursor.as_deref(), Some(1))
            .is_err()
    );
    fixture.session.recover_history(&mut port).unwrap();
    assert_eq!(port.rebuilds, 2);
    assert_eq!(fixture.session.last_event_seq(), 5);
    assert_eq!(fixture.events.delivered.lock().unwrap().len(), 5);
}

#[test]
fn crash_orphan_stays_in_its_effective_branch() {
    let mut fixture = Fixture::new();
    let mut port = SqlHistory::new();
    fixture.session.append(player("根节点")).unwrap();
    let body = fork(&fixture.session, 1);
    fixture
        .session
        .commit_history_fork(body, &mut port)
        .unwrap();
    let turn = uuid::Uuid::new_v4().to_string();
    fixture
        .session
        .delta(&turn, &Target::Narration, 1, "已经落盘", true)
        .unwrap();
    fixture.session.close().unwrap();
    let mut reopened = Session::open(fixture.path.clone(), fixture.events.clone()).unwrap();
    reopened.recover_history(&mut port).unwrap();
    assert_eq!(reopened.path_at(2).unwrap(), vec![1, 3, 4]);
    assert_eq!(reopened.read(4).unwrap().body.unwrap().branch_seq, Some(2));
    assert!(!reopened.needs_recovery());
}

#[test]
fn read_only_and_frozen_history_never_rebuild_sql() {
    let mut fixture = Fixture::new();
    let mut port = SqlHistory::new();
    fixture.session.read_only = true;
    assert!(fixture.session.recover_history(&mut port).is_err());
    fixture.session.read_only = false;
    fixture.session.close().unwrap();
    assert!(fixture.session.recover_history(&mut port).is_err());
    assert_eq!(port.rebuilds, 0);
    assert!(fixture.session.path_at(999).is_err());
}

#[test]
fn invalid_fork_targets_and_corrupt_control_chains_never_rebuild_sql() {
    let mut fixture = Fixture::new();
    let mut port = SqlHistory::new();
    fixture.session.append(player("根节点")).unwrap();
    assert!(fixture.session.path_at(1).is_err());
    assert!(
        fixture
            .session
            .commit_history_fork(player("错误 kind"), &mut port)
            .is_err()
    );
    let orphan = Body::System {
        code: "orphan".into(),
        message: "诊断".into(),
        related_seq: None,
        turn_id: None,
        data: serde_json::json!({"version":1,"recoveryId":crate::record::format::hash(b"orphan")}),
    };
    fixture.session.append(orphan.clone()).unwrap();
    assert!(fixture.session.path_at(2).is_err());
    assert!(
        fixture
            .session
            .commit_history_fork(orphan, &mut port)
            .is_err()
    );
    for (parent, target, hash) in [
        (1, 1, fixture.session.prefix_hash(&[1]).unwrap()),
        (0, 999, fixture.session.prefix_hash(&[1]).unwrap()),
        (0, 1, crate::record::format::hash(b"wrong")),
    ] {
        let body = Body::System {
            code: "historyFork".into(),
            message: "错误控制".into(),
            related_seq: None,
            turn_id: None,
            data: serde_json::json!({"version":1,"operationId":uuid::Uuid::new_v4().to_string(),"mode":"rewind","parentControlSeq":parent,"targetSeq":target,"prefixHash":hash}),
        };
        assert!(
            fixture
                .session
                .commit_history_fork(body, &mut port)
                .is_err()
        );
    }
    let body = fork(&fixture.session, 1);
    fixture
        .session
        .commit_history_fork(body, &mut port)
        .unwrap();
    assert_eq!(
        fixture.session.prefix_hash(&[1, 3]).unwrap(),
        fixture.session.prefix_hash(&[1]).unwrap()
    );
    let path = fixture.path.clone();
    let events = fixture.events.clone();
    fixture.session.close().unwrap();
    let control = serde_json::json!({"seq":10,"kind":"system","createdAt":crate::record::format::now(),"code":"historyFork","message":"错误根控制","data":{"version":1,"operationId":uuid::Uuid::new_v4().to_string(),"mode":"rewind","parentControlSeq":0,"targetSeq":1,"prefixHash":crate::record::format::hash(fixture.session.read(1).unwrap().raw.as_bytes())}});
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(crate::record::format::line(&control).unwrap().as_bytes())
        .unwrap();
    drop(file);
    let mut reopened = Session::open(path, events).unwrap();
    assert!(reopened.recover_history(&mut port).is_err());
    assert!(reopened.needs_recovery());
    assert_eq!(port.rebuilds, 1);
}

#[test]
fn stored_fork_must_reference_an_ancestor_with_its_exact_prefix_hash() {
    for (target, hash) in [
        (999, crate::record::format::hash(b"bad")),
        (4, crate::record::format::hash(b"bad")),
        (1, crate::record::format::hash(b"bad")),
    ] {
        let mut fixture = Fixture::new();
        fixture.session.append(player("根")).unwrap();
        fixture.session.next_seq = 10;
        let body = Body::System {
            code: "historyFork".into(),
            message: "损坏控制".into(),
            related_seq: None,
            turn_id: None,
            data: serde_json::json!({"version":1,"operationId":uuid::Uuid::new_v4().to_string(),"mode":"rewind","parentControlSeq":0,"targetSeq":target,"prefixHash":hash}),
        };
        fixture.session.append_intent(body).unwrap();
        assert!(fixture.session.path_at(10).is_err());
    }
    assert!(
        fork_identity(
            "orphan",
            &serde_json::json!({"version":1,"recoveryId":crate::record::format::hash(b"r")})
        )
        .is_err()
    );
}

#[test]
fn preapplied_fork_does_not_rebuild_and_unrelated_pending_confirmation_is_preserved() {
    struct Applied;
    impl HistoryPort for Applied {
        fn verify_target(&self, session: &Session, target: u64) -> Result<(), Fault> {
            session.read(target).map(drop).map_err(store_fault)
        }
        fn applied(&self, _: &str, _: &str) -> Result<bool, Fault> {
            Ok(true)
        }
        fn rebuild(&mut self, _: &Session, _: &[u64], _: &str, _: &str) -> Result<(), Fault> {
            panic!("已 applied 不得重复重建")
        }
    }
    let mut fixture = Fixture::new();
    fixture.session.append(player("根")).unwrap();
    let body = fork(&fixture.session, 1);
    fixture
        .session
        .commit_history_fork(body, &mut Applied)
        .unwrap();
    let seq = fixture.session.append_intent(player("未确认")).unwrap();
    fixture.session.recover_history(&mut Applied).unwrap();
    assert!(fixture.session.pending_confirmations().contains(&seq));
}
