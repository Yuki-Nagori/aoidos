use super::*;
#[test]
fn strict_json_and_original_bytes_survive_compatibility() {
    for raw in [
        "{\"seq\":1,\"seq\":2}\n",
        "{\"x\":{\"a\":1,\"a\":2}}\n",
        "{}",
        "{} {}\n",
        "[{\"a\":1,\"a\":2}]\n",
        "[1,]\n",
    ] {
        assert!(json(raw).is_err());
    }
    assert!(json("{\"a\":[null,true,false,-1,2,1.5,\"字\"]}\n").is_ok());
    let raw =
        "{ \"kind\":\"future\",\"seq\":1,\"createdAt\":\"2026-10-08T00:00:00Z\",\"future\":42 }\n";
    let parsed = parse(raw).unwrap();
    assert!(parsed.read_only);
    assert_eq!(parsed.raw, raw);
    assert_eq!(
        hash(b"abc"),
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert!(valid_hash(&hash(b"")));
    assert!(!valid_uuid("session-a"));
    let id = uuid::Uuid::new_v4().to_string();
    assert!(valid_uuid(&id));
}
#[test]
fn player_ranges_use_utf8_not_utf16() {
    let mut record = Record {
        seq: 1,
        created_at: now(),
        branch_seq: None,
        body: Body::PlayerSpeech {
            player_id: "p".into(),
            text: "我😀".into(),
            mode: Some(InputMode::OutOfCharacter),
            content_range: Some(ContentRange { start: 3, end: 7 }),
        },
    };
    record.validate().unwrap();
    assert!(!parse(&line(&record).unwrap()).unwrap().read_only);
    if let Body::PlayerSpeech { content_range, .. } = &mut record.body {
        *content_range = Some(ContentRange { start: 1, end: 7 });
    }
    assert!(record.validate().is_err());
}

#[test]
fn oversized_timestamp_cannot_stall_bounded_record_pages() {
    let raw = serde_json::json!({"kind":"future","seq":1,"createdAt":format!("2026-10-08T00:00:00.{}Z","0".repeat(600_000))});
    assert!(parse(&line(&raw).unwrap()).is_err());
    assert!(valid_time("2026-10-08T00:00:00.123456789Z"));
}

#[test]
fn registered_dice_check_and_terminal_schema_reject_corrupt_known_fields() {
    let dice = serde_json::json!({"seq":4,"createdAt":now(),"kind":"dice","expression":"2d6+2","rolls":[{"sides":6,"value":4},{"sides":6,"value":5}],"total":11,"source":{"kind":"rule","id":"r"},"planId":"plan","rng":{"algorithm":"chacha20-v1","mappingVersion":1,"seed":"a".repeat(64),"startCounter":"0","endCounter":"2"},"modifiers":[{"value":2,"source":{"kind":"actor","id":"p"}}]});
    assert!(parse(&line(&dice).unwrap()).is_ok());
    for (pointer, value) in [
        ("/expression", serde_json::json!("bogus")),
        ("/expression", serde_json::json!("1d6+2")),
        ("/total", serde_json::json!(100)),
        ("/source/kind", serde_json::json!("")),
        ("/source/id", serde_json::json!("/")),
        ("/rolls/0/value", serde_json::json!(0)),
        ("/modifiers/0/source/id", serde_json::json!("/")),
        ("/rng/algorithm", serde_json::json!("unknown")),
        ("/rng/mappingVersion", serde_json::json!(2)),
        ("/rng/seed", serde_json::json!("x".repeat(64))),
        ("/rng/startCounter", serde_json::json!("bad")),
        ("/rng/endCounter", serde_json::json!("1")),
    ] {
        let mut bad = dice.clone();
        *bad.pointer_mut(pointer).unwrap() = value;
        assert!(parse(&line(&bad).unwrap()).is_err(), "{pointer}");
    }
    for expression in ["1d20", "3d6-2", "20d1000+100"] {
        assert!(dice_expression(expression).is_some());
    }
    for expression in [
        "d6", "01d6", "1d06", "1d6+0", "21d6", "1d1001", "1d6+101", "1d6-101", "1D6", "1d6++2",
    ] {
        assert!(dice_expression(expression).is_none());
    }
    let check = serde_json::json!({"seq":5,"createdAt":now(),"kind":"check","diceSeq":4,"dc":10.5,"result":"costlySuccess","ruleId":"r","planId":"plan"});
    assert!(parse(&line(&check).unwrap()).is_ok());
    for (key, value) in [
        ("diceSeq", serde_json::json!(5)),
        ("ruleId", serde_json::json!("/")),
        ("dc", serde_json::Value::Null),
        ("result", serde_json::json!("bogus")),
    ] {
        let mut bad = check.clone();
        bad[key] = value;
        assert!(parse(&line(&bad).unwrap()).is_err());
    }
    let narration = serde_json::json!({"seq":6,"createdAt":now(),"kind":"narration","turnId":uuid::Uuid::new_v4().to_string(),"text":"","outcome":"failed","finishReason":"length","error":{"code":"llm.empty-output","message":"空输出"}});
    assert!(parse(&line(&narration).unwrap()).is_ok());
    for (key, value) in [
        ("outcome", serde_json::json!("completed")),
        ("outcome", serde_json::json!("cancelled")),
        ("text", serde_json::json!("\r")),
        ("turnId", serde_json::json!("bad")),
        ("error", serde_json::json!({"code":"","message":"empty"})),
    ] {
        let mut bad = narration.clone();
        bad[key] = value;
        assert!(parse(&line(&bad).unwrap()).is_err());
    }
    let mut bad = narration;
    bad.as_object_mut().unwrap().remove("finishReason");
    assert!(parse(&line(&bad).unwrap()).is_err());
    let unknown =
        serde_json::json!({"seq":7,"createdAt":now(),"kind":"future","error":null,"mode":null});
    assert!(parse(&line(&unknown).unwrap()).unwrap().read_only);
}

#[test]
fn header_common_fields_system_and_recap_limits_are_strict() {
    let mut header = crate::record::test_support::header();
    header.static_prefix_hash = hash(b"wrong");
    assert!(header.validate().is_err());
    let base = serde_json::json!({"seq":1,"kind":"playerSpeech","createdAt":now(),"playerId":"p","text":"x"});
    for (key, value) in [
        ("seq", serde_json::json!(0)),
        ("branchSeq", serde_json::json!(1)),
        ("playerId", serde_json::json!("/")),
        ("text", serde_json::json!("")),
    ] {
        let mut bad = base.clone();
        bad[key] = value;
        assert!(parse(&line(&bad).unwrap()).is_err());
    }
    let bad = serde_json::json!({"seq":2,"kind":"characterSpeech","createdAt":now(),"speakerId":"/","text":"x","turnId":uuid::Uuid::new_v4().to_string(),"outcome":"cancelled"});
    assert!(parse(&line(&bad).unwrap()).is_err());
    let bad = serde_json::json!({"seq":2,"kind":"system","createdAt":now(),"code":"future","message":"x","data":[]});
    assert!(parse(&line(&bad).unwrap()).is_err());
    let recap = Record {
        seq: 2,
        created_at: now(),
        branch_seq: None,
        body: Body::Recap {
            from_seq: 1,
            through_seq: 1,
            text: "".into(),
            source_hash: hash(b"source"),
            estimator_version: 1,
            origin: RecapOrigin::Manual,
        },
    };
    assert!(recap.validate().is_err());
    assert!(line(&"x".repeat(MAX_LINE)).is_err());
}
