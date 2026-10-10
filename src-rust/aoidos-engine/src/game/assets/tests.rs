use super::*;

#[test]
fn default_original_and_attributions_are_available_without_runtime_files() {
    let original = scenario("mistbell").unwrap();
    assert_eq!(original.title, "雾钟地窖");
    assert_eq!(original.body.as_bytes(), BUNDLED[0].body);
    assert_eq!(
        revision("mistbell").unwrap(),
        crate::record::format::hash(original.body.as_bytes())
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
