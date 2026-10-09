use super::*;
fn plan() -> CheckPlan {
    freeze_pbta(PlanSpec {
        rule_id: "search",
        actor_id: "player",
        modifiers: vec![],
        branches: ["success".into(), "costly".into(), "failure".into()],
        world_revision: "0",
    })
    .unwrap()
}
#[test]
fn pbta_boundaries_plan_integrity_and_rejection_sampling_are_stable() {
    for (total, result) in [
        (6, CheckResult::Failure),
        (7, CheckResult::CostlySuccess),
        (9, CheckResult::CostlySuccess),
        (10, CheckResult::Success),
    ] {
        assert_eq!(classify(total), result);
    }
    let p = plan();
    validate_plan(&p, "0").unwrap();
    assert!(validate_plan(&p, "1").is_err());
    let mut changed = p.clone();
    changed.expression = "2d6+1".into();
    assert!(validate_plan(&changed, "0").is_err());
    // 同一个采样源实例经历拒绝后成功与硬上限失败，验证失败不返回部分骰子。
    for words in [vec![u32::MAX, 0, 5], vec![0; 1]] {
        let succeeds = words.len() == 3;
        let mut words = words.into_iter();
        let sampled = sample(2, 6, || words.next().unwrap_or(u32::MAX));
        if succeeds {
            let (values, counter) = sampled.unwrap();
            assert_eq!(
                values.iter().map(|r| r.value).collect::<Vec<_>>(),
                vec![1, 6]
            );
            assert_eq!(counter, 3);
        } else {
            assert_eq!(sampled.unwrap_err().code, "engine.invalid-phase");
        }
    }
    let rolled = roll(&p, "0", [0; 32]).unwrap();
    if let Body::Dice {
        rolls, rng, total, ..
    } = rolled
    {
        assert_eq!(rng.start_counter, "0");
        assert_eq!(rng.end_counter, "2");
        assert_eq!(rng.seed, "0".repeat(64));
        // RFC 8439 的零 key / nonce ChaCha20 block 首字：ade0b876 / 903df1a0。
        assert_eq!(
            rolls.iter().map(|r| r.value).collect::<Vec<_>>(),
            vec![1, 1]
        );
        assert_eq!(total, 2);
    } else {
        panic!("expected dice");
    }
    assert!(roll(&p, "1", [0; 32]).is_err());
    assert!(fresh_seed().is_ok());
    assert!(
        freeze_pbta(PlanSpec {
            rule_id: "/",
            actor_id: "player",
            modifiers: vec![],
            branches: ["s".into(), "c".into(), "f".into()],
            world_revision: "0"
        })
        .is_err()
    );
    assert!(
        modifier_total(&[Modifier {
            value: 4,
            source: Source {
                kind: "actor".into(),
                id: "player".into()
            }
        }])
        .is_err()
    );
    assert!(
        modifier_total(&[Modifier {
            value: 1,
            source: Source {
                kind: "/".into(),
                id: "player".into()
            }
        }])
        .is_err()
    );
    assert!(
        entropy_error(getrandom::Error::UNSUPPORTED)
            .message
            .contains("随机源")
    );
    assert!(
        encode_error(aoidos_json::Error::InvalidEncoding)
            .code
            .contains("bad-request")
    );
}
#[test]
fn plan_hash_has_a_fixed_canonical_vector_and_rejects_all_frozen_identity_changes() {
    let mut p = plan();
    p.plan_id = "00000000-0000-0000-0000-000000000001".into();
    p.plan_hash = plan_hash(&p).unwrap();
    assert_eq!(
        p.plan_hash,
        "sha256:f1520672e9faf299a37df2f84edf294644d1da5b94683a998ef65160ad9ee688"
    );
    for mutate in [
        |p: &mut CheckPlan| p.plan_id = "/".into(),
        |p: &mut CheckPlan| p.rule_id = "/".into(),
        |p: &mut CheckPlan| p.actor_id = "/".into(),
        |p: &mut CheckPlan| p.success_branch_id = "/".into(),
        |p: &mut CheckPlan| p.world_revision.clear(),
        |p: &mut CheckPlan| p.rule_version = 2,
        |p: &mut CheckPlan| p.result_policy.dc = Some(8.0),
        |p: &mut CheckPlan| p.result_policy.kind = "unknown".into(),
        |p: &mut CheckPlan| {
            p.modifiers.push(Modifier {
                value: 4,
                source: Source {
                    kind: "actor".into(),
                    id: "player".into(),
                },
            })
        },
    ] {
        let mut changed = p.clone();
        mutate(&mut changed);
        changed.plan_hash = plan_hash(&changed).unwrap();
        assert_eq!(
            validate_plan(&changed, "0").unwrap_err().code,
            "engine.invalid-phase"
        );
    }
    assert!(
        freeze_pbta(PlanSpec {
            rule_id: "search",
            actor_id: "player",
            modifiers: vec![],
            branches: ["/".into(), "c".into(), "f".into()],
            world_revision: "0"
        })
        .is_err()
    );
}
#[test]
fn signed_modifiers_preserve_source_and_total_in_the_frozen_plan_and_roll() {
    for modifier in [-3, 3] {
        let p = freeze_pbta(PlanSpec {
            rule_id: "search",
            actor_id: "player",
            modifiers: vec![Modifier {
                value: modifier,
                source: Source {
                    kind: "actor".into(),
                    id: "player".into(),
                },
            }],
            branches: ["s".into(), "c".into(), "f".into()],
            world_revision: "0",
        })
        .unwrap();
        assert_eq!(p.expression, format!("2d6{modifier:+}"));
        let Body::Dice {
            total, modifiers, ..
        } = roll(&p, "0", [0; 32]).unwrap()
        else {
            panic!("expected dice");
        };
        assert_eq!(total, 2 + modifier);
        assert_eq!(modifiers[0].source.id, "player");
        assert_eq!(modifiers[0].value, modifier);
    }
}
