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
        notices: &[],
    };
    let directory = entity_directory_from_raw(&raw).unwrap();
    assert_eq!(directory.revision(), 7);
    assert_eq!(
        directory.script_revision(),
        script_revision(raw.body, raw.entities, 7)
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
    let baseline = script_revision(b"body", b"entities", 1);
    assert_ne!(baseline, script_revision(b"changed", b"entities", 1));
    assert_ne!(baseline, script_revision(b"body", b"changed", 1));
    assert_ne!(baseline, script_revision(b"body", b"entities", 2));
}

#[test]
fn script_i18n_rejects_invalid_or_incomplete_display_names() {
    let oversized = format!(
        r#"{{"version":1,"displayNames":{{"en":"{}","zh-Hans":"雾钟地窖"}}}}"#,
        "x".repeat(257)
    );
    for source in [
        r#"{"version":2,"displayNames":{"en":"Mistbell Cellar","zh-Hans":"雾钟地窖"}}"#,
        r#"{"version":1,"displayNames":{"en":"Mistbell Cellar"}}"#,
        r#"{"version":1,"displayNames":{"en":" ","zh-Hans":"雾钟地窖"}}"#,
        r#"{"version":1,"displayNames":{"en":"Mistbell Cellar","zh-Hans":"雾钟地窖","bad_tag":"Bad"}}"#,
        &oversized,
    ] {
        assert_eq!(
            parse_display_names(source).unwrap_err().code,
            "app.not-ready"
        );
    }
}

#[test]
fn malformed_localization_json_maps_to_a_safe_not_ready_fault() {
    let fault = parse_display_names(
        r#"{"version":1,"displayNames":{"en":"Mistbell Cellar","zh-Hans":"雾钟地窖"}"#,
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
