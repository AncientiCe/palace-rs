//! Embedding speed bake-off. Skipped unless `PALACE_RUN_BAKEOFF=1`; run with
//! `cargo test --release --test embedding_bakeoff -- --nocapture`.
//!
//! Texts come from a palace when `PALACE_BAKEOFF_DB` points at one (opened
//! read-only), otherwise from a synthetic mixed-length corpus.

use std::time::{Duration, Instant};

const SAMPLE: usize = 400;

fn enabled() -> bool {
    std::env::var("PALACE_RUN_BAKEOFF").ok().as_deref() == Some("1")
}

fn corpus() -> Vec<String> {
    if let Ok(path) = std::env::var("PALACE_BAKEOFF_DB") {
        let conn =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("open bake-off palace read-only");
        let mut stmt = conn
            .prepare("SELECT content FROM drawers ORDER BY id LIMIT ?1")
            .expect("prepare corpus query");
        return stmt
            .query_map([SAMPLE as i64], |row| row.get(0))
            .expect("query corpus")
            .filter_map(Result::ok)
            .collect();
    }
    let sentence = "Palace keeps verbatim drawers and ranks them with BM25 plus cosine. ";
    (0..SAMPLE)
        .map(|i| sentence.repeat(1 + (i * 7919) % 40))
        .collect()
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

#[test]
fn embedding_speed_bakeoff() {
    if !enabled() {
        eprintln!("PALACE_RUN_BAKEOFF is not set; embedding bake-off skipped.");
        return;
    }
    let texts = corpus();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    palace::embedder::embed_one("warm up").expect("warm up");

    let start = Instant::now();
    for text in &refs {
        palace::embedder::embed_one(text).expect("embed_one");
    }
    let single = start.elapsed();

    let start = Instant::now();
    palace::embedder::embed_batch(&refs).expect("embed_batch");
    let batch = start.elapsed();

    let mut query_times: Vec<Duration> = (0..50)
        .map(|i| {
            let start = Instant::now();
            palace::embedder::embed_one(&format!("why did we pick option {i}?")).expect("query");
            start.elapsed()
        })
        .collect();
    query_times.sort();

    let n = refs.len() as f64;
    eprintln!(
        "bake-off: {} texts | single {:.1} docs/s | batch {:.1} docs/s | query p50 {:?} p95 {:?}",
        refs.len(),
        n / single.as_secs_f64(),
        n / batch.as_secs_f64(),
        percentile(&query_times, 0.5),
        percentile(&query_times, 0.95),
    );
}
