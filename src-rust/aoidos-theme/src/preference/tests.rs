use super::*;

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(crate::schema::SCHEMA).unwrap();
    connection
}

#[test]
fn reads_default_and_round_trips_only_theme_choice() {
    let mut connection = connection();
    assert_eq!(get(&connection).unwrap(), ThemePreference::default());
    assert_eq!(set(&mut connection, "light".into()).unwrap().theme, "light");
    assert_eq!(get(&connection).unwrap().theme, "light");
    assert_eq!(
        set(&mut connection, "light-purple".into()).unwrap().theme,
        "light-purple"
    );
    assert_eq!(get(&connection).unwrap().theme, "light-purple");
    assert_eq!(set(&mut connection, "dark".into()).unwrap().theme, "dark");
    assert_eq!(get(&connection).unwrap().theme, "dark");
}

#[test]
fn unknown_stored_version_or_value_is_not_overwritten() {
    let connection = connection();
    connection
        .execute("INSERT INTO theme_preference VALUES (1, 1, 'dark')", [])
        .unwrap();
    connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    connection
        .execute("UPDATE theme_preference SET theme='Bad ID' WHERE id=1", [])
        .unwrap();
    assert_eq!(get(&connection).unwrap_err().code(), "corrupt");
    assert_eq!(
        connection
            .query_row("SELECT theme FROM theme_preference", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "Bad ID"
    );
}

#[test]
fn unsupported_persisted_version_and_unregistered_set_value_are_rejected() {
    let mut connection = connection();
    assert_eq!(
        set(&mut connection, "system".into()).unwrap_err().code(),
        "corrupt"
    );
    assert_eq!(get(&connection).unwrap().theme, "dark");

    connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    connection
        .execute("INSERT INTO theme_preference VALUES (1, 2, 'dark')", [])
        .unwrap();
    assert_eq!(get(&connection).unwrap_err().code(), "corrupt");
    assert_eq!(
        connection
            .query_row("SELECT version FROM theme_preference", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn removed_but_well_formed_theme_id_is_preserved_for_startup_fallback() {
    let mut connection = connection();
    connection
        .execute(
            "INSERT INTO theme_preference VALUES (1, 1, 'retired-theme')",
            [],
        )
        .unwrap();

    assert_eq!(get(&connection).unwrap().theme, "retired-theme");
    assert_eq!(
        set(&mut connection, "retired-theme".into())
            .unwrap_err()
            .code(),
        "corrupt"
    );
    assert_eq!(get(&connection).unwrap().theme, "retired-theme");
}

#[test]
fn schema_validation_rejects_missing_table() {
    let connection = Connection::open_in_memory().unwrap();
    assert_eq!(
        crate::schema::verify(&connection).unwrap_err().code(),
        "corrupt"
    );
}
