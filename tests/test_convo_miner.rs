use palace::convo_miner::{mine_convos, ExtractMode};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

/// Same rationale as the equivalent `miner::mine` test: a modest vocabulary
/// gives each chunk a realistic spread of unique BM25 terms, which is exactly
/// what made the old per-term-insert behaviour blow up the commit count.
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

/// Write `file_count` small plain-text "conversation" files, each large
/// enough to produce exactly one chunk with a good spread of unique terms.
fn write_test_convos(dir: &std::path::Path, file_count: usize) {
    for i in 0..file_count {
        let mut content = String::new();
        for j in 0..40 {
            content.push_str(VOCAB[(i * 7 + j) % VOCAB.len()]);
            content.push(' ');
        }
        content.push_str(&format!("conversation number {i} unique marker token{i}."));
        std::fs::write(dir.join(format!("chat_{i:03}.txt")), content).unwrap();
    }
}

// ── Write batching (regression guard, mirrors the `miner::mine` fix) ────────
//
// `mine_convos` had the exact same unbatched per-chunk `add_drawer` write
// loop as `miner::mine`. This test installs a commit counter via
// `commit_hook` and asserts mining batches writes into a handful of
// transactions regardless of how many drawers/terms are produced.
#[test]
fn mine_convos_batches_writes_into_few_transactions() {
    let convo_dir = TempDir::new().unwrap();
    write_test_convos(convo_dir.path(), 25);

    let mut conn = palace::db::open_in_memory().unwrap();
    let commits = Arc::new(AtomicUsize::new(0));
    {
        let counter = commits.clone();
        conn.commit_hook(Some(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            false
        }));
    }

    mine_convos(
        &mut conn,
        convo_dir.path(),
        Some("convo_batch_test"),
        "test",
        0,
        false,
        ExtractMode::Exchange,
    )
    .expect("mine_convos should succeed");

    let total = commits.load(Ordering::SeqCst);
    assert!(
        total <= 5,
        "mining 25 small conversation files should commit in a handful of \
         batched transactions, not one per drawer/term (saw {total} commits)"
    );
}

#[test]
fn mine_convos_dry_run_does_not_write_to_db() {
    let convo_dir = TempDir::new().unwrap();
    write_test_convos(convo_dir.path(), 5);
    let mut conn = palace::db::open_in_memory().unwrap();

    mine_convos(
        &mut conn,
        convo_dir.path(),
        Some("convo_batch_test"),
        "test",
        0,
        true,
        ExtractMode::Exchange,
    )
    .expect("dry run should succeed");

    assert_eq!(
        palace::store::count_drawers(&conn).unwrap(),
        0,
        "dry run must not write any drawers"
    );
}

#[test]
fn mine_convos_rerun_skips_already_mined_files() {
    let convo_dir = TempDir::new().unwrap();
    write_test_convos(convo_dir.path(), 8);
    let mut conn = palace::db::open_in_memory().unwrap();

    mine_convos(
        &mut conn,
        convo_dir.path(),
        Some("convo_batch_test"),
        "test",
        0,
        false,
        ExtractMode::Exchange,
    )
    .unwrap();
    let first_count = palace::store::count_drawers(&conn).unwrap();
    assert!(first_count > 0);

    mine_convos(
        &mut conn,
        convo_dir.path(),
        Some("convo_batch_test"),
        "test",
        0,
        false,
        ExtractMode::Exchange,
    )
    .unwrap();
    let second_count = palace::store::count_drawers(&conn).unwrap();
    assert_eq!(
        second_count, first_count,
        "re-mining the same conversation directory should not duplicate drawers"
    );
}
