use super::*;
use crate::repository::test_support::Fixture;

#[test]
fn application_configuration_uses_versioned_defaults_and_compare_and_swap() {
    let mut fixture = Fixture::new_in_memory();
    let defaults = fixture.repo.app_config(&fixture.conn).unwrap();
    assert_eq!(defaults.config_version, 1);
    assert_eq!(
        defaults.default_parameters,
        PolicyParameters::development_defaults()
    );
    assert_eq!(
        defaults.parameters,
        PolicyParameters::development_defaults()
    );

    let mut changed = defaults.parameters.clone();
    changed.algorithm = crate::model::Algorithm::None;
    let saved = fixture
        .repo
        .save_app_config(&mut fixture.conn, 1, changed.clone())
        .unwrap();
    assert_eq!(saved.config_version, 2);
    let saved = fixture.repo.app_config(&fixture.conn).unwrap();
    assert_eq!(saved.parameters, changed);
    assert_eq!(
        saved.default_parameters,
        PolicyParameters::development_defaults()
    );

    assert!(matches!(
        fixture
            .repo
            .save_app_config(&mut fixture.conn, 1, defaults.parameters),
        Err(Error::Rejected(Reason::StaleVersion))
    ));
    let versions: u64 = fixture
        .conn
        .query_row(
            "SELECT count(*) FROM memory_config_versions WHERE scope_kind='application'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(versions, 2);
}

#[test]
fn script_overrides_inherit_fields_and_require_both_expected_versions() {
    let mut fixture = Fixture::new_in_memory();
    let app = fixture.repo.app_config(&fixture.conn).unwrap();
    let initial = fixture
        .repo
        .script_config(&fixture.conn, "mistbell")
        .unwrap();
    assert_eq!(initial.config_version, 0);
    assert_eq!(initial.app_config_version, app.config_version);
    assert_eq!(initial.inherited, app.parameters);
    assert_eq!(initial.effective, app.parameters);

    let overrides = PolicyOverrides {
        reinforcement_gain: Some(0),
        ..PolicyOverrides::default()
    };
    let saved = fixture
        .repo
        .save_script_config(
            &mut fixture.conn,
            "mistbell",
            0,
            app.config_version,
            overrides.clone(),
        )
        .unwrap();
    assert_eq!(saved.config_version, 1);
    let stored = fixture
        .repo
        .script_config(&fixture.conn, "mistbell")
        .unwrap();
    assert_eq!(stored.overrides, overrides);
    assert_eq!(stored.effective.reinforcement_gain, 0);
    assert_eq!(
        stored.effective.half_life_rounds,
        app.parameters.half_life_rounds
    );

    assert!(matches!(
        fixture.repo.save_script_config(
            &mut fixture.conn,
            "mistbell",
            0,
            app.config_version,
            PolicyOverrides::default(),
        ),
        Err(Error::Rejected(Reason::StaleVersion))
    ));
    assert!(matches!(
        fixture.repo.save_script_config(
            &mut fixture.conn,
            "mistbell",
            1,
            app.config_version + 1,
            PolicyOverrides::default(),
        ),
        Err(Error::Rejected(Reason::StaleVersion))
    ));

    let reset = fixture
        .repo
        .save_script_config(
            &mut fixture.conn,
            "mistbell",
            1,
            app.config_version,
            PolicyOverrides::default(),
        )
        .unwrap();
    assert_eq!(reset.config_version, 2);
    let inherited = fixture
        .repo
        .script_config(&fixture.conn, "mistbell")
        .unwrap();
    assert_eq!(inherited.effective, app.parameters);
    assert_eq!(inherited.inherited, app.parameters);
    assert_eq!(inherited.overrides, PolicyOverrides::default());
}

#[test]
fn application_update_cannot_make_an_existing_script_override_invalid() {
    let mut fixture = Fixture::new_in_memory();
    let app = fixture.repo.app_config(&fixture.conn).unwrap();
    fixture
        .repo
        .save_script_config(
            &mut fixture.conn,
            "mistbell",
            0,
            app.config_version,
            PolicyOverrides {
                base_strength: Some(900_000),
                reinforcement_cap: Some(0),
                ..PolicyOverrides::default()
            },
        )
        .unwrap();

    let mut incompatible = app.parameters.clone();
    incompatible.base_strength = 200_000;
    incompatible.strength_cap = 800_000;
    assert!(matches!(
        fixture
            .repo
            .save_app_config(&mut fixture.conn, app.config_version, incompatible),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
    assert_eq!(fixture.repo.app_config(&fixture.conn).unwrap(), app);
}

#[test]
fn script_configuration_rejects_null_unknown_and_cross_field_conflicts() {
    assert!(aoidos_json::decode::<PolicyOverrides>(r#"{"baseStrength":null}"#, 1024).is_err());
    assert!(aoidos_json::decode::<PolicyOverrides>(r#"{"unknown":1}"#, 1024).is_err());

    let mut fixture = Fixture::new_in_memory();
    let app = fixture.repo.app_config(&fixture.conn).unwrap();
    assert!(matches!(
        fixture.repo.save_script_config(
            &mut fixture.conn,
            "mistbell",
            0,
            app.config_version,
            PolicyOverrides {
                base_strength: Some(900_000),
                reinforcement_cap: Some(900_000),
                ..PolicyOverrides::default()
            },
        ),
        Err(Error::Rejected(Reason::InvalidSchema))
    ));
    assert_eq!(
        fixture
            .repo
            .script_config(&fixture.conn, "mistbell")
            .unwrap()
            .config_version,
        0
    );
}

#[test]
fn corrupt_configuration_payload_fails_closed() {
    let mut fixture = Fixture::new_in_memory();
    fixture
        .repo
        .save_app_config(
            &mut fixture.conn,
            1,
            PolicyParameters::development_defaults(),
        )
        .unwrap();
    fixture
        .conn
        .execute(
            "UPDATE memory_config_versions SET payload_json=?1 WHERE scope_kind='application' AND config_version=2",
            [b"{}".as_slice()],
        )
        .unwrap();
    assert!(matches!(
        fixture.repo.app_config(&fixture.conn),
        Err(Error::Corrupt)
    ));
}

#[test]
fn unknown_future_configuration_versions_remain_unavailable_not_corrupt() {
    for (column_version, payload) in [
        (
            2,
            serde_json::json!({
                "version": 1,
                "algorithmVersion": 2,
                "value": { "futureParameter": true }
            }),
        ),
        (
            1,
            serde_json::json!({
                "version": 2,
                "algorithmVersion": 1,
                "value": { "futureParameter": true }
            }),
        ),
    ] {
        let mut fixture = Fixture::new_in_memory();
        fixture
            .repo
            .save_app_config(
                &mut fixture.conn,
                1,
                PolicyParameters::development_defaults(),
            )
            .unwrap();
        let bytes = aoidos_json::canonical_string(&payload)
            .unwrap()
            .into_bytes();
        fixture
            .conn
            .execute(
                "UPDATE memory_config_versions SET algorithm_version=?1,payload_hash=?2,payload_json=?3 WHERE scope_kind='application' AND config_version=2",
                rusqlite::params![column_version, hash(&bytes).as_slice(), bytes],
            )
            .unwrap();

        assert!(matches!(
            fixture.repo.app_config(&fixture.conn),
            Err(Error::Rejected(Reason::PolicyUnavailable))
        ));
    }
}
