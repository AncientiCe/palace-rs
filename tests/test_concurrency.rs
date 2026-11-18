//! Several palace processes share one `palace.db`. These tests use a real
//! database file, because SQLite locking does not exist for `:memory:`.

use palace::convo_miner::{mine_convos, ExtractMode};
use palace::db;
use palace::miner::mine;
use palace::room_detector::{save_config, Room};
use rusqlite::Connection;
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const NOTE: &str = "The palace keeps drawers in wings and rooms so an assistant can find \
    earlier decisions again. This note is long enough to become one chunk when the miner \
    reads it, and it mentions sqlite, locking, timeouts and write transactions.";

const CHAT: &str = "> How do we stop the database from reporting that it is locked?\n\
    Set a busy timeout, keep write transactions short, and skip writes when a palace is \
    already up to date. That way a second process waits briefly instead of failing.\n";

#[test]
fn open_succeeds_while_another_connection_holds_the_write_lock() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("palace.db");
    drop(db::open(&db_path).unwrap());

    let writer = Connection::open(&db_path).unwrap();
    writer
        .execute_batch("BEGIN IMMEDIATE; INSERT INTO meta (key, value) VALUES ('held', '1');")
        .unwrap();

    let opened = db::open(&db_path);

    assert!(
        opened.is_ok(),
        "open should not need the write lock: {:?}",
        opened.err()
    );
}

#[test]
fn opening_an_up_to_date_database_changes_no_rows() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("palace.db");
    drop(db::open(&db_path).unwrap());

    let conn = db::open(&db_path).unwrap();

    assert_eq!(conn.total_changes(), 0);
}

#[test]
fn open_waits_for_a_writer_that_commits_within_the_timeout() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("palace.db");
    let setup = db::open(&db_path).unwrap();
    // A drawer whose wing is not registered yet, so the next open has to write.
    setup
        .execute(
            "INSERT INTO drawers (id, wing, room, content) VALUES ('d1', 'latecomer', 'r', 'c')",
            [],
        )
        .unwrap();
    drop(setup);

    let (locked_tx, locked_rx) = mpsc::channel();
    let writer_path = db_path.clone();
    let writer = thread::spawn(move || {
        let writer = Connection::open(&writer_path).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE;").unwrap();
        locked_tx.send(()).unwrap();
        thread::sleep(Duration::from_millis(300));
        writer.execute_batch("COMMIT;").unwrap();
    });
    locked_rx.recv().unwrap();

    let conn = db::open(&db_path).expect("open should wait for the writer, not fail");
    writer.join().unwrap();

    let registered: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM wings WHERE name = 'latecomer'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(registered, 1);
}

#[test]
fn palace_starts_when_the_wings_backfill_stays_locked_past_the_timeout() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("palace.db");
    let setup = db::open(&db_path).unwrap();
    setup
        .execute(
            "INSERT INTO drawers (id, wing, room, content) VALUES ('d1', 'latecomer', 'r', 'c')",
            [],
        )
        .unwrap();
    drop(setup);
    let writer = Connection::open(&db_path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE;").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_palace"))
        .arg("status")
        .env("HOME", tmp.path())
        .env("PALACE_PALACE_PATH", tmp.path())
        .env("PALACE_BUSY_TIMEOUT_MS", "200")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn palace_reports_the_other_process_when_a_needed_migration_stays_locked() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("palace.db");
    // A database without the palace schema, so open has to migrate it.
    let writer = Connection::open(&db_path).unwrap();
    writer
        .execute_batch("PRAGMA journal_mode=WAL; BEGIN IMMEDIATE; CREATE TABLE held (id INTEGER);")
        .unwrap();

    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_palace"))
        .arg("status")
        .env("HOME", tmp.path())
        .env("PALACE_PALACE_PATH", tmp.path())
        .env("PALACE_BUSY_TIMEOUT_MS", "200")
        .output()
        .unwrap();
    let elapsed = started.elapsed();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("another palace process"),
        "stderr: {stderr}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "should give up after the 200 ms timeout, took {elapsed:?}"
    );
}

#[test]
fn two_concurrent_mines_into_one_database_both_succeed() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("palace.db");
    drop(db::open(&db_path).unwrap());

    let project_a = TempDir::new().unwrap();
    save_config(
        project_a.path(),
        "project_a",
        &[Room {
            name: "general".into(),
            description: "general".into(),
            keywords: vec![],
        }],
    )
    .unwrap();
    std::fs::write(project_a.path().join("one.txt"), format!("{NOTE} a-one")).unwrap();
    std::fs::write(project_a.path().join("two.txt"), format!("{NOTE} a-two")).unwrap();
    std::fs::write(
        project_a.path().join("three.txt"),
        format!("{NOTE} a-three"),
    )
    .unwrap();

    let project_b = TempDir::new().unwrap();
    save_config(
        project_b.path(),
        "project_b",
        &[Room {
            name: "general".into(),
            description: "general".into(),
            keywords: vec![],
        }],
    )
    .unwrap();
    std::fs::write(project_b.path().join("one.txt"), format!("{NOTE} b-one")).unwrap();
    std::fs::write(project_b.path().join("two.txt"), format!("{NOTE} b-two")).unwrap();

    let start = Arc::new(Barrier::new(2));

    let start_a = start.clone();
    let db_a = db_path.clone();
    let dir_a = project_a.path().to_path_buf();
    let mine_a = thread::spawn(move || {
        let mut conn = db::open(&db_a)?;
        start_a.wait();
        mine(
            &mut conn,
            &dir_a,
            None,
            "test",
            0,
            false,
            true,
            &[],
            true,
            None,
        )
    });

    let start_b = start.clone();
    let db_b = db_path.clone();
    let dir_b = project_b.path().to_path_buf();
    let mine_b = thread::spawn(move || {
        let mut conn = db::open(&db_b)?;
        start_b.wait();
        mine(
            &mut conn,
            &dir_b,
            None,
            "test",
            0,
            false,
            true,
            &[],
            true,
            None,
        )
    });

    let summary_a = mine_a
        .join()
        .unwrap()
        .expect("mine of project_a should succeed");
    let summary_b = mine_b
        .join()
        .unwrap()
        .expect("mine of project_b should succeed");

    assert_eq!(summary_a.drawers_added, 3);
    assert_eq!(summary_b.drawers_added, 2);
    let conn = db::open(&db_path).unwrap();
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM drawers", [], |row| row.get(0))
        .unwrap();
    assert_eq!(total, 5);
}

#[test]
fn mine_convos_succeeds_while_a_project_mine_runs() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("palace.db");
    drop(db::open(&db_path).unwrap());

    let project = TempDir::new().unwrap();
    save_config(
        project.path(),
        "project_a",
        &[Room {
            name: "general".into(),
            description: "general".into(),
            keywords: vec![],
        }],
    )
    .unwrap();
    std::fs::write(project.path().join("one.txt"), format!("{NOTE} one")).unwrap();
    std::fs::write(project.path().join("two.txt"), format!("{NOTE} two")).unwrap();

    let convos = TempDir::new().unwrap();
    std::fs::write(convos.path().join("chat_one.txt"), format!("{CHAT} first")).unwrap();
    std::fs::write(convos.path().join("chat_two.txt"), format!("{CHAT} second")).unwrap();

    let start = Arc::new(Barrier::new(2));

    let start_project = start.clone();
    let db_project = db_path.clone();
    let project_dir = project.path().to_path_buf();
    let project_mine = thread::spawn(move || {
        let mut conn = db::open(&db_project)?;
        start_project.wait();
        mine(
            &mut conn,
            &project_dir,
            None,
            "test",
            0,
            false,
            true,
            &[],
            true,
            None,
        )
    });

    let start_convos = start.clone();
    let db_convos = db_path.clone();
    let convo_dir = convos.path().to_path_buf();
    let convo_mine = thread::spawn(move || {
        let mut conn = db::open(&db_convos)?;
        start_convos.wait();
        mine_convos(
            &mut conn,
            &convo_dir,
            Some("chats"),
            "test",
            0,
            false,
            ExtractMode::Exchange,
        )
    });

    project_mine
        .join()
        .unwrap()
        .expect("project mine should succeed");
    convo_mine
        .join()
        .unwrap()
        .expect("mine_convos should succeed");

    let conn = db::open(&db_path).unwrap();
    let project_drawers: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM drawers WHERE wing = 'project_a'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let chat_drawers: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM drawers WHERE wing = 'chats'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(project_drawers, 2);
    assert_eq!(chat_drawers, 2);
}
