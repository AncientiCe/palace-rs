use palace::db;
use palace::knowledge_graph::*;

fn test_db() -> rusqlite::Connection {
    db::open_in_memory().unwrap()
}

#[test]
fn add_entity_and_query() {
    let conn = test_db();
    let id = add_entity(&conn, "Alice", "person", None).unwrap();
    assert_eq!(id, "alice");
}

#[test]
fn add_and_query_triple() {
    let conn = test_db();
    let tid = add_triple(
        &conn,
        "Alice",
        "loves",
        "Coffee",
        Some("2025-01-01"),
        None,
        1.0,
        None,
        None,
    )
    .unwrap();
    assert!(!tid.is_empty());

    let facts = query_entity(&conn, "Alice", None, "outgoing").unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].predicate, "loves");
    assert_eq!(facts[0].object, "Coffee");
    assert!(facts[0].current);
}

#[test]
fn invalidate_ends_fact() {
    let conn = test_db();
    add_triple(
        &conn,
        "Bob",
        "works_at",
        "Acme",
        Some("2020-01-01"),
        None,
        1.0,
        None,
        None,
    )
    .unwrap();
    invalidate(&conn, "Bob", "works_at", "Acme", Some("2024-06-01")).unwrap();

    let facts = query_entity(&conn, "Bob", None, "both").unwrap();
    assert_eq!(facts.len(), 1);
    assert!(!facts[0].current);
    assert_eq!(facts[0].valid_to, Some("2024-06-01".to_string()));
}

#[test]
fn timeline_returns_ordered_facts() {
    let conn = test_db();
    add_triple(
        &conn,
        "Eve",
        "joined",
        "Team",
        Some("2020-01-01"),
        None,
        1.0,
        None,
        None,
    )
    .unwrap();
    add_triple(
        &conn,
        "Eve",
        "promoted",
        "Senior",
        Some("2022-06-01"),
        None,
        1.0,
        None,
        None,
    )
    .unwrap();

    let tl = timeline(&conn, Some("Eve")).unwrap();
    assert_eq!(tl.len(), 2);
    assert_eq!(tl[0].predicate, "joined");
    assert_eq!(tl[1].predicate, "promoted");
}

#[test]
fn stats_shows_counts() {
    let conn = test_db();
    add_triple(&conn, "X", "rel1", "Y", None, None, 1.0, None, None).unwrap();
    add_triple(&conn, "A", "rel2", "B", None, None, 1.0, None, None).unwrap();
    let s = stats(&conn).unwrap();
    assert_eq!(s.entities, 4);
    assert_eq!(s.triples, 2);
    assert_eq!(s.current_facts, 2);
    assert_eq!(s.expired_facts, 0);
    assert!(s.relationship_types.contains(&"rel1".to_string()));
}

#[test]
fn deduplication_prevents_duplicate_active_triples() {
    let conn = test_db();
    let id1 = add_triple(&conn, "A", "knows", "B", None, None, 1.0, None, None).unwrap();
    let id2 = add_triple(&conn, "A", "knows", "B", None, None, 1.0, None, None).unwrap();
    assert_eq!(id1, id2, "duplicate active triple should return same id");
}
