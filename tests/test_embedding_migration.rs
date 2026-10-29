//! Switching a palace between embedding models.
//!
//! New palaces start on the default model (`minilm-l6-v2-q8`); the target
//! here is the legacy fp32 MiniLM. The active model is process-wide, so tests
//! that switch it hold `SERIAL`.

use palace::embedding_migration::{self as migration, OwnerKind};
use palace::embedding_models;
use palace::ranker::hybrid_search;
use palace::store::{add_drawer, DrawerFilter};
use rusqlite::Connection;
use std::sync::Mutex;

const TARGET: &str = embedding_models::LEGACY_MODEL_ID;

static SERIAL: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn seeded_palace(dir: &tempfile::TempDir) -> Connection {
    let conn = palace::db::open(&dir.path().join("palace.db")).expect("open palace");
    let texts = [
        "We chose bundled SQLite so users never install a database server.",
        "I prefer small public APIs that route through the Palace facade.",
        "The migration test failed because legacy drawers lacked metadata columns.",
        "Run cargo clippy with -D warnings before every commit to keep CI green.",
        "Hooks remind agents to search the palace before grepping the repository.",
        "Release tags are pushed only after CI is green on every operating system.",
    ];
    let vectors = palace::embedder::embed_documents(&texts).expect("embed");
    for (index, (text, vector)) in texts.iter().zip(&vectors).enumerate() {
        add_drawer(
            &conn,
            "w",
            "r",
            text,
            Some(vector),
            "notes.md",
            index,
            "test",
            3.0,
        )
        .expect("insert drawer");
    }
    conn
}

/// `(id, embedding, pref_embedding)` per drawer.
type HotRow = (String, Option<Vec<u8>>, Option<Vec<u8>>);

fn hot_vectors(conn: &Connection) -> Vec<HotRow> {
    let mut stmt = conn
        .prepare("SELECT id, embedding, pref_embedding FROM drawers ORDER BY id")
        .unwrap();
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn stored_rows(conn: &Connection, model: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM embedding_vectors WHERE model = ?1",
        [model],
        |row| row.get(0),
    )
    .unwrap()
}

fn backfill(conn: &Connection) -> u64 {
    let spec = embedding_models::find(TARGET).unwrap();
    migration::backfill(conn, spec, &mut |_| {}).expect("backfill")
}

fn ranked_ids(conn: &Connection, query: &str) -> Vec<(String, f64)> {
    let qvec = palace::embedder::embed_query(query).unwrap();
    hybrid_search(conn, query, Some(&qvec), &DrawerFilter::default(), 5)
        .unwrap()
        .into_iter()
        .map(|r| (r.drawer.id, r.combined))
        .collect()
}

#[test]
fn new_palace_records_default_model_and_has_vector_table() {
    let _guard = lock();
    let dir = tempfile::tempdir().unwrap();
    let conn = seeded_palace(&dir);
    assert_eq!(
        migration::active_model_id(&conn).unwrap(),
        embedding_models::DEFAULT_MODEL_ID
    );
    assert_eq!(stored_rows(&conn, TARGET), 0);
}

#[test]
fn legacy_palace_is_marked_legacy_without_rewriting_vectors() {
    let _guard = lock();
    let dir = tempfile::tempdir().unwrap();
    let conn = seeded_palace(&dir);
    conn.execute("DELETE FROM meta WHERE key = 'active_embedding_model'", [])
        .unwrap();
    let before = hot_vectors(&conn);
    drop(conn);

    let conn = palace::db::open(&dir.path().join("palace.db")).unwrap();
    assert_eq!(
        migration::active_model_id(&conn).unwrap(),
        embedding_models::LEGACY_MODEL_ID
    );
    assert_eq!(hot_vectors(&conn), before);
    palace::embedder::set_active_model(embedding_models::DEFAULT_MODEL_ID).unwrap();
}

#[test]
fn configured_model_applies_only_to_empty_palaces() {
    let _guard = lock();
    let empty = palace::db::open_in_memory().unwrap();
    empty
        .execute("DELETE FROM meta WHERE key = 'active_embedding_model'", [])
        .unwrap();
    migration::ensure_active_model(&empty, Some(TARGET)).unwrap();
    assert_eq!(migration::active_model_id(&empty).unwrap(), TARGET);

    let dir = tempfile::tempdir().unwrap();
    let used = seeded_palace(&dir);
    used.execute("DELETE FROM meta WHERE key = 'active_embedding_model'", [])
        .unwrap();
    migration::ensure_active_model(&used, Some(TARGET)).unwrap();
    assert_eq!(
        migration::active_model_id(&used).unwrap(),
        embedding_models::LEGACY_MODEL_ID,
        "a palace with vectors and no recorded model predates model choice"
    );
    palace::embedder::set_active_model(embedding_models::DEFAULT_MODEL_ID).unwrap();
}

#[test]
fn interrupted_backfill_resumes_without_duplicates() {
    let _guard = lock();
    let dir = tempfile::tempdir().unwrap();
    let conn = seeded_palace(&dir);
    // Enough drawers for several batches.
    for i in 0..70 {
        add_drawer(
            &conn,
            "w",
            "bulk",
            &format!("bulk note {i} about resumable backfills"),
            None,
            "bulk.md",
            i,
            "test",
            3.0,
        )
        .unwrap();
    }
    let spec = migration::start(&conn, TARGET).unwrap();

    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        migration::backfill(&conn, spec, &mut |written| {
            if written >= 32 {
                panic!("simulated interruption");
            }
        })
    }));
    assert!(interrupted.is_err());
    let partial = migration::coverage(&conn, TARGET).unwrap();
    assert!(partial.drawers_done >= 32 && !partial.complete());

    backfill(&conn);
    let full = migration::coverage(&conn, TARGET).unwrap();
    assert!(full.complete(), "{full:?}");
    assert_eq!(
        stored_rows(&conn, TARGET) as u64,
        full.total(),
        "one row per owner"
    );
    assert_eq!(backfill(&conn), 0, "a finished backfill has nothing to do");
}

#[test]
fn search_is_unchanged_until_cutover() {
    let _guard = lock();
    let dir = tempfile::tempdir().unwrap();
    let conn = seeded_palace(&dir);
    let before = ranked_ids(&conn, "why bundled sqlite?");
    migration::start(&conn, TARGET).unwrap();
    backfill(&conn);
    assert_eq!(ranked_ids(&conn, "why bundled sqlite?"), before);
    assert_eq!(
        migration::active_model_id(&conn).unwrap(),
        embedding_models::DEFAULT_MODEL_ID
    );
}

#[test]
fn cutover_requires_verify_unless_forced() {
    let _guard = lock();
    let dir = tempfile::tempdir().unwrap();
    let conn = seeded_palace(&dir);
    migration::start(&conn, TARGET).unwrap();
    backfill(&conn);
    let err = migration::cutover(&conn, false).unwrap_err();
    assert!(err.to_string().contains("verify"), "{err}");

    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('migration_verified', ?1)",
        [format!("{TARGET}:failed")],
    )
    .unwrap();
    assert!(migration::cutover(&conn, false).is_err());
    assert_eq!(
        migration::active_model_id(&conn).unwrap(),
        embedding_models::DEFAULT_MODEL_ID
    );
}

#[test]
fn verify_scores_both_models_and_records_the_result() {
    let _guard = lock();
    let dir = tempfile::tempdir().unwrap();
    let conn = seeded_palace(&dir);
    migration::start(&conn, TARGET).unwrap();
    assert!(
        migration::verify(&conn, 20, None).is_err(),
        "verify needs a finished backfill"
    );
    backfill(&conn);
    let report = migration::verify(&conn, 20, Some(10_000.0)).unwrap();
    assert!(report.probes > 0);
    assert_eq!(report.target.model, TARGET);
    let status = migration::status(&conn, None).unwrap();
    let expected = if report.passed { "passed" } else { "failed" };
    assert_eq!(status.verified.as_deref(), Some(expected));
}

#[test]
fn cutover_then_rollback_restores_identical_vectors() {
    let _guard = lock();
    let dir = tempfile::tempdir().unwrap();
    let conn = seeded_palace(&dir);
    let original = hot_vectors(&conn);
    migration::start(&conn, TARGET).unwrap();
    backfill(&conn);

    let switched = migration::cutover(&conn, true).unwrap();
    assert_eq!(switched.to, TARGET);
    assert_eq!(migration::active_model_id(&conn).unwrap(), TARGET);
    assert_eq!(palace::embedder::active_model().id, TARGET);
    assert_ne!(hot_vectors(&conn), original);
    assert_eq!(
        stored_rows(&conn, TARGET),
        0,
        "target now lives in hot columns"
    );
    assert!(stored_rows(&conn, embedding_models::DEFAULT_MODEL_ID) > 0);
    // Search still works on the new model.
    assert!(!ranked_ids(&conn, "why bundled sqlite?").is_empty());

    migration::rollback(&conn).unwrap();
    assert_eq!(
        migration::active_model_id(&conn).unwrap(),
        embedding_models::DEFAULT_MODEL_ID
    );
    assert_eq!(hot_vectors(&conn), original, "rollback is byte-identical");
    palace::embedder::set_active_model(embedding_models::DEFAULT_MODEL_ID).unwrap();
}

#[test]
fn deleting_or_editing_a_drawer_drops_its_stored_vectors() {
    let _guard = lock();
    let dir = tempfile::tempdir().unwrap();
    let conn = seeded_palace(&dir);
    migration::start(&conn, TARGET).unwrap();
    backfill(&conn);
    let ids: Vec<String> = hot_vectors(&conn)
        .into_iter()
        .map(|(id, _, _)| id)
        .collect();
    let owned = |id: &str| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM embedding_vectors WHERE owner_id = ?1 AND owner_kind = ?2",
            [id, OwnerKind::Drawer.as_str()],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(owned(&ids[0]), 1);

    palace::store::delete_drawer(&conn, &ids[0]).unwrap();
    assert_eq!(owned(&ids[0]), 0, "deleted drawers lose stored vectors");

    conn.execute(
        "UPDATE drawers SET content = 'rewritten content' WHERE id = ?1",
        [&ids[1]],
    )
    .unwrap();
    assert_eq!(owned(&ids[1]), 0, "edited drawers lose stale vectors");
    assert_eq!(owned(&ids[2]), 1);
}

#[test]
fn cleanup_drops_archived_vectors_and_rollback_target() {
    let _guard = lock();
    let dir = tempfile::tempdir().unwrap();
    let conn = seeded_palace(&dir);
    migration::start(&conn, TARGET).unwrap();
    backfill(&conn);
    migration::cutover(&conn, true).unwrap();
    let report = migration::cleanup(&conn).unwrap();
    assert!(report.rows_deleted > 0);
    assert_eq!(stored_rows(&conn, embedding_models::DEFAULT_MODEL_ID), 0);
    assert!(migration::rollback(&conn).is_err());
    palace::embedder::set_active_model(embedding_models::DEFAULT_MODEL_ID).unwrap();
}

#[test]
fn cancel_keeps_vectors_so_restart_resumes() {
    let _guard = lock();
    let dir = tempfile::tempdir().unwrap();
    let conn = seeded_palace(&dir);
    migration::start(&conn, TARGET).unwrap();
    backfill(&conn);
    assert_eq!(migration::cancel(&conn).unwrap().as_deref(), Some(TARGET));
    assert!(migration::target(&conn).unwrap().is_none());
    migration::start(&conn, TARGET).unwrap();
    assert_eq!(backfill(&conn), 0);
}

#[test]
fn status_reports_pending_migration_for_configured_model() {
    let _guard = lock();
    let conn = palace::db::open_in_memory().unwrap();
    let status = migration::status(&conn, Some(TARGET)).unwrap();
    assert!(status.migration_pending);
    assert_eq!(status.wanted.as_deref(), Some(TARGET));
    let status = migration::status(&conn, Some(embedding_models::DEFAULT_MODEL_ID)).unwrap();
    assert!(!status.migration_pending);
}

#[test]
fn starting_a_migration_to_the_active_model_fails() {
    let _guard = lock();
    let conn = palace::db::open_in_memory().unwrap();
    assert!(migration::start(&conn, embedding_models::DEFAULT_MODEL_ID).is_err());
    assert!(migration::start(&conn, "no-such-model").is_err());
}

#[test]
fn preference_vectors_follow_the_stored_span() {
    let _guard = lock();
    let dir = tempfile::tempdir().unwrap();
    let conn = seeded_palace(&dir);
    migration::start(&conn, TARGET).unwrap();
    backfill(&conn);
    let pref_id: String = conn
        .query_row(
            "SELECT id FROM drawers WHERE pref_embedding IS NOT NULL LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("the seeded palace has a preference drawer");
    let stored = |conn: &Connection| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM embedding_vectors WHERE owner_kind = 'pref' AND owner_id = ?1",
            [&pref_id],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(stored(&conn), 1);

    conn.execute(
        "UPDATE drawers SET metadata = json_set(metadata, '$.preference_span', 'a re-tagged span')
         WHERE id = ?1",
        [&pref_id],
    )
    .unwrap();
    assert_eq!(
        stored(&conn),
        0,
        "re-tagging drops the stale preference vector"
    );
    assert_eq!(backfill(&conn), 1, "and the backfill rebuilds it");
}
