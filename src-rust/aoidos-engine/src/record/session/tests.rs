use super::*;

#[test]
fn lost_record_delivery_does_not_rollback_and_regressing_preparation_cannot_confirm() {
    let mut fixture = crate::record::test_support::Fixture::new();
    fixture
        .events
        .fail_delivery
        .store(true, std::sync::atomic::Ordering::SeqCst);
    fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "p".into(),
            text: "已确认".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    assert_eq!(fixture.session.last_event_seq(), 1);
    assert_eq!(fixture.session.read(1).unwrap().seq, 1);
    assert!(fixture.events.delivered.lock().unwrap().is_empty());
    fixture
        .events
        .seq
        .store(0, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        fixture.session.confirm_intent(1).unwrap_err().code,
        "app.event-failed"
    );
    assert_eq!(fixture.session.last_event_seq(), 1);
}

#[test]
fn empty_crash_and_repeated_cleanup_preserve_one_recovery_marker() {
    let (path, header, events) = setup();
    let turn = uuid::Uuid::new_v4().to_string();
    let mut session = Session::create(path.clone(), header, events.clone()).unwrap();
    session.ensure_partial(&turn, &Target::Narration).unwrap();
    let partial = session.active.as_ref().unwrap().path.clone();
    let bytes = std::fs::read(&partial).unwrap();
    drop(session);
    let first = Session::open(path.clone(), events.clone()).unwrap();
    assert_eq!(first.index.len(), 1);
    assert_eq!(first.read(2).unwrap().kind, "system");
    drop(first);
    std::fs::write(&partial, &bytes).unwrap();
    let second = Session::open(path.clone(), events.clone()).unwrap();
    assert_eq!(second.index.len(), 1);
    drop(second);
    std::fs::write(&partial, bytes).unwrap();
    let original = std::fs::read_to_string(&path).unwrap();
    let mut lines = original
        .lines()
        .map(|line| format::json(&format!("{line}\n")).unwrap())
        .collect::<Vec<_>>();
    lines[1]["turnId"] = serde_json::json!(uuid::Uuid::new_v4().to_string());
    std::fs::write(
        &path,
        lines
            .iter()
            .map(|line| format::line(line).unwrap())
            .collect::<String>(),
    )
    .unwrap();
    assert_eq!(
        Session::open(path.clone(), events).err().unwrap().code,
        "store.corrupt"
    );
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn malformed_sidecar_multiple_writers_and_changed_formal_rows_are_rejected() {
    for case in ["part", "meta", "guard", "mismatch", "behind", "multiple"] {
        let (path, header, events) = setup();
        let mut session = Session::create(path.clone(), header, events.clone()).unwrap();
        let turn = uuid::Uuid::new_v4().to_string();
        session
            .delta(
                &turn,
                &Target::Narration,
                1,
                if case == "guard" {
                    "\n[AOIDOS:PLAYER id=p]"
                } else {
                    "原文"
                },
                true,
            )
            .unwrap();
        let partial = session.active.as_ref().unwrap().path.clone();
        let bytes = std::fs::read_to_string(&partial).unwrap();
        let mut lines = bytes
            .lines()
            .map(|line| format::json(&format!("{line}\n")).unwrap())
            .collect::<Vec<_>>();
        if case == "part" {
            lines[1]["partSeq"] = serde_json::json!(99);
        }
        if case == "meta" {
            lines[0]["sessionId"] = serde_json::json!(uuid::Uuid::new_v4().to_string());
        }
        std::fs::write(
            &partial,
            lines
                .iter()
                .map(|line| format::line(line).unwrap())
                .collect::<String>(),
        )
        .unwrap();
        if case == "multiple" {
            std::fs::copy(
                &partial,
                path.with_file_name(format!(
                    "{}.{}.partial.jsonl",
                    session.header.session_id,
                    uuid::Uuid::new_v4()
                )),
            )
            .unwrap();
        }
        if case == "mismatch" || case == "behind" {
            let record = Record {
                seq: if case == "mismatch" { 1 } else { 10 },
                created_at: format::now(),
                branch_seq: None,
                body: Body::Narration {
                    text: "不同正文".into(),
                    turn_id: turn.clone(),
                    terminal: Terminal::Cancelled,
                },
            };
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            file.write_all(format::line(&record).unwrap().as_bytes())
                .unwrap();
        }
        drop(session);
        assert_eq!(
            Session::open(path.clone(), events).err().unwrap().code,
            "store.corrupt",
            "{case}"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}

#[tokio::test]
async fn writer_boundaries_closing_errors_and_event_failures_remain_desensitized() {
    let (path, header, events) = setup();
    let mut session = Session::create(path.clone(), header, events.clone()).unwrap();
    assert!(session.ensure_partial("bad", &Target::Narration).is_err());
    session.next_seq = MAX_SEQ + 1;
    assert!(session.reserve().is_err());
    session.next_seq = 1;
    let turn = uuid::Uuid::new_v4().to_string();
    let partial = path.with_file_name(format!(
        "{}.{}.partial.jsonl",
        session.header.session_id, turn
    ));
    std::fs::write(&partial, b"existing").unwrap();
    assert!(session.ensure_partial(&turn, &Target::Narration).is_err());
    assert!(session.frozen);
    drop(session);
    std::fs::remove_file(partial).unwrap();
    let session = Arc::new(Mutex::new(Session::open(path.clone(), events).unwrap()));
    let writer = PersistentWriter {
        session: session.clone(),
        target: Target::Narration,
        high: false,
    };
    writer.begin(&turn).await.unwrap();
    writer.append(&turn, 1, "前文").await.unwrap();
    assert!(writer.append(&turn, 2, "\r").await.is_err());
    writer.finish(&turn, &Terminal::Cancelled).await.unwrap();
    assert!(writer.begin(&turn).await.is_err());
    {
        let mut state = session.lock().unwrap();
        assert!(state.confirm_intent(999).is_err());
        state.confirm_intent(1).unwrap();
        state
            .log
            .inject_fault(aoidos_store::journal::FaultStage::Sync);
        assert!(state.close().is_err());
    }
    for code in [
        "store.io",
        "store.disk-full",
        "store.permission",
        "store.not-found",
        "store.locked",
        "engine.invalid-phase",
    ] {
        let error = recovery_store_error(Fault::new(code, "private"));
        assert!(!error.to_string().contains("private"));
    }
    drop(writer);
    drop(session);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
use std::sync::atomic::{AtomicU64, Ordering};
struct Events(AtomicU64);
impl RecordEvents for Events {
    fn prepare(&self, _: &Appended) -> std::result::Result<u64, Fault> {
        Ok(self.0.fetch_add(1, Ordering::SeqCst) + 1)
    }
    fn deliver(&self, _: u64, _: Appended) -> std::result::Result<(), Fault> {
        Ok(())
    }
}
fn setup() -> (PathBuf, Header, Arc<Events>) {
    let session_id = uuid::Uuid::new_v4().to_string();
    let path = std::env::temp_dir()
        .join(format!("aoidos-record-{session_id}"))
        .join(format!("{session_id}.jsonl"));
    let prefix = "[AOIDOS:STATIC]\n只推进已知场景。\n[/AOIDOS:STATIC]\n".to_owned();
    let header = Header {
        kind: "header".into(),
        format_version: 1,
        grammar_version: 1,
        projection_version: 1,
        script_id: "demo".into(),
        session_id,
        created_at: format::now(),
        static_prefix_hash: format::hash(prefix.as_bytes()),
        static_prefix: prefix,
        script_revision: format::hash(b"script"),
    };
    (path, header, Arc::new(Events(AtomicU64::new(0))))
}
#[test]
fn reopen_preserves_confirmed_partial_and_orphan_once() {
    let (path, header, events) = setup();
    let turn = uuid::Uuid::new_v4().to_string();
    let mut session = Session::create(path.clone(), header, events.clone()).unwrap();
    session
        .delta(&turn, &Target::Narration, 1, "前文😀", false)
        .unwrap();
    let partial = session.active.as_ref().unwrap().path.clone();
    assert_eq!(session.in_flight(), Some((turn.clone(), 1)));
    assert_eq!(session.view(None).unwrap().in_flight.unwrap().turn_id, turn);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&partial)
        .unwrap();
    use std::io::Write;
    file.write_all(b"{\"partSeq\":2").unwrap();
    drop(file);
    drop(session);
    let session = Session::open(path.clone(), events.clone()).unwrap();
    assert_eq!(session.index.len(), 2);
    let body = session.read(1).unwrap().body.unwrap().body;
    assert!(
        matches!(body,Body::Narration{text,terminal:Terminal::Failed{error,..},..} if text=="前文😀"&&error.code=="engine.interrupted")
    );
    assert!(session.in_flight().is_none());
    assert!(!partial.exists());
    drop(session);
    let session = Session::open(path.clone(), events).unwrap();
    assert_eq!(session.index.len(), 2);
    drop(session);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn sealed_text_matches_deltas_and_partial_blocks_other_appends() {
    let (path, header, events) = setup();
    let mut session = Session::create(path.clone(), header, events).unwrap();
    let turn = uuid::Uuid::new_v4().to_string();
    session
        .delta(&turn, &Target::Character("keeper".into()), 1, "灯", true)
        .unwrap();
    assert!(
        session
            .delta(&turn, &Target::Narration, 1, "重复", true)
            .is_err()
    );
    assert!(
        session
            .append(Body::System {
                code: "orphan".into(),
                message: "x".into(),
                related_seq: None,
                turn_id: None,
                data: serde_json::json!({"version":1})
            })
            .is_err()
    );
    session
        .delta(
            &turn,
            &Target::Character("keeper".into()),
            2,
            "亮着。",
            true,
        )
        .unwrap();
    session
        .seal(
            &turn,
            &Target::Character("keeper".into()),
            &Terminal::Completed {
                finish_reason: aoidos_llm::schedule::FinishReason::Stop,
            },
        )
        .unwrap();
    assert_eq!(
        body_text(&session.read(1).unwrap().body.unwrap().body),
        Some("灯亮着。")
    );
    assert_eq!(session.last_event_seq, 1);
    drop(session);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn append_faults_never_confirm_failed_chunks_and_uncertain_seal_freezes() {
    use aoidos_store::journal::FaultStage;
    for (stage, uncertain) in [
        (FaultStage::Write, false),
        (FaultStage::Flush, false),
        (FaultStage::Rollback, true),
        (FaultStage::Sync, true),
    ] {
        let (path, header, events) = setup();
        let mut session = Session::create(path.clone(), header, events).unwrap();
        let turn = uuid::Uuid::new_v4().to_string();
        session
            .delta(&turn, &Target::Narration, 1, "前文", true)
            .unwrap();
        session.active.as_mut().unwrap().log.inject_fault(stage);
        assert!(
            session
                .delta(&turn, &Target::Narration, 2, "未确认", true)
                .is_err()
        );
        assert_eq!(session.active.as_ref().unwrap().text, "前文");
        assert_eq!(session.frozen, uncertain);
        if !uncertain {
            session
                .seal(
                    &turn,
                    &Target::Narration,
                    &Terminal::failed(Fault::new("store.io", "写入失败")),
                )
                .unwrap();
            assert_eq!(
                body_text(&session.read(1).unwrap().body.unwrap().body),
                Some("前文")
            );
        }
        drop(session);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    let (path, header, events) = setup();
    let mut session = Session::create(path.clone(), header, events.clone()).unwrap();
    events.0.store(MAX_SEQ, std::sync::atomic::Ordering::SeqCst);
    assert!(
        session
            .append(Body::PlayerSpeech {
                player_id: "p".into(),
                text: "x".into(),
                mode: None,
                content_range: None
            })
            .is_err()
    );
    assert!(session.index.is_empty());
    events.0.store(0, std::sync::atomic::Ordering::SeqCst);
    let turn = uuid::Uuid::new_v4().to_string();
    session
        .delta(&turn, &Target::Narration, 1, "封口前文", false)
        .unwrap();
    session.log.inject_fault(FaultStage::Sync);
    assert!(
        session
            .seal(&turn, &Target::Narration, &Terminal::Cancelled)
            .is_err()
    );
    assert!(session.frozen);
    assert!(session.active.is_some());
    drop(session);
    let reopened = Session::open(path.clone(), Arc::new(Events(AtomicU64::new(0)))).unwrap();
    assert_eq!(reopened.index.len(), 2);
    drop(reopened);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn future_control_opens_read_only_without_reinterpreting_its_branch() {
    let (path, header, events) = setup();
    let mut session = Session::create(path.clone(), header.clone(), events.clone()).unwrap();
    let turn = uuid::Uuid::new_v4().to_string();
    session
        .delta(&turn, &Target::Narration, 1, "已提交", true)
        .unwrap();
    let partial = session.active.as_ref().unwrap().path.clone();
    drop(session);
    use std::io::Write;
    let future = serde_json::json!({"seq":2,"createdAt":format::now(),"kind":"system","code":"historyFork","message":"新版本控制","data":{"version":2}});
    let branched = serde_json::json!({"seq":3,"createdAt":format::now(),"kind":"playerSpeech","playerId":"p","text":"未来分支","branchSeq":2});
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(format::line(&future).unwrap().as_bytes())
        .unwrap();
    file.write_all(format::line(&branched).unwrap().as_bytes())
        .unwrap();
    drop(file);
    let before = std::fs::read(&partial).unwrap();
    let mut reopened = Session::open(path.clone(), events).unwrap();
    assert!(reopened.is_read_only());
    assert!(reopened.needs_recovery());
    assert_eq!(std::fs::read(&partial).unwrap(), before);
    assert_eq!(
        reopened.read(2).unwrap().raw,
        format::line(&future).unwrap()
    );
    assert!(
        reopened
            .append(Body::PlayerSpeech {
                player_id: "p".into(),
                text: "禁止".into(),
                mode: None,
                content_range: None
            })
            .is_err()
    );
    assert!(
        reopened
            .view(None)
            .unwrap()
            .page
            .items
            .iter()
            .all(|item| item.body.is_none() && item.body_ref.is_some())
    );
    drop(reopened);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn recovery_event_preparation_failure_is_not_reported_as_corruption() {
    use crate::record::test_support::Events as FaultEvents;
    let (path, header, _) = setup();
    let events = Arc::new(FaultEvents::default());
    let turn = uuid::Uuid::new_v4().to_string();
    let mut session = Session::create(path.clone(), header, events.clone()).unwrap();
    session
        .delta(&turn, &Target::Narration, 1, "恢复正文", true)
        .unwrap();
    drop(session);
    events.fail_prepare.store(true, Ordering::SeqCst);
    let error = Session::open(path.clone(), events.clone()).err().unwrap();
    assert_eq!(error.code, "app.event-failed");
    events.fail_prepare.store(false, Ordering::SeqCst);
    let reopened = Session::open(path.clone(), events).unwrap();
    assert_eq!(reopened.index.len(), 2);
    assert_eq!(
        body_text(&reopened.read(1).unwrap().body.unwrap().body),
        Some("恢复正文")
    );
    drop(reopened);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn v2_recap_round_trips_and_overlapping_candidates_never_poison_projection() {
    use crate::record::projection::{self, Budget, RecordView, Shape, WorldView};
    let (path, header, events) = setup();
    let mut session = Session::create(path.clone(), header, events.clone()).unwrap();
    for text in ["前文一", "前文二"] {
        session
            .append(Body::Narration {
                text: text.repeat(400),
                turn_id: uuid::Uuid::new_v4().to_string(),
                terminal: Terminal::Completed {
                    finish_reason: aoidos_llm::schedule::FinishReason::Stop,
                },
            })
            .unwrap();
    }
    session
        .append(Body::PlayerSpeech {
            player_id: "p".into(),
            text: "当前行动".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    let source = format!(
        "{}{}",
        session.read(1).unwrap().raw,
        session.read(2).unwrap().raw
    );
    let recap = Body::Recap {
        from_seq: 1,
        through_seq: 2,
        text: "已经发生的事".into(),
        source_hash: format::hash(source.as_bytes()),
        estimator_version: 2,
        origin: format::RecapOrigin::Manual,
    };
    let working = session.projection_working_set().unwrap();
    let plan = projection::project(
        &working.view(),
        &WorldView {
            context: "",
            needs_recovery: false,
        },
        &Budget {
            context_limit: Some(65536),
            tail: 100,
            ..Budget::default()
        },
        &Target::Narration,
        Shape::Chat,
    )
    .unwrap();
    let candidate = session.prepare_recap(&working, &plan, 1, 2).unwrap();
    session
        .commit_recap(
            candidate,
            "已经发生的事".into(),
            format::RecapOrigin::Manual,
        )
        .unwrap();
    assert!(session.append(recap).is_err());
    assert_eq!(session.index.len(), 4);
    session.close().unwrap();
    drop(session);
    let reopened = Session::open(path.clone(), events).unwrap();
    let records = reopened
        .index
        .keys()
        .map(|seq| reopened.read(*seq).unwrap())
        .collect::<Vec<_>>();
    let plan = projection::project(
        &RecordView {
            header: reopened.header(),
            records: &records,
            needs_recovery: false,
            omitted_ranges: &[],
            verified_recaps: &std::collections::BTreeMap::new(),
        },
        &WorldView {
            context: "",
            needs_recovery: false,
        },
        &Budget {
            context_limit: Some(65536),
            ..Budget::default()
        },
        &Target::Narration,
        Shape::ChatPrefix,
    )
    .unwrap();
    assert_eq!(plan.included, vec![3, 4]);
    assert_eq!(plan.estimator_version, 2);
    drop(reopened);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn indexed_file_tampering_unknown_rows_and_duplicate_sequences_are_rejected() {
    use crate::record::test_support::Fixture;
    let mut fixture = Fixture::new();
    let unknown = Body::System {
        code: "futureFact".into(),
        message: "未知".into(),
        related_seq: None,
        turn_id: None,
        data: serde_json::json!({}),
    };
    assert_eq!(
        fixture.session.append(unknown).unwrap_err().code,
        "engine.invalid-phase"
    );
    let body = Body::PlayerSpeech {
        player_id: "p".into(),
        text: "正文".into(),
        mode: None,
        content_range: None,
    };
    let committed = fixture.session.append(body.clone()).unwrap();
    let original = std::fs::read_to_string(&fixture.path).unwrap();
    std::fs::write(&fixture.path, original.replace("正文", "改文")).unwrap();
    assert!(fixture.session.read(committed).is_err());
    std::fs::write(&fixture.path, &original).unwrap();
    fixture.session.close().unwrap();
    let duplicate = format::line(&Record {
        seq: committed,
        created_at: format::now(),
        branch_seq: None,
        body,
    })
    .unwrap();
    std::fs::write(&fixture.path, format!("{original}{duplicate}")).unwrap();
    assert_eq!(
        Session::open(fixture.path.clone(), fixture.events.clone())
            .err()
            .unwrap()
            .code,
        "store.corrupt"
    );
    std::fs::write(&fixture.path, "{\"kind\":\"header\"}\n").unwrap();
    assert!(Session::open(fixture.path.clone(), fixture.events.clone()).is_err());
    let orphan = Body::System {
        code: "orphan".into(),
        message: "诊断".into(),
        related_seq: None,
        turn_id: None,
        data: serde_json::json!({"version":1,"recoveryId":format::hash(b"r")}),
    };
    assert_eq!(body_text(&orphan), None);
}

#[test]
fn incomplete_generated_sources_and_overlapping_recaps_cannot_be_summarized() {
    use crate::record::format::RecapOrigin;
    use crate::record::test_support::{Fixture, candidate};
    let mut fixture = Fixture::new();
    let frozen = candidate(&mut fixture.session);
    fixture
        .session
        .commit_recap(frozen, "已确认摘要".into(), RecapOrigin::Manual)
        .unwrap();
    let source = fixture.session.read(1).unwrap();
    let recap = Record {
        seq: 4,
        created_at: format::now(),
        branch_seq: None,
        body: Body::Recap {
            from_seq: 1,
            through_seq: 1,
            text: "重复".into(),
            source_hash: format::hash(source.raw.as_bytes()),
            estimator_version: 2,
            origin: RecapOrigin::Manual,
        },
    };
    assert!(fixture.session.check_references(&recap).is_err());
    for terminal in [
        Terminal::Cancelled,
        Terminal::Failed {
            error: Fault::new("llm.empty-output", "空"),
            finish_reason: Some(aoidos_llm::schedule::FinishReason::Length),
        },
    ] {
        let mut other = Fixture::new();
        other
            .session
            .append(Body::Narration {
                text: "未完成".into(),
                turn_id: uuid::Uuid::new_v4().to_string(),
                terminal,
            })
            .unwrap();
        let row = other.session.read(1).unwrap();
        let mut recap = recap.clone();
        recap.seq = 2;
        if let Body::Recap { source_hash, .. } = &mut recap.body {
            *source_hash = format::hash(row.raw.as_bytes());
        }
        assert!(other.session.check_references(&recap).is_err());
    }
    let mut other = Fixture::new();
    other
        .session
        .append(Body::PlayerSpeech {
            player_id: "p".into(),
            text: "当前".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    assert!(other.session.check_references(&recap).is_err());
    let turn = uuid::Uuid::new_v4().to_string();
    other
        .session
        .ensure_partial(&turn, &Target::Narration)
        .unwrap();
    assert_eq!(
        other
            .session
            .append_intent(Body::PlayerSpeech {
                player_id: "p".into(),
                text: "拒绝".into(),
                mode: None,
                content_range: None
            })
            .unwrap_err()
            .code,
        "app.busy"
    );
}

#[test]
fn a_generation_turn_can_only_have_one_formal_block_even_after_reopen() {
    let mut fixture = crate::record::test_support::Fixture::new();
    let turn = uuid::Uuid::new_v4().to_string();
    let terminal = Terminal::Completed {
        finish_reason: aoidos_llm::schedule::FinishReason::Stop,
    };
    fixture
        .session
        .append(Body::Narration {
            text: "唯一正式块".into(),
            turn_id: turn.clone(),
            terminal: terminal.clone(),
        })
        .unwrap();
    let duplicate = Body::CharacterSpeech {
        speaker_id: "npc".into(),
        text: "重复块".into(),
        turn_id: turn,
        terminal,
    };
    assert_eq!(
        fixture.session.append(duplicate.clone()).unwrap_err().code,
        "store.corrupt"
    );
    fixture.session.close().unwrap();
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.path)
        .unwrap();
    file.write_all(
        format::line(&Record {
            seq: 2,
            created_at: format::now(),
            branch_seq: None,
            body: duplicate,
        })
        .unwrap()
        .as_bytes(),
    )
    .unwrap();
    drop(file);
    assert_eq!(
        Session::open(fixture.path.clone(), fixture.events.clone())
            .err()
            .unwrap()
            .code,
        "store.corrupt"
    );
}

#[test]
fn recovery_refuses_changed_character_identity_and_reports_marker_commit_failure() {
    let (path, header, events) = setup();
    let mut session = Session::create(path.clone(), header, events.clone()).unwrap();
    let turn = uuid::Uuid::new_v4().to_string();
    session
        .delta(
            &turn,
            &Target::Character("original".into()),
            1,
            "原文",
            true,
        )
        .unwrap();
    let body = Body::CharacterSpeech {
        speaker_id: "other".into(),
        text: "原文".into(),
        turn_id: turn,
        terminal: Terminal::Cancelled,
    };
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(
        format::line(&Record {
            seq: 1,
            created_at: format::now(),
            branch_seq: None,
            body,
        })
        .unwrap()
        .as_bytes(),
    )
    .unwrap();
    drop(file);
    drop(session);
    assert_eq!(
        Session::open(path.clone(), events).err().unwrap().code,
        "store.corrupt"
    );
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    let (path, header, events) = setup();
    let mut session = Session::create(path.clone(), header, events.clone()).unwrap();
    session
        .ensure_partial(&uuid::Uuid::new_v4().to_string(), &Target::Narration)
        .unwrap();
    drop(session);
    events.0.store(MAX_SEQ, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        Session::open(path.clone(), events).err().unwrap().code,
        "app.event-failed"
    );
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn newer_recap_source_hash_excludes_previously_confirmed_summary_rows() {
    use crate::record::{
        format::RecapOrigin,
        test_support::{Fixture, candidate},
    };
    let mut fixture = Fixture::new();
    let frozen = candidate(&mut fixture.session);
    fixture
        .session
        .commit_recap(frozen, "旧摘要".into(), RecapOrigin::Manual)
        .unwrap();
    fixture
        .session
        .append(Body::Narration {
            text: "新正文".into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: Terminal::Completed {
                finish_reason: aoidos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap();
    let raw = format!(
        "{}{}",
        fixture.session.read(2).unwrap().raw,
        fixture.session.read(4).unwrap().raw
    );
    let record = Record {
        seq: 5,
        created_at: format::now(),
        branch_seq: None,
        body: Body::Recap {
            from_seq: 2,
            through_seq: 4,
            text: "新摘要".into(),
            source_hash: format::hash(raw.as_bytes()),
            estimator_version: 2,
            origin: RecapOrigin::Manual,
        },
    };
    fixture.session.check_references(&record).unwrap();
}
