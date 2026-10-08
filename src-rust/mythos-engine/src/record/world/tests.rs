use super::*;
use crate::{
    ports::Terminal,
    record::{
        facts::{MutationData, WorldMutation},
        test_support::Fixture,
    },
};

#[test]
fn registered_operation_boundary_rejects_unknown_scene_and_reuses_applied_mutations() {
    let mut fixture = Fixture::new();
    let mut port = port();
    fixture
        .session
        .append(Body::Narration {
            text: "确认正文".into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: Terminal::Completed {
                finish_reason: mythos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap();
    let once = mutation(0);
    fixture
        .session
        .commit_world(planned(1, vec![once.clone()]), &mut port)
        .unwrap();
    fixture
        .session
        .commit_world(planned(1, vec![once]), &mut port)
        .unwrap();
    assert_eq!(
        port.connection
            .query_row("SELECT value FROM fixture", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let base = serde_json::json!({"version":1,"roundId":uuid::Uuid::new_v4().to_string(),"operationId":uuid::Uuid::new_v4().to_string(),"previousSceneId":"demo","worldRevision":"1","mutationId":uuid::Uuid::new_v4().to_string(),"position":{"sceneId":"next","path":[]},"reasonId":"finished"});
    for code in ["sceneAdvanced", "sessionEnded"] {
        let body = Body::System {
            code: code.into(),
            message: "变更意图".into(),
            related_seq: None,
            turn_id: None,
            data: base.clone(),
        };
        assert_eq!(
            fixture
                .session
                .commit_world(body, &mut port)
                .unwrap_err()
                .code,
            "engine.invalid-phase"
        );
    }
    let orphan = Body::System {
        code: "orphan".into(),
        message: "诊断".into(),
        related_seq: None,
        turn_id: None,
        data: serde_json::json!({"version":1,"recoveryId":format::hash(b"recovery")}),
    };
    assert!(fixture.session.commit_world(orphan, &mut port).is_err());
    let mut another = Fixture::new();
    another
        .session
        .append(Body::PlayerSpeech {
            player_id: "p".into(),
            text: "输入".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    assert!(
        another
            .session
            .commit_world(planned(1, vec![]), &mut port)
            .is_err()
    );
}
struct SqlWorld {
    connection: rusqlite::Connection,
    registered: bool,
    fail: bool,
}
impl WorldPort for SqlWorld {
    fn registered(&self, kind: &str, version: u32) -> bool {
        self.registered && kind == "fixture-increment" && version == 1
    }
    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault> {
        mythos_store::applied::contains(&self.connection, id, hash).map_err(store_fault)
    }
    fn apply(&mut self, mutation: &WorldMutation, hash: &str) -> Result<(), Fault> {
        if self.fail {
            return Err(Fault::new("store.io", "合成 SQL 故障"));
        }
        let expected = mutation.data.payload["expected"].as_i64().unwrap();
        mythos_store::applied::apply_once(
            &mut self.connection,
            &mutation.mutation_id,
            hash,
            &mut |tx| {
                if tx
                    .execute(
                        "UPDATE fixture SET value=value+1 WHERE value=?1",
                        [expected],
                    )
                    .map_err(mythos_store::db::sqlite_error)?
                    != 1
                {
                    return Err(format::corrupt());
                }
                Ok(())
            },
        )
        .map_err(store_fault)?;
        Ok(())
    }
}
fn port() -> SqlWorld {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection
        .execute_batch(mythos_store::applied::SCHEMA)
        .unwrap();
    connection
        .execute_batch(
            "CREATE TABLE fixture(value INTEGER NOT NULL);INSERT INTO fixture VALUES(0);",
        )
        .unwrap();
    SqlWorld {
        connection,
        registered: true,
        fail: false,
    }
}
fn planned(narrative_seq: u64, mutations: Vec<WorldMutation>) -> Body {
    Body::System {
        code: "settlementPlanned".into(),
        message: "变更意图，尚未确认成功".into(),
        related_seq: None,
        turn_id: None,
        data: serde_json::json!({"version":1,"roundId":uuid::Uuid::new_v4().to_string(),"operationId":uuid::Uuid::new_v4().to_string(),"narrativeSeq":narrative_seq,"worldRevision":"0","mutations":mutations}),
    }
}
fn mutation(expected: i64) -> WorldMutation {
    WorldMutation {
        mutation_id: uuid::Uuid::new_v4().to_string(),
        data: MutationData {
            version: 1,
            kind: "fixture-increment".into(),
            payload: serde_json::json!({"expected":expected}),
        },
    }
}
#[test]
fn sql_failure_keeps_pending_intent_and_recovery_applies_once() {
    let mut fixture = Fixture::new();
    fixture
        .session
        .append(Body::Narration {
            text: "确认正文".into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: Terminal::Completed {
                finish_reason: mythos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap();
    let mut port = port();
    let first = mutation(0);
    let second = mutation(1);
    let body = planned(1, vec![first.clone(), second.clone()]);
    port.fail = true;
    assert!(
        fixture
            .session
            .commit_world(body.clone(), &mut port)
            .is_err()
    );
    assert!(fixture.session.needs_recovery);
    assert_eq!(fixture.session.last_event_seq, 1);
    assert!(
        fixture
            .session
            .append(Body::PlayerSpeech {
                player_id: "p".into(),
                text: "后文".into(),
                mode: None,
                content_range: None
            })
            .is_err()
    );
    port.fail = false;
    fixture.session.recover_world(&mut port).unwrap();
    assert!(!fixture.session.needs_recovery);
    assert_eq!(fixture.session.last_event_seq, 2);
    fixture.session.recover_world(&mut port).unwrap();
    let value: i64 = port
        .connection
        .query_row("SELECT value FROM fixture", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, 2);
    port.registered = false;
    assert!(
        fixture
            .session
            .commit_world(planned(1, vec![mutation(2)]), &mut port)
            .is_err()
    );
    assert_eq!(fixture.session.index.len(), 2);
    assert!(fixture.session.append(body).is_err());
}
#[test]
fn applied_content_conflict_and_unregistered_recovery_stop_without_replaying() {
    let mut fixture = Fixture::new();
    fixture
        .session
        .append(Body::Narration {
            text: "确认正文".into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: Terminal::Completed {
                finish_reason: mythos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap();
    let mut port = port();
    port.fail = true;
    let mutation = mutation(0);
    assert!(
        fixture
            .session
            .commit_world(planned(1, vec![mutation.clone()]), &mut port)
            .is_err()
    );
    port.fail = false;
    port.registered = false;
    assert!(fixture.session.recover_world(&mut port).is_err());
    assert!(fixture.session.needs_recovery);
    port.registered = true;
    port.connection
        .execute(
            "INSERT INTO store_applied(id,content_hash) VALUES(?1,'wrong')",
            [&mutation.mutation_id],
        )
        .unwrap();
    assert!(fixture.session.recover_world(&mut port).is_err());
    assert!(fixture.session.needs_recovery);
    fixture.session.pending_history = true;
    assert!(fixture.session.recover_world(&mut port).is_err());
    assert!(
        mutations(&Body::PlayerSpeech {
            player_id: "p".into(),
            text: "x".into(),
            mode: None,
            content_range: None
        })
        .is_err()
    );
}

#[test]
fn complete_registered_round_keeps_plan_dice_check_and_settlement_references() {
    let mut fixture = Fixture::new();
    let mut port = port();
    let round = uuid::Uuid::new_v4().to_string();
    let system = |code: &str, fields: serde_json::Value| {
        let mut data = serde_json::json!({"version":1,"roundId":round,"operationId":uuid::Uuid::new_v4().to_string()});
        data.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        Body::System {
            code: code.into(),
            message: "因果事实".into(),
            related_seq: None,
            turn_id: None,
            data,
        }
    };
    fixture
        .session
        .append(Body::PlayerSpeech {
            player_id: "p".into(),
            text: "当前输入".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    fixture.session.append(system("roundAccepted",serde_json::json!({"inputSeq":1,"mode":"inCharacter","diceMode":"manual","profileId":"narration","profileRevision":"revision","target":{"kind":"narration"},"sceneId":"demo"}))).unwrap();
    let plan = serde_json::json!({"planId":"plan","ruleId":"rule","ruleVersion":1,"actorId":"p","expression":"1d20+2","modifiers":[{"value":2,"source":{"kind":"actor","id":"p"}}],"resultPolicy":{"kind":"dc","dc":10},"successBranchId":"success","costlySuccessBranchId":"costly","failureBranchId":"failure","worldRevision":"0","planHash":format::hash(b"plan")});
    fixture
        .session
        .append(system(
            "checkPlanned",
            serde_json::json!({"diceMode":"manual","plan":plan}),
        ))
        .unwrap();
    let mismatch = serde_json::json!({"kind":"dice","seq":4,"createdAt":format::now(),"expression":"1d6+2","rolls":[{"sides":6,"value":4}],"total":6,"source":{"kind":"rule","id":"rule"},"planId":"plan","rng":{"algorithm":"chacha20-v1","mappingVersion":1,"seed":"1".repeat(64),"startCounter":"0","endCounter":"1"},"modifiers":[{"value":2,"source":{"kind":"actor","id":"p"}}]});
    let parsed = format::parse(&format::line(&mismatch).unwrap())
        .unwrap()
        .body
        .unwrap();
    assert!(fixture.session.check_references(&parsed).is_err());
    let mut missing = parsed.clone();
    if let Body::Dice { plan_id, .. } = &mut missing.body {
        *plan_id = "missing".into();
    }
    assert!(fixture.session.check_references(&missing).is_err());
    let dice = serde_json::json!({"kind":"dice","seq":4,"createdAt":format::now(),"expression":"1d20+2","rolls":[{"sides":20,"value":13}],"total":15,"source":{"kind":"rule","id":"rule"},"planId":"plan","rng":{"algorithm":"chacha20-v1","mappingVersion":1,"seed":"1".repeat(64),"startCounter":"0","endCounter":"1"},"modifiers":[{"value":2,"source":{"kind":"actor","id":"p"}}]});
    fixture
        .session
        .append(
            format::parse(&format::line(&dice).unwrap())
                .unwrap()
                .body
                .unwrap()
                .body,
        )
        .unwrap();
    fixture
        .session
        .append(Body::Check {
            dice_seq: 4,
            dc: Some(10.0),
            result: format::CheckResult::CostlySuccess,
            rule_id: "rule".into(),
            plan_id: "plan".into(),
        })
        .unwrap();
    fixture
        .session
        .append(Body::Narration {
            text: "确认正文".into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: Terminal::Completed {
                finish_reason: mythos_llm::schedule::FinishReason::Stop,
            },
        })
        .unwrap();
    fixture
        .session
        .commit_world(
            system(
                "settlementPlanned",
                serde_json::json!({"narrativeSeq":6,"worldRevision":"0","mutations":[mutation(0)]}),
            ),
            &mut port,
        )
        .unwrap();
    fixture
        .session
        .append(system(
            "roundSettled",
            serde_json::json!({"narrativeSeq":6,"settlementSeq":7}),
        ))
        .unwrap();
    fixture
        .session
        .append(system(
            "sceneStayed",
            serde_json::json!({"sceneId":"demo","worldRevision":"1"}),
        ))
        .unwrap();
    fixture.session.append(system("roundEnded",serde_json::json!({"outcome":"completed","throughSeq":9,"completedSteps":["proposal","check","narration","settlement","advance"]}))).unwrap();
    fixture
        .session
        .append(system(
            "abandonCheckpoint",
            serde_json::json!({"sourceRoundId":round,"checkpointSeq":10}),
        ))
        .unwrap();
    let invalid_end = format::Record {
        seq: 99,
        created_at: format::now(),
        branch_seq: None,
        body: system(
            "roundEnded",
            serde_json::json!({"outcome":"completed","throughSeq":100,"completedSteps":["proposal","check","narration","settlement","advance"]}),
        ),
    };
    assert!(fixture.session.check_references(&invalid_end).is_err());
    let invalid_checkpoint = format::Record {
        seq: 99,
        created_at: format::now(),
        branch_seq: None,
        body: system(
            "abandonCheckpoint",
            serde_json::json!({"sourceRoundId":round,"checkpointSeq":100}),
        ),
    };
    assert!(
        fixture
            .session
            .check_references(&invalid_checkpoint)
            .is_err()
    );
    assert!(
        fixture
            .session
            .append(Body::Check {
                dice_seq: 1,
                dc: Some(10.0),
                result: format::CheckResult::Success,
                rule_id: "rule".into(),
                plan_id: "plan".into()
            })
            .is_err()
    );
    assert!(fixture.session.append(system("roundAccepted",serde_json::json!({"inputSeq":6,"mode":"inCharacter","diceMode":"manual","profileId":"narration","profileRevision":"revision","target":{"kind":"narration"},"sceneId":"demo"}))).is_err());
    assert!(
        fixture
            .session
            .append(system(
                "roundSettled",
                serde_json::json!({"narrativeSeq":999,"settlementSeq":7})
            ))
            .is_err()
    );
    assert!(
        fixture
            .session
            .append(system(
                "roundEnded",
                serde_json::json!({"outcome":"completed","throughSeq":999,"completedSteps":[]})
            ))
            .is_err()
    );
    fixture.session.close().unwrap();
    let mut reopened =
        crate::record::session::Session::open(fixture.path.clone(), fixture.events.clone())
            .unwrap();
    assert!(reopened.needs_recovery());
    reopened.recover_world(&mut port).unwrap();
    assert!(!reopened.needs_recovery());
    assert_eq!(reopened.read(5).unwrap().kind, "check");
    assert_eq!(
        port.connection
            .query_row("SELECT value FROM fixture", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    reopened.read_only = true;
    assert!(reopened.recover_world(&mut port).is_err());
}
