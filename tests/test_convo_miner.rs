use palace::convo_miner::{mine_convos, mine_convos_with, ExtractMode};
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

#[test]
fn mine_convos_force_replaces_drawers_of_already_mined_files() {
    let convo_dir = TempDir::new().unwrap();
    write_test_convos(convo_dir.path(), 3);
    let mut conn = palace::db::open_in_memory().unwrap();
    let mine = |conn: &mut rusqlite::Connection, force: bool| {
        mine_convos_with(
            conn,
            convo_dir.path(),
            Some("convo_force_test"),
            "test",
            0,
            false,
            ExtractMode::Exchange,
            force,
        )
        .unwrap();
    };

    mine(&mut conn, false);
    let first_count = palace::store::count_drawers(&conn).unwrap();

    // A file edited after mining stays stale without --force.
    let edited = convo_dir.path().join("chat_000.txt");
    std::fs::write(
        &edited,
        "Edited conversation about a replacement marker named zebrafinch, long \
         enough to be chunked into a drawer of its own.",
    )
    .unwrap();
    mine(&mut conn, false);
    let has_marker = |conn: &rusqlite::Connection| -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM drawers WHERE content LIKE '%zebrafinch%'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
            > 0
    };
    assert!(!has_marker(&conn), "plain re-mine must skip mined files");

    mine(&mut conn, true);
    assert!(has_marker(&conn), "--force must re-chunk mined files");
    assert_eq!(
        palace::store::count_drawers(&conn).unwrap(),
        first_count,
        "--force must replace drawers, not add duplicates"
    );
}

#[test]
fn chunk_exchanges_keeps_every_line_of_a_long_reply() {
    // The reply has 20 lines; exchange chunking used to keep the first 8 and
    // silently drop the rest.
    let reply: Vec<String> = (1..=20)
        .map(|n| format!("Step {n}: whisk ingredient number {n} into the basil sauce."))
        .collect();
    let transcript = format!(
        "> How do I make basil sauce?\n{}\n\n> Thanks, and the mint dip?\nBlend mint with yogurt.\n\n> And dessert?\nServe fresh berries.\n",
        reply.join("\n")
    );

    let chunks = palace::convo_miner::chunk_exchanges(&transcript);
    let all_text: String = chunks.iter().map(|(text, _)| text.as_str()).collect();

    for line in &reply {
        assert!(all_text.contains(line.as_str()), "lost reply line: {line}");
    }
    assert!(all_text.contains("Blend mint with yogurt."));
    for (text, _) in &chunks {
        assert!(
            text.starts_with("> "),
            "each chunk keeps its user turn: {text}"
        );
    }
}

#[test]
fn chunk_exchanges_splits_a_reply_too_long_to_embed() {
    let reply = "Use fresh basil leaves and good olive oil for this sauce. ".repeat(80);
    let transcript = format!(
        "> How do I make basil sauce?\n{reply}\n\n> Next?\nBlend mint with yogurt.\n\n> And dessert?\nServe fresh berries.\n"
    );

    let chunks = palace::convo_miner::chunk_exchanges(&transcript);
    let basil: Vec<&String> = chunks
        .iter()
        .map(|(text, _)| text)
        .filter(|text| text.starts_with("> How do I make basil sauce?"))
        .collect();

    assert!(
        basil.len() > 1,
        "a ~4,700-char reply should span several chunks"
    );
    let kept: usize = basil
        .iter()
        .map(|text| text.matches("fresh basil leaves").count())
        .sum();
    assert_eq!(kept, 80, "every sentence of the reply is kept exactly once");
}
