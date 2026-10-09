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
    assert!(scripts[0].attributions.contains("Creative Commons"));
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
