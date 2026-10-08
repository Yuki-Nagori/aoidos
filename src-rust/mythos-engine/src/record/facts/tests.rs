use super::*;

fn identity() -> Value {
    serde_json::json!({"version":1,"roundId":uuid::Uuid::new_v4().to_string(),"operationId":uuid::Uuid::new_v4().to_string()})
}
pub(crate) fn plan() -> Value {
    serde_json::json!({"planId":"plan","ruleId":"rule","ruleVersion":1,"actorId":"p","expression":"1d20+2","modifiers":[{"value":2,"source":{"kind":"actor","id":"p"}}],"resultPolicy":{"kind":"dc","dc":10},"successBranchId":"success","costlySuccessBranchId":"costly","failureBranchId":"failure","worldRevision":"0","planHash":format::hash(b"plan")})
}
#[test]
fn registered_facts_keep_typed_values_and_reject_incomplete_identity() {
    let base = identity();
    let definitions = [
        (
            "roundAccepted",
            serde_json::json!({"inputSeq":1,"mode":"inCharacter","diceMode":"manual","profileId":"narration","profileRevision":"revision","target":{"kind":"characterSpeech","speakerId":"keeper"},"sceneId":"demo"}),
        ),
        (
            "checkPlanned",
            serde_json::json!({"diceMode":"auto","plan":plan()}),
        ),
        (
            "checkSkipped",
            serde_json::json!({"reason":"outOfCharacter","worldRevision":"0"}),
        ),
        (
            "settlementPlanned",
            serde_json::json!({"narrativeSeq":6,"worldRevision":"0","mutations":[]}),
        ),
        (
            "roundSettled",
            serde_json::json!({"narrativeSeq":6,"settlementSeq":7}),
        ),
        (
            "sceneAdvanced",
            serde_json::json!({"previousSceneId":"previous","position":{"sceneId":"scene","path":[{"kind":"scene","id":"scene","title":"场景"}]},"worldRevision":"0","mutationId":uuid::Uuid::new_v4().to_string()}),
        ),
        (
            "sceneStayed",
            serde_json::json!({"sceneId":"scene","worldRevision":"0"}),
        ),
        (
            "sessionEnded",
            serde_json::json!({"previousSceneId":"scene","reasonId":"finished","worldRevision":"0","mutationId":uuid::Uuid::new_v4().to_string()}),
        ),
        (
            "roundEnded",
            serde_json::json!({"outcome":"completed","throughSeq":9,"completedSteps":["proposal","check","narration","settlement","advance"]}),
        ),
        (
            "abandonCheckpoint",
            serde_json::json!({"sourceRoundId":uuid::Uuid::new_v4().to_string(),"checkpointSeq":10}),
        ),
    ];
    for (code, fields) in definitions {
        let mut data = base.clone();
        data.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        assert!(Fact::decode(code, &data).is_ok(), "{code}");
        for key in ["operationId", "roundId"] {
            let mut bad = data.clone();
            bad[key] = serde_json::json!("bad");
            assert!(Fact::decode(code, &bad).is_err());
        }
        let mut bad = data.clone();
        bad["version"] = serde_json::json!(2);
        assert!(Fact::decode(code, &bad).is_err());
        if code == "roundAccepted" {
            data["sourceRoundId"] = serde_json::json!(uuid::Uuid::new_v4().to_string());
            assert!(Fact::decode(code, &data).is_err());
        }
        if code == "roundEnded" {
            data["outcome"] = serde_json::json!("failed");
            assert!(Fact::decode(code, &data).is_err());
        }
        if code == "checkPlanned" {
            data["plan"]["planId"] = serde_json::json!("/");
            assert!(Fact::decode(code, &data).is_err());
        }
        if code == "settlementPlanned" {
            let item = serde_json::json!({"mutationId":uuid::Uuid::new_v4().to_string(),"data":{"version":1,"kind":"fixture","payload":{}}});
            data["mutations"] = serde_json::json!([item.clone(), item]);
            assert!(Fact::decode(code, &data).is_err());
        }
    }
    assert!(Fact::decode("unknown", &base).is_err());
    assert!(
        Fact::decode(
            "orphan",
            &serde_json::json!({"version":1,"recoveryId":"bad"})
        )
        .is_err()
    );
    assert!(Fact::decode("historyFork",&serde_json::json!({"version":1,"operationId":uuid::Uuid::new_v4().to_string(),"mode":"rewind","parentControlSeq":0,"targetSeq":1,"prefixHash":"bad"})).is_err());
}
