use super::*;

fn policy() -> Policy {
    Policy {
        version: 1,
        algorithm_version: 1,
        config_version: 0,
        app_config_version: 0,
        algorithm: Algorithm::Exponential,
        base_strength: 100_000,
        reinforcement_gain: 50_000,
        reinforcement_cap: 400_000,
        strength_cap: 1_000_000,
        half_life_rounds: 100,
        repeat_window_rounds: 5,
        repeat_limit: 2,
        revival_cooldown_rounds: 10,
        active_capacity: 100,
        rate_table_version: 1,
        decay_rate: 100_000,
    }
}
fn source() -> SourceRef {
    SourceRef {
        script_id: "mistbell".into(),
        run_id: new_id(),
        session_id: new_id(),
        record_seq: 1,
        body_hash: hash("雾钟".as_bytes()),
        start_byte: 0,
        end_byte: 6,
        mode: Some(InputMode::InCharacter),
    }
}
fn version() -> Version {
    let sources = vec![source()];
    Version {
        version: 1,
        entry_id: new_id(),
        version_id: new_id(),
        parent_version_id: None,
        restored_from_version_id: None,
        subject: Subject::PlayerPreference,
        dimension: Dimension::PlayerPreference,
        summary: "喜欢探索".into(),
        attitude: Attitude::Believed,
        inferred: false,
        change: ChangeKind::New,
        source_set_hash: set_hash(&sources).unwrap(),
        evidence_set_hash: set_hash::<KnowledgeEvidence>(&[]).unwrap(),
        sources,
        evidence: vec![],
        policy_hash: hash(b"policy"),
    }
}
fn character() -> Version {
    let mut v = version();
    v.subject = Subject::Character { id: "guard".into() };
    v.dimension = Dimension::Story;
    v.evidence = vec![KnowledgeEvidence {
        evidence_id: new_id(),
        subject_id: "guard".into(),
        source: v.sources[0].clone(),
        kind: KnowledgeKind::Observed,
        informant_id: None,
        scope: "scene".into(),
        rule_id: "observed".into(),
    }];
    refresh(&mut v);
    v
}
fn refresh(v: &mut Version) {
    v.source_set_hash = set_hash(&v.sources).unwrap();
    v.evidence_set_hash = set_hash(&v.evidence).unwrap();
}
fn rejected(result: Result<()>) {
    assert!(matches!(
        result,
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
}

#[test]
fn policy_rejects_each_invalid_parameter_and_keeps_inclusive_bounds() {
    assert!(policy().validate().is_ok());
    for field in ["version", "algorithmVersion", "rateTableVersion"] {
        let mut value = aoidos_json::to_value(policy()).unwrap();
        value[field] = serde_json::json!(2);
        let p: Policy = aoidos_json::from_value(value).unwrap();
        assert!(matches!(
            p.validate(),
            Err(Error::Rejected(Reason::PolicyUnavailable))
        ));
    }
    for (field, values) in [
        ("configVersion", vec![MAX_INTEGER + 1]),
        ("appConfigVersion", vec![MAX_INTEGER + 1]),
        ("baseStrength", vec![49_999, 1_000_001]),
        ("reinforcementGain", vec![500_001]),
        ("reinforcementCap", vec![950_001]),
        ("strengthCap", vec![99_999, 1_000_001]),
        ("halfLifeRounds", vec![9, 10_001]),
        ("repeatWindowRounds", vec![0, 10_001]),
        ("repeatLimit", vec![0, 11]),
        ("revivalCooldownRounds", vec![4, 10_001]),
        ("activeCapacity", vec![99, 10_001]),
        ("decayRate", vec![0, 1_000_001]),
    ] {
        for invalid in values {
            let mut value = aoidos_json::to_value(policy()).unwrap();
            value[field] = serde_json::json!(invalid);
            let p: Policy = aoidos_json::from_value(value).unwrap();
            rejected(p.validate());
        }
    }
    let mut p = policy();
    p.strength_cap = 100_000;
    rejected(p.validate());
    p.reinforcement_cap = 0;
    assert!(p.validate().is_ok());
    p.base_strength = 50_000;
    p.reinforcement_cap = 950_000;
    p.strength_cap = 1_000_000;
    p.reinforcement_gain = 500_000;
    p.config_version = MAX_INTEGER;
    p.app_config_version = MAX_INTEGER;
    p.half_life_rounds = 10_000;
    p.repeat_window_rounds = 10_000;
    p.revival_cooldown_rounds = 10_000;
    p.repeat_limit = 10;
    p.active_capacity = 10_000;
    p.decay_rate = 1_000_000;
    assert!(p.validate().is_ok());
}

#[test]
fn source_identity_and_byte_ranges_are_bounded() {
    assert!(source().validate().is_ok());
    for invalid in ["", "../mistbell", "李雷", &"a".repeat(65)] {
        assert!(!valid_name(invalid));
    }
    assert!(valid_name("NPC_1-a"));
    let id = new_id();
    assert!(valid_id(&id));
    assert!(!valid_id("ABCDEF00-1234-4567-89AB-0123456789AB"));
    assert!(!valid_id("not-a-uuid"));
    for mutation in [
        |s: &mut SourceRef| s.script_id.clear(),
        |s: &mut SourceRef| s.run_id.clear(),
        |s: &mut SourceRef| s.session_id.clear(),
        |s: &mut SourceRef| s.record_seq = 0,
        |s: &mut SourceRef| s.record_seq = MAX_INTEGER + 1,
        |s: &mut SourceRef| s.start_byte = s.end_byte,
        |s: &mut SourceRef| s.end_byte = MAX_INTEGER + 1,
    ] {
        let mut s = source();
        mutation(&mut s);
        rejected(s.validate());
    }
    let mut s = source();
    s.record_seq = MAX_INTEGER;
    s.end_byte = MAX_INTEGER;
    assert!(s.validate().is_ok());
}

#[test]
fn version_rejects_bad_identity_restore_relationships_and_summary() {
    assert!(version().validate().is_ok());
    let mut future = version();
    future.version = 2;
    assert!(matches!(
        future.validate(),
        Err(Error::Rejected(Reason::PolicyUnavailable))
    ));
    for mutation in [
        |v: &mut Version| v.entry_id.clear(),
        |v: &mut Version| v.version_id.clear(),
        |v: &mut Version| v.parent_version_id = Some("bad".into()),
        |v: &mut Version| v.parent_version_id = Some(v.version_id.clone()),
        |v: &mut Version| v.restored_from_version_id = Some("bad".into()),
        |v: &mut Version| v.restored_from_version_id = Some(new_id()),
        |v: &mut Version| v.change = ChangeKind::Restore,
        |v: &mut Version| v.summary = " \t\n".into(),
        |v: &mut Version| v.summary = "a".repeat(4097),
        |v: &mut Version| v.sources.clear(),
        |v: &mut Version| v.sources = vec![v.sources[0].clone(); 129],
        |v: &mut Version| v.source_set_hash = hash(b"wrong"),
        |v: &mut Version| v.evidence_set_hash = hash(b"wrong"),
    ] {
        let mut v = version();
        mutation(&mut v);
        rejected(v.validate());
    }
    let mut v = version();
    v.parent_version_id = Some(new_id());
    v.restored_from_version_id = Some(new_id());
    v.change = ChangeKind::Restore;
    v.summary = "a".repeat(4096);
    assert!(v.validate().is_ok());
    v.sources.push(v.sources[0].clone());
    refresh(&mut v);
    rejected(v.validate());
    let mut v = version();
    v.sources[0].record_seq = 0;
    refresh(&mut v);
    rejected(v.validate());
}

#[test]
fn character_knowledge_requires_same_subject_and_trusted_source() {
    assert!(character().validate().is_ok());
    for mutation in [
        |v: &mut Version| {
            v.subject = Subject::Character {
                id: "bad/name".into(),
            }
        },
        |v: &mut Version| v.dimension = Dimension::PlayerPreference,
        |v: &mut Version| v.evidence.clear(),
        |v: &mut Version| v.evidence = vec![v.evidence[0].clone(); 129],
        |v: &mut Version| v.evidence[0].evidence_id.clear(),
        |v: &mut Version| v.evidence[0].subject_id = "other".into(),
        |v: &mut Version| v.evidence[0].source.record_seq += 1,
        |v: &mut Version| v.evidence.push(v.evidence[0].clone()),
        |v: &mut Version| v.evidence[0].scope.clear(),
        |v: &mut Version| v.evidence[0].rule_id.clear(),
        |v: &mut Version| v.evidence[0].informant_id = Some("bad/name".into()),
        |v: &mut Version| v.evidence[0].kind = KnowledgeKind::Told,
        |v: &mut Version| v.subject = Subject::PlayerPreference,
    ] {
        let mut v = character();
        mutation(&mut v);
        refresh(&mut v);
        rejected(v.validate());
    }
    let mut v = character();
    v.evidence[0].kind = KnowledgeKind::Told;
    v.evidence[0].informant_id = Some("guide".into());
    refresh(&mut v);
    assert!(v.validate().is_ok());
    let mut v = version();
    v.dimension = Dimension::Story;
    rejected(v.validate());
}

#[test]
fn immutable_version_size_includes_sources_and_evidence() {
    let mut v = version();
    for seq in 2..=128 {
        let mut s = v.sources[0].clone();
        s.record_seq = seq;
        v.sources.push(s);
    }
    refresh(&mut v);
    assert!(canonical(&v).unwrap().len() > MAX_VERSION_BYTES);
    rejected(v.validate());
}

#[test]
fn canonical_sets_ignore_order_and_serialization_errors_are_sanitized() {
    assert_eq!(
        set_hash(&["a", "b"]).unwrap(),
        set_hash(&["b", "a"]).unwrap()
    );
    assert_ne!(set_hash(&["a"]).unwrap(), set_hash(&["a", "a"]).unwrap());
    struct Broken;
    impl Serialize for Broken {
        fn serialize<S: serde::Serializer>(&self, _: S) -> std::result::Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("private input"))
        }
    }
    assert!(matches!(canonical(&Broken), Err(Error::Corrupt)));
    assert!(matches!(set_hash(&[Broken]), Err(Error::Corrupt)));
}

#[test]
fn persisted_types_reject_unknown_fields_and_round_trip_canonical_values() {
    let original = version();
    let decoded: Version =
        aoidos_json::from_value(aoidos_json::to_value(&original).unwrap()).unwrap();
    assert_eq!(decoded, original);
    for mut value in [
        aoidos_json::to_value(policy()).unwrap(),
        aoidos_json::to_value(source()).unwrap(),
        aoidos_json::to_value(version()).unwrap(),
    ] {
        value["futureField"] = serde_json::json!(true);
        let bytes = aoidos_json::canonical_string(&value).unwrap();
        if value.get("algorithm").is_some() {
            assert!(aoidos_json::decode::<Policy>(&bytes, bytes.len()).is_err());
        } else if value.get("recordSeq").is_some() {
            assert!(aoidos_json::decode::<SourceRef>(&bytes, bytes.len()).is_err());
        } else {
            assert!(aoidos_json::decode::<Version>(&bytes, bytes.len()).is_err());
        }
    }
}
