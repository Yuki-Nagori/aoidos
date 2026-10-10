use super::*;
use rusqlite::Connection;

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(crate::schema::SCHEMA).unwrap();
    connection
}

#[test]
fn maps_language_and_script_regions_to_supported_locales() {
    for tag in ["zh", "zh-CN", "zh-SG", "zh-TW", "zh-HK", "zh-Hant"] {
        assert_eq!(resolve_language_tag(Some(tag)), Locale::ZhHans, "{tag}");
    }
    for tag in ["en", "en-US", "en-GB", "fr-FR", "", "bad_tag"] {
        assert_eq!(resolve_language_tag(Some(tag)), Locale::En, "{tag}");
    }
    assert_eq!(resolve_language_tag(None), Locale::En);
}

#[test]
fn resolves_explicit_choices_and_uses_the_system_language_for_system_choice() {
    assert_eq!(LocaleChoice::ZhHans.resolve(), Locale::ZhHans);
    assert_eq!(LocaleChoice::En.resolve(), Locale::En);
    assert_eq!(
        LocaleChoice::System.resolve(),
        resolve_language_tag(sys_locale::get_locale().as_deref())
    );
}

#[test]
fn reads_system_default_and_round_trips_each_supported_choice() {
    let mut connection = connection();
    assert_eq!(get(&connection).unwrap(), LocalePreference::default());
    for locale in [LocaleChoice::System, LocaleChoice::ZhHans, LocaleChoice::En] {
        let expected = LocalePreference { version: 1, locale };
        assert_eq!(set(&mut connection, expected).unwrap(), expected);
        assert_eq!(get(&connection).unwrap(), expected);
    }
}

#[test]
fn rejects_bad_persisted_values_and_versions_without_overwriting_them() {
    let mut connection = connection();
    assert_eq!(
        set(
            &mut connection,
            LocalePreference {
                version: 2,
                locale: LocaleChoice::En,
            },
        )
        .unwrap_err()
        .code(),
        "corrupt"
    );
    connection
        .execute("INSERT INTO locale_preference VALUES (1, 1, 'system')", [])
        .unwrap();
    connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    connection
        .execute("UPDATE locale_preference SET locale='fr' WHERE id=1", [])
        .unwrap();
    assert_eq!(get(&connection).unwrap_err().code(), "corrupt");
    assert_eq!(
        connection
            .query_row("SELECT locale FROM locale_preference", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "fr"
    );
}

#[test]
fn rejects_unknown_persisted_schema_version() {
    let connection = connection();
    connection
        .execute("INSERT INTO locale_preference VALUES (1, 1, 'en')", [])
        .unwrap();
    connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    connection
        .execute("UPDATE locale_preference SET version=2 WHERE id=1", [])
        .unwrap();

    assert_eq!(get(&connection).unwrap_err().code(), "corrupt");
}

#[test]
fn maps_preference_write_failure_to_store_io_error() {
    let mut connection = connection();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_locale_preference BEFORE INSERT ON locale_preference
             BEGIN SELECT RAISE(ABORT, 'test write failure'); END;",
        )
        .unwrap();

    let error = set(
        &mut connection,
        LocalePreference {
            version: 1,
            locale: LocaleChoice::En,
        },
    )
    .unwrap_err();

    assert_eq!(error.code(), "io");
    assert!(connection.is_autocommit(), "failed write must roll back");
    assert_eq!(get(&connection).unwrap(), LocalePreference::default());
}

#[test]
fn schema_verification_rejects_a_missing_table() {
    assert_eq!(
        crate::schema::verify(&Connection::open_in_memory().unwrap())
            .unwrap_err()
            .code(),
        "corrupt"
    );
}
