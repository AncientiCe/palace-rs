use palace::backend::Backend;
use palace::backends::sqlite::SqliteBackend;

#[test]
fn sqlite_backend_exposes_palace_identity_and_count() {
    let backend = SqliteBackend::open_in_memory().expect("backend should open");
    let palace_ref = backend.palace_ref();

    assert_eq!(palace_ref.backend, "sqlite");
    assert_eq!(palace_ref.location, ":memory:");
    assert_eq!(backend.count_drawers().unwrap(), 0);
}
