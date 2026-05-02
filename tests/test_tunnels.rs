use mempalace::config::normalize_wing_name;
use mempalace::db;
use mempalace::palace_graph::{
    create_tunnel, delete_tunnel, follow_tunnels, list_tunnels, topic_room,
};

#[test]
fn persisted_tunnels_can_be_created_followed_and_deleted() {
    let conn = db::open_in_memory().expect("db should open");

    let id = create_tunnel(&conn, "Wing A", "api", "Wing B", "backend", "explicit")
        .expect("tunnel should create");
    let tunnels = list_tunnels(&conn, Some("wing_a"), None).expect("tunnels should list");
    assert_eq!(tunnels.len(), 1);
    assert_eq!(tunnels[0].id, id);
    assert_eq!(tunnels[0].wing_a, "wing_a");

    let followed = follow_tunnels(&conn, "wing_a", "api").expect("tunnels should follow");
    assert_eq!(followed.len(), 1);
    assert_eq!(followed[0].wing_b, "wing_b");

    assert!(delete_tunnel(&conn, &id).expect("tunnel should delete"));
    assert!(list_tunnels(&conn, None, None).unwrap().is_empty());
}

#[test]
fn topic_tunnel_rooms_are_synthetic() {
    assert_eq!(topic_room("Angular"), "topic:angular");
    assert_eq!(normalize_wing_name("mempalace-public"), "mempalace_public");
}
