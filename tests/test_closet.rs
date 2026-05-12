use palace::closet::{add_closet, list_closets};
use palace::db;

#[test]
fn closets_store_topic_pointers_for_a_room() {
    let conn = db::open_in_memory().expect("db should open");
    let pointers = vec!["drawer_a".to_string(), "drawer_b".to_string()];

    add_closet(&conn, "wing", "room", "database schema", &pointers, None)
        .expect("closet should insert");

    let closets = list_closets(&conn, Some("wing"), Some("room")).expect("closets should list");
    assert_eq!(closets.len(), 1);
    assert_eq!(closets[0].topic, "database schema");
    assert_eq!(closets[0].pointer_drawer_ids, pointers);
}
