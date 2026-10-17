use super::*;

fn revision() -> String {
    format::hash(b"script").to_owned()
}

fn character(id: &str, scope: &str, name: &str) -> CanonicalEntity {
    CanonicalEntity {
        id: id.into(),
        kind: EntityKind::Character,
        scope: scope.into(),
        canonical_name: name.into(),
    }
}

#[test]
fn complete_directory_resolves_only_exact_names_within_kind_and_scope() {
    let directory = EntityDirectory::new(
        revision(),
        1,
        true,
        vec![
            character("player", "global", "玩家"),
            character("npc-li-lei", "global", "李雷"),
            CanonicalEntity {
                id: "east-gate".into(),
                kind: EntityKind::Place,
                scope: "mistbell".into(),
                canonical_name: "东门".into(),
            },
            CanonicalEntity {
                id: "east-gate-river".into(),
                kind: EntityKind::Place,
                scope: "river-town".into(),
                canonical_name: "东门".into(),
            },
        ],
    )
    .unwrap();

    assert_eq!(directory.revision(), 1);
    assert_eq!(
        directory
            .resolve_exact(EntityKind::Character, "global", "李雷")
            .unwrap()
            .unwrap()
            .id,
        "npc-li-lei"
    );
    assert!(
        directory
            .resolve_exact(EntityKind::Character, "global", "李磊")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        directory
            .resolve_exact(EntityKind::Place, "river-town", "东门")
            .unwrap()
            .unwrap()
            .id,
        "east-gate-river"
    );
    assert!(
        directory
            .resolve_exact(EntityKind::Place, "unknown-town", "东门")
            .unwrap()
            .is_none()
    );
}

#[test]
fn incomplete_snapshot_never_resolves_a_locally_unique_name() {
    let directory = EntityDirectory::new(
        revision(),
        2,
        false,
        vec![character("npc-li-lei", "global", "李雷")],
    )
    .unwrap();

    assert!(!directory.is_complete());
    assert!(
        directory
            .resolve_exact(EntityKind::Character, "global", "李雷")
            .is_err()
    );
}

#[test]
fn exact_resolution_rejects_invalid_scope_and_name() {
    let directory = EntityDirectory::new(
        revision(),
        1,
        true,
        vec![character("npc-li-lei", "global", "李雷")],
    )
    .unwrap();

    for (scope, name) in [
        ("", "李雷"),
        (" global", "李雷"),
        ("global", ""),
        ("global", " 李雷"),
        ("global", "李\n雷"),
        ("x".repeat(MAX_SCOPE_BYTES + 1).as_str(), "李雷"),
        ("global", "x".repeat(MAX_NAME_BYTES + 1).as_str()),
    ] {
        assert!(
            directory
                .resolve_exact(EntityKind::Character, scope, name)
                .is_err()
        );
    }
}

#[test]
fn malformed_or_conflicting_snapshots_are_rejected_as_a_whole() {
    let invalid = [
        vec![character("bad/id", "global", "李雷")],
        vec![
            character("same", "global", "李雷"),
            character("same", "global", "小李"),
        ],
        vec![
            character("one", "global", "李雷"),
            character("two", "global", "李雷"),
        ],
        vec![character("one", "global", "\0")],
        vec![character("one", " ", "李雷")],
    ];
    for entries in invalid {
        assert!(EntityDirectory::new(revision(), 1, true, entries).is_err());
    }
    assert!(
        EntityDirectory::new(
            revision(),
            0,
            true,
            vec![character("one", "global", "李雷")]
        )
        .is_err()
    );
    assert!(
        EntityDirectory::new(
            revision(),
            1_u64 << 53,
            true,
            vec![character("one", "global", "李雷")]
        )
        .is_err()
    );
    assert!(
        EntityDirectory::new(
            "unknown".into(),
            1,
            true,
            vec![character("one", "global", "李雷")]
        )
        .is_err()
    );
}

#[test]
fn directory_larger_than_one_mib_is_rejected() {
    let entries = (0..MAX_ENTITIES)
        .map(|index| {
            character(
                &format!("entity-{index}"),
                "global",
                &format!("{}-{index:04}", "名".repeat(80)),
            )
        })
        .collect();

    assert!(EntityDirectory::new(revision(), 1, true, entries).is_err());
}

#[test]
fn encoded_length_stops_at_the_first_over_limit_entity() {
    let first = CanonicalEntity {
        id: "first".into(),
        kind: EntityKind::Character,
        scope: "global".into(),
        canonical_name: "x".repeat(MAX_DIRECTORY_BYTES),
    };
    let later = character("later", "global", "must-not-be-counted");
    let by_id = BTreeMap::from([(first.id.clone(), first.clone()), (later.id.clone(), later)]);
    let directory = EntityDirectory {
        script_revision: revision(),
        revision: 1,
        complete: true,
        snapshot_session_id: None,
        snapshot_history_revision: None,
        by_id,
        by_name: BTreeMap::new(),
    };

    let expected = directory.script_revision.len()
        + 32
        + first.id.len()
        + first.scope.len()
        + first.canonical_name.len()
        + 16;
    assert_eq!(directory.encoded_len().unwrap(), expected);
    assert!(expected > MAX_DIRECTORY_BYTES);
}

#[test]
fn creation_rules_and_dynamic_merge_reject_invalid_boundaries() {
    for (id, scope, limit) in [
        ("bad id", "global", 1),
        ("rule", "", 1),
        ("rule", "global", 0),
        ("rule", "global", MAX_ENTITIES + 1),
    ] {
        assert!(
            EntityCreationRule::new(id.into(), EntityKind::Character, scope.into(), limit).is_err()
        );
    }
    let directory = EntityDirectory::new(
        revision(),
        2,
        true,
        vec![character("player", "global", "玩家")],
    )
    .unwrap();
    assert!(directory.with_dynamic(1, vec![]).is_err());
}

#[test]
fn stable_identity_lookup_requires_complete_directory() {
    let entity = character("npc", "demo", "角色");
    let directory = EntityDirectory::new(revision(), 1, true, vec![entity.clone()]).unwrap();
    assert_eq!(directory.entity("npc").unwrap(), Some(&entity));
    assert_eq!(directory.entity("missing").unwrap(), None);
    let incomplete = EntityDirectory::new(revision(), 1, false, vec![entity]).unwrap();
    assert!(incomplete.entity("npc").is_err());
}
