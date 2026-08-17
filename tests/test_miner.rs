use palace::miner::{chunk_text, detect_room, mine, CHUNK_SIZE};
use palace::room_detector::{save_config, Room};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

/// A small vocabulary reused across files so each chunk has realistic BM25
/// term overlap (and enough unique terms to make the old per-term-insert
/// behaviour blow up the commit count if the batching regresses).
const VOCAB: &[&str] = &[
    "alpha",
    "bravo",
    "charlie",
    "delta",
    "echo",
    "foxtrot",
    "golf",
    "hotel",
    "india",
    "juliet",
    "kilo",
    "lima",
    "mike",
    "november",
    "oscar",
    "papa",
    "quebec",
    "romeo",
    "sierra",
    "tango",
    "uniform",
    "victor",
    "whiskey",
    "xray",
    "yankee",
    "zulu",
    "memory",
    "palace",
    "drawer",
    "wing",
    "room",
    "search",
    "embedding",
    "vector",
    "database",
    "project",
    "notes",
];

/// Write a minimal mineable project: a `palace.yaml` plus `file_count` small
/// text files, each large enough to produce exactly one chunk with a good
/// spread of unique terms.
fn write_test_project(dir: &Path, file_count: usize) {
    let rooms = vec![Room {
        name: "general".into(),
        description: "general project content".into(),
        keywords: vec![],
    }];
    save_config(dir, "commit_batch_test", &rooms).unwrap();

    for i in 0..file_count {
        let mut content = String::new();
        for j in 0..40 {
            content.push_str(VOCAB[(i * 7 + j) % VOCAB.len()]);
            content.push(' ');
        }
        content.push_str(&format!("file number {i} unique marker token{i}."));
        std::fs::write(dir.join(format!("note_{i:03}.txt")), content).unwrap();
    }
}

#[test]
fn project_wing_status_unknown_for_fresh_dir() {
    use palace::miner::{project_wing_status, ProjectWingStatus};
    let conn = palace::db::open_in_memory().unwrap();
    let tmp = TempDir::new().unwrap();

    let status = project_wing_status(&conn, tmp.path()).unwrap();
    match status {
        ProjectWingStatus::Unknown {
            has_palace_yaml, ..
        } => assert!(!has_palace_yaml),
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn project_wing_status_registered_not_mined() {
    use palace::miner::{project_wing_status, wing_slug_from_dir, ProjectWingStatus};
    let conn = palace::db::open_in_memory().unwrap();
    let tmp = TempDir::new().unwrap();
    let wing = wing_slug_from_dir(tmp.path());
    palace::store::ensure_wing_registered(&conn, &wing).unwrap();

    let status = project_wing_status(&conn, tmp.path()).unwrap();
    assert!(matches!(
        status,
        ProjectWingStatus::RegisteredNotMined { .. }
    ));
}

#[test]
fn project_wing_status_mined_when_drawers_present() {
    use palace::miner::{project_wing_status, wing_slug_from_dir, ProjectWingStatus};
    let conn = palace::db::open_in_memory().unwrap();
    let tmp = TempDir::new().unwrap();
    let wing = wing_slug_from_dir(tmp.path());
    palace::store::add_drawer(
        &conn, &wing, "general", "content", None, "f.txt", 0, "test", 3.0,
    )
    .unwrap();
    palace::store::ensure_wing_registered(&conn, &wing).unwrap();

    let status = project_wing_status(&conn, tmp.path()).unwrap();
    match status {
        ProjectWingStatus::Mined { drawers, .. } => assert_eq!(drawers, 1),
        other => panic!("expected Mined, got {other:?}"),
    }
}

#[test]
fn chunk_text_short_content_produces_one_chunk() {
    // Text must be >= MIN_CHUNK_SIZE (50 chars) to produce a chunk
    let text = "Hello world, this is a somewhat longer sentence that exceeds minimum size.";
    let chunks = chunk_text(text);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].1, 0);
}

#[test]
fn chunk_text_long_content_produces_multiple_chunks() {
    let text = "a".repeat(CHUNK_SIZE * 3);
    let chunks = chunk_text(&text);
    assert!(
        chunks.len() >= 2,
        "long content should produce multiple chunks"
    );
}

#[test]
fn chunk_text_respects_paragraph_breaks() {
    let text = format!("{}\n\n{}", "a".repeat(600), "b".repeat(600));
    let chunks = chunk_text(&text);
    // Should split at paragraph boundary
    assert!(chunks.len() >= 2, "should respect paragraph breaks");
}

#[test]
fn chunk_text_handles_multibyte_at_chunk_boundary() {
    // Place a 3-byte UTF-8 char ('─' = E2 94 80) so that byte index CHUNK_SIZE
    // lands strictly inside it. Without char-boundary handling, slicing panics.
    let prefix = "a".repeat(CHUNK_SIZE - 2);
    let text = format!("{prefix}─{}", "b".repeat(CHUNK_SIZE * 2));
    let chunks = chunk_text(&text);
    assert!(
        chunks.len() >= 2,
        "long multibyte content should still chunk without panicking"
    );
}

#[test]
fn chunk_text_handles_multibyte_at_overlap_rewind() {
    // After a chunk ends cleanly, the next start = cut - CHUNK_OVERLAP.
    // If that lands inside a multibyte char, the next iteration's slice panics.
    // Put '─' so its bytes straddle (CHUNK_SIZE - CHUNK_OVERLAP).
    let head = "a".repeat(CHUNK_SIZE - 100 - 1);
    let text = format!("{head}─{}", "b".repeat(CHUNK_SIZE * 2));
    let chunks = chunk_text(&text);
    assert!(
        !chunks.is_empty(),
        "overlap rewind into multibyte char must not panic"
    );
}

#[test]
fn chunk_text_indices_are_sequential() {
    let text = "w".repeat(CHUNK_SIZE * 4);
    let chunks = chunk_text(&text);
    for (i, (_, idx)) in chunks.iter().enumerate() {
        assert_eq!(*idx, i, "chunk indices should be sequential");
    }
}

#[test]
fn detect_room_uses_folder_path() {
    let rooms = vec![
        Room {
            name: "backend".into(),
            description: "backend code".into(),
            keywords: vec!["api".into()],
        },
        Room {
            name: "frontend".into(),
            description: "ui code".into(),
            keywords: vec!["ui".into()],
        },
        Room {
            name: "general".into(),
            description: "other".into(),
            keywords: vec![],
        },
    ];
    let project = Path::new("/project");
    let path = Path::new("/project/backend/server.py");
    let room = detect_room(path, "some content", &rooms, project);
    assert_eq!(room, "backend");
}

#[test]
fn detect_room_keyword_scoring() {
    let rooms = vec![
        Room {
            name: "database".into(),
            description: "db".into(),
            keywords: vec!["sql".into(), "schema".into()],
        },
        Room {
            name: "general".into(),
            description: "other".into(),
            keywords: vec![],
        },
    ];
    let project = Path::new("/project");
    let path = Path::new("/project/config.txt");
    let content = "CREATE TABLE users (id INT); ALTER TABLE schema.sql;";
    let room = detect_room(path, content, &rooms, project);
    assert_eq!(room, "database");
}

#[test]
fn detect_room_defaults_to_general() {
    let rooms = vec![
        Room {
            name: "backend".into(),
            description: "backend".into(),
            keywords: vec!["rust".into()],
        },
        Room {
            name: "general".into(),
            description: "other".into(),
            keywords: vec![],
        },
    ];
    let project = Path::new("/project");
    let path = Path::new("/project/data.csv");
    let room = detect_room(path, "some random content xyz", &rooms, project);
    assert_eq!(room, "general");
}

// ── Write batching (regression guard for the "mine hangs on bigger projects"
// fix) ────────────────────────────────────────────────────────────────────
//
// Before the fix, every drawer insert plus every unique BM25 term insert was
// its own autocommit transaction. A project with just a few dozen files could
// trigger hundreds of individual commits. This test installs a commit
// counter via `commit_hook` and asserts mining batches writes into a handful
// of transactions regardless of how many drawers/terms are produced.
#[test]
fn mine_batches_writes_into_few_transactions() {
    let project = TempDir::new().unwrap();
    write_test_project(project.path(), 25);

    let mut conn = palace::db::open_in_memory().unwrap();
    let commits = Arc::new(AtomicUsize::new(0));
    {
        let counter = commits.clone();
        conn.commit_hook(Some(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            false
        }));
    }

    mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .expect("mine should succeed");

    let total = commits.load(Ordering::SeqCst);
    assert!(
        total <= 5,
        "mining 25 small files should commit in a handful of batched \
         transactions, not one per drawer/term (saw {total} commits)"
    );
}

#[test]
fn mine_dry_run_does_not_write_to_db() {
    let project = TempDir::new().unwrap();
    write_test_project(project.path(), 5);
    let mut conn = palace::db::open_in_memory().unwrap();

    mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        true,
        true,
        &[],
        true,
        None,
    )
    .expect("dry run should succeed");

    assert_eq!(
        palace::store::count_drawers(&conn).unwrap(),
        0,
        "dry run must not write any drawers"
    );
}

#[test]
fn mine_records_one_drawer_per_file_and_marks_wing_mined() {
    let project = TempDir::new().unwrap();
    write_test_project(project.path(), 12);
    let mut conn = palace::db::open_in_memory().unwrap();

    mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .expect("mine should succeed");

    assert_eq!(palace::store::count_drawers(&conn).unwrap(), 12);
    let rooms = palace::store::room_counts(&conn, Some("commit_batch_test")).unwrap();
    assert_eq!(rooms.get("general").copied(), Some(12));

    use palace::miner::{project_wing_status, ProjectWingStatus};
    let status = project_wing_status(&conn, project.path()).unwrap();
    match status {
        ProjectWingStatus::Mined { drawers, .. } => assert_eq!(drawers, 12),
        other => panic!("expected Mined, got {other:?}"),
    }
}

#[test]
fn mine_rerun_skips_already_mined_files() {
    let project = TempDir::new().unwrap();
    write_test_project(project.path(), 8);
    let mut conn = palace::db::open_in_memory().unwrap();

    mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .unwrap();
    let first_count = palace::store::count_drawers(&conn).unwrap();
    assert_eq!(first_count, 8);

    mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .unwrap();
    let second_count = palace::store::count_drawers(&conn).unwrap();
    assert_eq!(
        second_count, first_count,
        "re-mining the same project should not duplicate drawers"
    );
}

// ── Progress reporting (MCP liveness fix) ───────────────────────────────────
//
// `palace_mine` used to give zero feedback for the entire duration of a long
// mine, which is exactly what made it look "stuck" to an MCP client. `mine`
// now accepts an optional progress callback invoked as each file is written.
#[test]
fn mine_reports_progress_after_each_file() {
    let project = TempDir::new().unwrap();
    write_test_project(project.path(), 6);
    let mut conn = palace::db::open_in_memory().unwrap();

    let mut seen: Vec<(usize, usize)> = Vec::new();
    {
        let mut on_progress = |done: usize, total: usize| seen.push((done, total));
        mine(
            &mut conn,
            project.path(),
            None,
            "test",
            0,
            false,
            true,
            &[],
            true,
            Some(&mut on_progress),
        )
        .expect("mine should succeed");
    }

    assert_eq!(
        seen.len(),
        6,
        "expected one progress callback per file, got {seen:?}"
    );
    for (i, &(done, total)) in seen.iter().enumerate() {
        assert_eq!(done, i + 1, "progress should increase monotonically");
        assert_eq!(total, 6, "total should be the full file count");
    }
}

// ── Incremental re-mining ("mine as sync") ──────────────────────────────────

#[test]
fn mine_rerun_updates_content_when_file_changes() {
    let project = TempDir::new().unwrap();
    write_test_project(project.path(), 3);
    let mut conn = palace::db::open_in_memory().unwrap();

    let summary = mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .unwrap();
    assert_eq!(summary.new_files, 3);
    assert_eq!(summary.updated_files, 0);

    let edited_path = project.path().join("note_000.txt");
    let edited_canonical = edited_path
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    std::fs::write(
        &edited_path,
        "completely different content for the regression test on edits, unique marker replaced.",
    )
    .unwrap();

    let summary2 = mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .unwrap();
    assert_eq!(
        summary2.updated_files, 1,
        "the edited file should be classified as updated"
    );
    assert_eq!(
        summary2.unchanged_files, 2,
        "untouched files should be reported as unchanged"
    );
    assert_eq!(summary2.new_files, 0);

    let drawers =
        palace::store::list_drawers(&conn, &palace::store::DrawerFilter::default(), 100).unwrap();
    let edited_drawer = drawers
        .iter()
        .find(|d| d.source_file == edited_canonical)
        .expect("edited file should still have a drawer");
    assert!(
        edited_drawer
            .content
            .contains("completely different content"),
        "drawer content should reflect the edit, not the stale original"
    );
}

#[test]
fn mine_rerun_removes_drawers_for_deleted_file() {
    let project = TempDir::new().unwrap();
    write_test_project(project.path(), 3);
    let mut conn = palace::db::open_in_memory().unwrap();

    mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .unwrap();
    assert_eq!(palace::store::count_drawers(&conn).unwrap(), 3);

    let removed_path = project.path().join("note_001.txt");
    let removed_canonical = removed_path
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    std::fs::remove_file(&removed_path).unwrap();

    let summary = mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .unwrap();
    assert_eq!(summary.removed_files, 1);
    assert_eq!(summary.unchanged_files, 2);
    assert_eq!(palace::store::count_drawers(&conn).unwrap(), 2);
    assert!(
        palace::store::get_mined_file_hash(&conn, &removed_canonical)
            .unwrap()
            .is_none()
    );

    let bm25_doc_stats: i64 = conn
        .query_row("SELECT COUNT(*) FROM bm25_doc_stats", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        bm25_doc_stats,
        palace::store::count_drawers(&conn).unwrap(),
        "bm25 doc stats should track exactly the surviving drawers"
    );
}

#[test]
fn mine_rerun_removes_stale_chunks_when_file_shrinks() {
    let project = TempDir::new().unwrap();
    let rooms = vec![Room {
        name: "general".into(),
        description: "general project content".into(),
        keywords: vec![],
    }];
    save_config(project.path(), "shrink_test", &rooms).unwrap();

    let big_content = "alpha bravo charlie delta echo foxtrot golf hotel india juliet ".repeat(60);
    let file_path = project.path().join("big.txt");
    std::fs::write(&file_path, &big_content).unwrap();

    let mut conn = palace::db::open_in_memory().unwrap();
    mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .unwrap();
    let before = palace::store::count_drawers(&conn).unwrap();
    assert!(before >= 2, "a big file should chunk into multiple drawers");

    std::fs::write(
        &file_path,
        "alpha bravo charlie delta echo unique marker only one small chunk remains.",
    )
    .unwrap();

    let summary = mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .unwrap();
    assert_eq!(summary.updated_files, 1);
    let after = palace::store::count_drawers(&conn).unwrap();
    assert_eq!(
        after, 1,
        "shrinking the file should drop the now-stale extra chunks"
    );
}

#[test]
fn mine_rerun_with_no_changes_does_not_rewrite_drawers() {
    let project = TempDir::new().unwrap();
    write_test_project(project.path(), 5);
    let mut conn = palace::db::open_in_memory().unwrap();

    mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .unwrap();

    let commits = Arc::new(AtomicUsize::new(0));
    {
        let counter = commits.clone();
        conn.commit_hook(Some(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            false
        }));
    }

    let summary = mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .unwrap();
    assert_eq!(summary.new_files, 0);
    assert_eq!(summary.updated_files, 0);
    assert_eq!(summary.unchanged_files, 5);
    assert_eq!(summary.drawers_added, 0);

    let total_commits = commits.load(Ordering::SeqCst);
    assert!(
        total_commits <= 1,
        "re-mining with zero content changes must not open a per-file write \
         transaction (saw {total_commits} commits, only the wing-mined \
         bookkeeping write is expected)"
    );
}

#[test]
fn mine_does_not_delete_drawers_for_files_still_on_disk_when_limited() {
    let project = TempDir::new().unwrap();
    write_test_project(project.path(), 6);
    let mut conn = palace::db::open_in_memory().unwrap();

    // First mine everything.
    mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .unwrap();
    assert_eq!(palace::store::count_drawers(&conn).unwrap(), 6);

    // Re-mine with a limit that only walks a subset of files. Files outside
    // the limited walk still exist on disk, so their drawers must survive.
    let summary = mine(
        &mut conn,
        project.path(),
        None,
        "test",
        2,
        false,
        true,
        &[],
        true,
        None,
    )
    .unwrap();
    assert_eq!(
        summary.removed_files, 0,
        "--limit must never be mistaken for mass deletion of files still on disk"
    );
    assert_eq!(
        palace::store::count_drawers(&conn).unwrap(),
        6,
        "drawers for files outside the limited walk must be untouched"
    );
}

#[test]
fn mine_without_progress_callback_still_succeeds() {
    let project = TempDir::new().unwrap();
    write_test_project(project.path(), 3);
    let mut conn = palace::db::open_in_memory().unwrap();

    mine(
        &mut conn,
        project.path(),
        None,
        "test",
        0,
        false,
        true,
        &[],
        true,
        None,
    )
    .expect("mine should succeed without a progress callback");
    assert_eq!(palace::store::count_drawers(&conn).unwrap(), 3);
}
