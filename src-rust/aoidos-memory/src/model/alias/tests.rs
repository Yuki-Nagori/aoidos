use super::*;

#[test]
fn persisted_entity_categories_round_trip_without_accepting_unknown_labels() {
    for category in [
        EntityCategory::Character,
        EntityCategory::Place,
        EntityCategory::Organization,
        EntityCategory::Item,
        EntityCategory::Term,
    ] {
        assert_eq!(EntityCategory::from_db(category.as_db()), Some(category));
    }
    assert_eq!(EntityCategory::from_db("future"), None);
}
