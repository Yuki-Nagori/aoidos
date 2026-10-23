use super::*;

#[test]
fn default_original_and_attributions_are_available_without_runtime_files() {
    let original = scenario("mistbell").unwrap();
    assert_eq!(original.title, "雾钟地窖");
    assert_eq!(original.body.as_bytes(), BUNDLED[0].body);
    assert_ne!(
        revision("mistbell").unwrap(),
        crate::record::format::hash(original.body.as_bytes())
    );
    let entities = entity_directory("mistbell").unwrap();
    assert!(entities.is_complete());
    assert_eq!(entities.revision(), 1);
    assert_eq!(
        entities
            .resolve_exact(
                super::super::catalog::EntityKind::Character,
                "mistbell",
                "洛苇"
            )
            .unwrap()
            .unwrap()
            .id,
        "luo-wei"
    );
    assert!(
        entities
            .resolve_exact(super::super::catalog::EntityKind::Term, "mistbell", "龙")
            .unwrap()
            .is_none()
    );
    let scripts = list().unwrap();
    assert_eq!(scripts.len(), 1);
    assert_eq!(scripts[0].script_id, original.script_id);
    assert_eq!(scripts[0].title, "雾钟地窖");
    assert_eq!(scripts[0].display_names["zh-Hans"], "雾钟地窖");
    assert_eq!(scripts[0].display_names["en"], "Mistbell Cellar");
    assert!(scripts[0].suggested_actions["zh-Hans"].contains("环顾四周"));
    assert!(scripts[0].suggested_actions["en"].starts_with("I look around"));
    assert!(scripts[0].attributions.contains("Creative Commons"));
}

#[test]
fn entity_manifest_rejects_unknown_fields_duplicate_keys_and_oversized_input() {
    for source in [
        br#"{"version":1,"revision":1,"complete":true,"entities":[],"unexpected":true}"#.as_slice(),
        br#"{"version":1,"version":1,"revision":1,"complete":true,"entities":[]}"#,
        br#"{"version":1,"revision":1,"complete":true,"entities":[{"id":"person","kind":"character","scope":"global","canonicalName":"Person","extra":0}]}"#,
    ] {
        assert_eq!(decode_entity_manifest(source).unwrap_err().code, "app.not-ready");
    }
    assert_eq!(
        decode_entity_manifest(&vec![b' '; MAX_ENTITY_MANIFEST_BYTES + 1])
            .unwrap_err()
            .code,
        "app.not-ready"
    );
}

#[test]
fn entity_manifest_rejects_unsupported_version_zero_revision_and_empty_directory() {
    for source in [
        br#"{"version":2,"revision":1,"complete":true,"entities":[{"id":"person","kind":"character","scope":"global","canonicalName":"Person"}]}"#.as_slice(),
        br#"{"version":1,"revision":0,"complete":true,"entities":[{"id":"person","kind":"character","scope":"global","canonicalName":"Person"}]}"#,
        br#"{"version":1,"revision":1,"complete":true,"entities":[]}"#,
    ] {
        let fault = decode_entity_manifest(source).unwrap_err();
        assert_eq!(fault.code, "app.not-ready");
        assert_eq!(fault.message, "内嵌剧本实体目录不可用");
    }
}

#[test]
fn entity_directory_preserves_every_registered_kind_and_manifest_revision() {
    let raw = RawScript {
        id: "fixture",
        body: b"fixture body",
        i18n: "",
        entities: br#"{"version":1,"revision":7,"complete":true,"entities":[
            {"id":"person","kind":"character","scope":"global","canonicalName":"Person"},
            {"id":"place","kind":"place","scope":"global","canonicalName":"Place"},
            {"id":"organization","kind":"organization","scope":"global","canonicalName":"Organization"},
            {"id":"item","kind":"item","scope":"global","canonicalName":"Item"},
            {"id":"term","kind":"term","scope":"global","canonicalName":"Term"}
        ]}"#,
        cycle_input: br#"{"version":1,"revision":1,"protagonistCharacterId":"person","characters":[{"characterId":"person","isGuide":false,"truth":{"personalName":"Person","background":"Background","identity":"Identity","race":"Human","age":"Adult","gender":"Female"},"initialKnowledge":["personalName"],"presentation":{"displayName":"Person","identity":"Identity","race":"Human","age":"Adult","gender":"Female","appearance":"A person"}}],"stateFields":[]}"#,
        legacy_cycle_input: br#"{"version":1,"revision":1,"stateFields":[]}"#,
        notices: &[],
    };
    let directory = entity_directory_from_raw(&raw).unwrap();
    assert_eq!(directory.revision(), 7);
    assert_eq!(
        directory.script_revision(),
        script_revision(raw.body, raw.entities, raw.cycle_input, 7, 1)
    );
    assert!(directory.is_complete());
    for (kind, id, name) in [
        (
            super::super::catalog::EntityKind::Character,
            "person",
            "Person",
        ),
        (super::super::catalog::EntityKind::Place, "place", "Place"),
        (
            super::super::catalog::EntityKind::Organization,
            "organization",
            "Organization",
        ),
        (super::super::catalog::EntityKind::Item, "item", "Item"),
        (super::super::catalog::EntityKind::Term, "term", "Term"),
    ] {
        let entity = directory
            .resolve_exact(kind, "global", name)
            .unwrap()
            .unwrap();
        assert_eq!(entity.id, id);
        assert_eq!(entity.kind, kind);
    }
}

#[test]
fn invalid_entity_directory_maps_to_a_safe_not_ready_fault() {
    for entities in [
        br#"{"version":1,"revision":1,"complete":true,"entities":[{"id":"bad/id","kind":"character","scope":"global","canonicalName":"Person"}]}"#.as_slice(),
        br#"{"version":1,"revision":1,"complete":true,"entities":[{"id":"person","kind":"character","scope":"global","canonicalName":"Person"},{"id":"person","kind":"character","scope":"global","canonicalName":"Other person"}]}"#,
        br#"{"version":1,"revision":1,"complete":true,"entities":[{"id":"person","kind":"character","scope":"global","canonicalName":"Person"},{"id":"other-person","kind":"character","scope":"global","canonicalName":"Person"}]}"#,
        b"invalid json",
    ] {
        let raw = RawScript {
            id: "fixture",
            body: b"fixture body",
            i18n: "",
            entities,
            cycle_input: br#"{"version":1,"revision":1,"stateFields":[]}"#,
            legacy_cycle_input: br#"{"version":1,"revision":1,"stateFields":[]}"#,
            notices: &[],
        };
        let fault = entity_directory_from_raw(&raw).err().unwrap();
        assert_eq!(fault.code, "app.not-ready");
        assert_eq!(fault.message, "内嵌剧本实体目录不可用");
        assert!(!fault.to_string().contains("Person"));
    }
}

#[test]
fn script_revision_covers_both_original_text_and_entity_manifest() {
    let baseline = script_revision(b"body", b"entities", b"cycle", 1, 1);
    assert_ne!(
        baseline,
        script_revision(b"changed", b"entities", b"cycle", 1, 1)
    );
    assert_ne!(
        baseline,
        script_revision(b"body", b"changed", b"cycle", 1, 1)
    );
    assert_ne!(
        baseline,
        script_revision(b"body", b"entities", b"changed", 1, 1)
    );
    assert_ne!(
        baseline,
        script_revision(b"body", b"entities", b"cycle", 2, 1)
    );
    assert_ne!(
        baseline,
        script_revision(b"body", b"entities", b"cycle", 1, 2)
    );
}

#[test]
fn legacy_script_revision_remains_available_without_binding_new_cycle_input() {
    let raw = &BUNDLED[0];
    let manifest = decode_entity_manifest(raw.entities).unwrap();
    let legacy = legacy_script_revision(raw.body, raw.entities, manifest.revision);
    assert_ne!(legacy, revision(raw.id).unwrap());
    let directory = entity_directory_for_revision(raw.id, &legacy).unwrap();
    assert_eq!(directory.script_revision(), legacy);
    assert_eq!(directory.revision(), manifest.revision);
}

#[test]
fn previously_current_cycle_input_revision_remains_available_after_schema_upgrade() {
    let raw = &BUNDLED[0];
    let entities = decode_entity_manifest(raw.entities).unwrap();
    let old_input: CycleInputManifest =
        aoidos_json::decode_bytes(raw.legacy_cycle_input, MAX_CYCLE_INPUT_BYTES).unwrap();
    let previous = script_revision(
        raw.body,
        raw.entities,
        raw.legacy_cycle_input,
        entities.revision,
        old_input.revision,
    );
    let directory = entity_directory_for_revision(raw.id, &previous).unwrap();
    assert_eq!(directory.script_revision(), previous);
    assert_eq!(directory.revision(), entities.revision);
}

#[test]
fn new_initialization_rejects_legacy_cycle_input_v1() {
    let raw = &BUNDLED[0];
    let manifest = decode_entity_manifest(raw.entities).unwrap();
    let legacy_input: CycleInputManifest =
        aoidos_json::decode_bytes(raw.legacy_cycle_input, MAX_CYCLE_INPUT_BYTES).unwrap();
    let legacy_revision = script_revision(
        raw.body,
        raw.entities,
        raw.legacy_cycle_input,
        manifest.revision,
        legacy_input.revision,
    );

    assert!(
        validate_frozen_initialization_assets(
            raw.id,
            &legacy_revision,
            raw.body,
            raw.entities,
            raw.legacy_cycle_input,
        )
        .is_err()
    );
}

#[test]
fn cycle_input_is_bounded_typed_versioned_and_bound_to_a_registered_character() {
    let manifest = decode_entity_manifest(BUNDLED[0].entities).unwrap();
    let input = decode_cycle_input(BUNDLED[0].cycle_input, &manifest).unwrap();
    assert_eq!(input.protagonist_character_id.as_deref(), Some("player"));
    assert_eq!(input.story_character_ids, ["player", "luo-wei"]);
    let guide = input
        .characters
        .iter()
        .find(|character| character.is_guide)
        .unwrap();
    assert_eq!(guide.character_id, "luo-wei");
    assert_eq!(guide.truth.race, "人类");
    assert_eq!(guide.presentation.display_name, "洛苇");
    assert!(guide.initial_knowledge.contains(&"background".to_owned()));
    assert_eq!(input.state_fields.len(), 1);
    assert_eq!(
        input.state_fields[0].source,
        "engine.committed-round-count.v1"
    );
    let unselected_character = decode_cycle_input(
        br#"{"version":1,"revision":1,"characters":[{"characterId":"luo-wei","truth":{"personalName":"Person","background":"x","identity":"x","race":"Human","age":"Adult","gender":"Female"},"initialKnowledge":[],"presentation":{"displayName":"Person","identity":"x","race":"Human","age":"Adult","gender":"Female","appearance":"A person"}}],"stateFields":[]}"#,
        &manifest,
    )
    .unwrap();
    assert!(
        !unselected_character
            .characters
            .iter()
            .any(|character| character.is_guide)
    );
    let explicitly_unselected = decode_cycle_input(
        br#"{"version":1,"revision":1,"characters":[{"characterId":"luo-wei","isGuide":false,"truth":{"personalName":"Person","background":"x","identity":"x","race":"Human","age":"Adult","gender":"Female"},"initialKnowledge":[],"presentation":{"displayName":"Person","identity":"x","race":"Human","age":"Adult","gender":"Female","appearance":"A person"}}],"stateFields":[]}"#,
        &manifest,
    )
    .unwrap();
    assert!(
        !explicitly_unselected
            .characters
            .iter()
            .any(|character| character.is_guide)
    );

    for source in [
        br#"{"version":2,"revision":1,"characters":[],"stateFields":[]}"#.as_slice(),
        br#"{"version":1,"revision":0,"stateFields":[]}"#,
        br#"{"version":1,"revision":1,"storyCharacterIds":["player"],"stateFields":[]}"#,
        br#"{"version":2,"revision":2,"storyCharacterIds":["luo-wei"],"protagonistCharacterId":"player","stateFields":[]}"#,
        br#"{"version":2,"revision":2,"storyCharacterIds":["luo-wei","luo-wei"],"stateFields":[]}"#,
        br#"{"version":1,"revision":1,"characters":[{"characterId":"missing","isGuide":true,"truth":{"personalName":"P","background":"x","identity":"x","race":"Human","age":"Adult","gender":"Female"},"initialKnowledge":[],"presentation":{"displayName":"P","identity":"x","race":"Human","age":"Adult","gender":"Female","appearance":"A person"},"openingRole":"x","openingRelationship":"x"}],"stateFields":[]}"#,
        br#"{"version":1,"revision":1,"characters":[{"characterId":"taoquan-village","isGuide":true,"truth":{"personalName":"P","background":"x","identity":"x","race":"Human","age":"Adult","gender":"Female"},"initialKnowledge":[],"presentation":{"displayName":"P","identity":"x","race":"Human","age":"Adult","gender":"Female","appearance":"A person"},"openingRole":"x","openingRelationship":"x"}],"stateFields":[]}"#,
        br#"{"version":1,"revision":1,"protagonistCharacterId":"taoquan-village","stateFields":[]}"#,
        br#"{"version":1,"revision":1,"protagonistCharacterId":"luo-wei","characters":[{"characterId":"luo-wei","isGuide":true,"truth":{"personalName":"P","background":"x","identity":"x","race":"Human","age":"Adult","gender":"Female"},"initialKnowledge":[],"presentation":{"displayName":"P","identity":"x","race":"Human","age":"Adult","gender":"Female","appearance":"A person"},"openingRole":"x","openingRelationship":"x"}],"stateFields":[]}"#,
        br#"{"version":1,"revision":1,"characters":[{"characterId":"person","isGuide":true,"truth":{"personalName":"P","background":"x","identity":"x","race":"Human","age":"Adult","gender":"Female"},"initialKnowledge":[],"presentation":{"displayName":"P","identity":"x","race":"Human","age":"Adult","gender":"Female","appearance":"A person"}}],"stateFields":[]}"#,
        br#"{"version":1,"revision":1,"characters":[{"characterId":"person","isGuide":true,"truth":{"personalName":"P","background":" ","identity":"x","race":"Human","age":"Adult","gender":"Female"},"initialKnowledge":[],"presentation":{"displayName":"P","identity":"x","race":"Human","age":"Adult","gender":"Female","appearance":"A person"},"openingRole":"x","openingRelationship":"x"}],"stateFields":[]}"#,
        br#"{"version":1,"revision":1,"characters":[{"characterId":"person","isGuide":true,"truth":{"personalName":"P","background":"x","identity":"x","race":"Human","age":"Adult","gender":"Female"},"initialKnowledge":[],"presentation":{"displayName":"P","identity":"x","race":"Human","age":"Adult","gender":"Female","appearance":"A person"},"openingRole":"x","openingRelationship":"x"},{"characterId":"person","isGuide":true,"truth":{"personalName":"P","background":"x","identity":"x","race":"Human","age":"Adult","gender":"Female"},"initialKnowledge":[],"presentation":{"displayName":"P","identity":"x","race":"Human","age":"Adult","gender":"Female","appearance":"A person"},"openingRole":"x","openingRelationship":"x"}],"stateFields":[]}"#,
        br#"{"version":1,"revision":1,"characters":[{"characterId":"luo-wei","truth":{"personalName":"P","background":"x","identity":"x","race":"Human","age":"Adult","gender":"Female"},"initialKnowledge":["secretFact"],"presentation":{"displayName":"P","identity":"x","race":"Human","age":"Adult","gender":"Female","appearance":"A person"}}],"stateFields":[]}"#,
        br#"{"version":1,"revision":1,"characters":[{"characterId":"luo-wei","truth":{"personalName":"P","background":"x","identity":"x","race":"Human","age":"Adult","gender":"Female"},"initialKnowledge":["race","race"],"presentation":{"displayName":"P","identity":"x","race":"Human","age":"Adult","gender":"Female","appearance":"A person"}}],"stateFields":[]}"#,
        br#"{"version":1,"revision":1,"stateFields":[{"id":"rounds","kind":"integer","source":"engine.committed-round-count.v1","min":1,"max":3,"operators":["atLeast"],"descriptions":{"en":"x","zh-Hans":"x"}}]}"#,
        br#"{"version":1,"revision":1,"stateFields":[{"id":"committed-round-count","kind":"integer","source":"unregistered","min":1,"max":3,"operators":["atLeast"],"descriptions":{"en":"x","zh-Hans":"x"}}]}"#,
        br#"{"version":1,"revision":1,"stateFields":[{"id":"committed-round-count","kind":"integer","source":"engine.committed-round-count.v1","min":1,"max":3,"operators":["greaterThan"],"descriptions":{"en":"x","zh-Hans":"x"}}]}"#,
        br#"{"version":1,"revision":1,"stateFields":[{"id":"committed-round-count","kind":"integer","source":"engine.committed-round-count.v1","min":4,"max":3,"operators":["atLeast"],"descriptions":{"en":"x","zh-Hans":"x"}}]}"#,
        br#"{"version":1,"revision":1,"stateFields":[{"id":"committed-round-count","kind":"integer","source":"engine.committed-round-count.v1","min":1,"max":3,"operators":["atLeast"],"descriptions":{"en":"x"}}]}"#,
    ] {
        assert_eq!(decode_cycle_input(source, &manifest).unwrap_err().code, "app.not-ready");
    }
}

#[test]
fn frozen_cycle_input_rejects_incomplete_or_invalid_entity_snapshots() {
    let cycle_input = BUNDLED[0].cycle_input;
    let mut snapshot: serde_json::Value = serde_json::from_slice(BUNDLED[0].entities).unwrap();
    snapshot["complete"] = serde_json::Value::Bool(false);
    assert!(
        validate_cycle_input_snapshot(cycle_input, &serde_json::to_vec(&snapshot).unwrap())
            .is_err()
    );

    let mut snapshot: serde_json::Value = serde_json::from_slice(BUNDLED[0].entities).unwrap();
    let duplicate = snapshot["entities"][0].clone();
    snapshot["entities"].as_array_mut().unwrap().push(duplicate);
    assert!(
        validate_cycle_input_snapshot(cycle_input, &serde_json::to_vec(&snapshot).unwrap())
            .is_err()
    );

    let mut snapshot: serde_json::Value = serde_json::from_slice(BUNDLED[0].entities).unwrap();
    let mut conflicting_kind = snapshot["entities"][0].clone();
    conflicting_kind["kind"] = serde_json::Value::String("place".into());
    conflicting_kind["canonicalName"] = serde_json::Value::String("Not a character".into());
    snapshot["entities"]
        .as_array_mut()
        .unwrap()
        .push(conflicting_kind);
    assert!(
        validate_cycle_input_snapshot(cycle_input, &serde_json::to_vec(&snapshot).unwrap())
            .is_err()
    );
}

#[test]
fn script_i18n_rejects_invalid_or_incomplete_display_names() {
    let oversized = format!(
        r#"{{"version":1,"displayNames":{{"en":"{}","zh-Hans":"雾钟地窖"}},"suggestedActions":{{"en":"look around","zh-Hans":"环顾四周"}}}}"#,
        "x".repeat(257)
    );
    for source in [
        r#"{"version":2,"displayNames":{"en":"Mistbell Cellar","zh-Hans":"雾钟地窖"},"suggestedActions":{"en":"look around","zh-Hans":"环顾四周"}}"#,
        r#"{"version":1,"displayNames":{"en":"Mistbell Cellar"},"suggestedActions":{"en":"look around","zh-Hans":"环顾四周"}}"#,
        r#"{"version":1,"displayNames":{"en":" ","zh-Hans":"雾钟地窖"},"suggestedActions":{"en":"look around","zh-Hans":"环顾四周"}}"#,
        r#"{"version":1,"displayNames":{"en":"Mistbell Cellar","zh-Hans":"雾钟地窖","bad_tag":"Bad"},"suggestedActions":{"en":"look around","zh-Hans":"环顾四周"}}"#,
        r#"{"version":1,"displayNames":{"en":"Mistbell Cellar","zh-Hans":"雾钟地窖"},"suggestedActions":{"en":"look around"}}"#,
        &oversized,
    ] {
        assert_eq!(parse_i18n(source).unwrap_err().code, "app.not-ready");
    }
}

#[test]
fn malformed_localization_json_maps_to_a_safe_not_ready_fault() {
    let fault = parse_i18n(
        r#"{"version":1,"displayNames":{"en":"Mistbell Cellar","zh-Hans":"雾钟地窖"},"suggestedActions":{"en":"look around","zh-Hans":"环顾四周"}"#,
    )
    .unwrap_err();

    assert_eq!(fault.code, "app.not-ready");
    assert_eq!(fault.message, "内嵌剧本本地化资源不可用");
    assert!(!fault.to_string().contains("displayNames"));
}

#[test]
fn unknown_identity_cannot_be_treated_as_a_file_path() {
    for id in ["", "../mistbell", "Mistbell", "/tmp/script.md"] {
        assert_eq!(scenario(id).unwrap_err().code, "app.not-found");
        assert_eq!(revision(id).unwrap_err().code, "app.not-found");
    }
    assert_eq!(
        invalid_asset(aoidos_script::Error::InvalidText).code,
        "app.not-ready"
    );
}
