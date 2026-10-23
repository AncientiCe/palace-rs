//! Embedding model bake-off: retrieval quality and speed per model.
//!
//! Skipped unless `PALACE_RUN_BAKEOFF=1`; run with
//! `cargo test --release --test embedding_bakeoff -- --nocapture`.
//!
//! - `PALACE_BAKEOFF_MODELS`: comma-separated ids (default: every model).
//! - `PALACE_BAKEOFF_DB`: a palace.db opened read-only. Its drawers give the
//!   speed corpus and known-item probes; without it only the coding-agent
//!   eval and a synthetic speed corpus run.
//! - `PALACE_BAKEOFF_TUNE=1`: also score a grid of cosine floors, cosine
//!   weights and query-prefix on/off, to tune a model's ranking settings.
//!
//! Quality: the coding-agent eval (hybrid and vector-only recall@1/@5) and,
//! with a palace, known-item probes. Speed: drawers/s through
//! `embed_documents`, short-query latency p50/p95, and model load time.

mod common;

use common::coding_eval::{DOCS, QUESTIONS};
use palace::embedding_eval::{known_item_probe, recall, Recall, Rng};
use palace::embedding_models::{self, ModelSpec};
use palace::ranker::hybrid_search_in;
use palace::store::{add_drawer, vector_search_in, DrawerFilter, VectorSource};
use rusqlite::Connection;
use std::time::{Duration, Instant};

const CORPUS: usize = 400;
const PROBES: usize = 200;
const SEED: u64 = 4040;

fn enabled(var: &str) -> bool {
    std::env::var(var).ok().as_deref() == Some("1")
}

fn models() -> Vec<&'static ModelSpec> {
    match std::env::var("PALACE_BAKEOFF_MODELS") {
        Ok(list) => list
            .split(',')
            .map(|id| embedding_models::find(id).unwrap_or_else(|| panic!("unknown model {id}")))
            .collect(),
        Err(_) => embedding_models::MODELS.iter().collect(),
    }
}

/// `(id, content)` pairs from the bake-off palace, or a synthetic corpus.
fn corpus() -> Vec<(String, String)> {
    if let Ok(path) = std::env::var("PALACE_BAKEOFF_DB") {
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open bake-off palace read-only");
        let ids: Vec<String> = conn
            .prepare("SELECT id FROM drawers ORDER BY id")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| row.get(0))?
                    .collect::<Result<Vec<String>, _>>()
            })
            .expect("list drawer ids");
        return Rng::new(SEED)
            .sample_indices(ids.len(), CORPUS)
            .into_iter()
            .map(|i| {
                let content: String = conn
                    .query_row(
                        "SELECT content FROM drawers WHERE id = ?1",
                        [&ids[i]],
                        |row| row.get(0),
                    )
                    .expect("read drawer");
                (ids[i].clone(), content)
            })
            .collect();
    }
    let sentence = "Palace keeps verbatim drawers and ranks them with BM25 plus cosine. ";
    (0..CORPUS)
        .map(|i| {
            (
                format!("synthetic-{i}"),
                sentence.repeat(1 + (i * 7919) % 40),
            )
        })
        .collect()
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

/// A palace embedded with one model, plus the questions to ask it.
/// `cases` are `(query, gold source_file)`.
struct Bench {
    conn: Connection,
    cases: Vec<(String, String)>,
}

fn build(
    spec: &'static ModelSpec,
    docs: &[(String, &str, String)],
    cases: Vec<(String, String)>,
) -> Bench {
    let conn = palace::db::open_in_memory().expect("db");
    let texts: Vec<&str> = docs.iter().map(|(_, _, text)| text.as_str()).collect();
    let vectors = palace::embedder::embed_documents_with(spec, &texts).expect("embed docs");
    for (index, ((source, room, text), vector)) in docs.iter().zip(&vectors).enumerate() {
        add_drawer(
            &conn,
            "bench",
            room,
            text,
            Some(vector),
            source,
            index,
            "bench",
            3.0,
        )
        .expect("insert drawer");
    }
    Bench { conn, cases }
}

fn coding_bench(spec: &'static ModelSpec) -> Bench {
    let docs: Vec<(String, &str, String)> = DOCS
        .iter()
        .map(|doc| (doc.id.to_string(), doc.room, doc.text.to_string()))
        .collect();
    let cases = QUESTIONS
        .iter()
        .map(|q| (q.query.to_string(), q.gold_source.to_string()))
        .collect();
    build(spec, &docs, cases)
}

fn probe_bench(spec: &'static ModelSpec, corpus: &[(String, String)]) -> Bench {
    let docs: Vec<(String, &str, String)> = corpus
        .iter()
        .map(|(id, content)| (id.clone(), "probe", content.clone()))
        .collect();
    let mut rng = Rng::new(SEED);
    let cases = corpus
        .iter()
        .filter_map(|(id, content)| known_item_probe(content, &mut rng).map(|p| (p, id.clone())))
        .take(PROBES)
        .collect();
    build(spec, &docs, cases)
}

/// `(hybrid, vector-only)` recall for queries embedded with `query_model`
/// and ranked with `ranking`'s floor and weights.
fn score(bench: &Bench, query_model: &'static ModelSpec, ranking: &ModelSpec) -> (Recall, Recall) {
    let mut hybrid = Vec::new();
    let mut vector_only = Vec::new();
    for (query, gold) in &bench.cases {
        let qvec = palace::embedder::embed_query_with(query_model, query).expect("embed query");
        let ranked = hybrid_search_in(
            &bench.conn,
            query,
            Some(&qvec),
            VectorSource::Hot,
            ranking,
            &DrawerFilter::default(),
            5,
        )
        .expect("hybrid search");
        hybrid.push((
            gold.as_str(),
            ranked.into_iter().map(|r| r.drawer.source_file).collect(),
        ));
        let ranked = vector_search_in(
            &bench.conn,
            &qvec,
            VectorSource::Hot,
            &DrawerFilter::default(),
            5,
        )
        .expect("vector search");
        vector_only.push((
            gold.as_str(),
            ranked.into_iter().map(|r| r.source_file).collect(),
        ));
    }
    (recall(hybrid), recall(vector_only))
}

fn speed(spec: &'static ModelSpec, corpus: &[(String, String)]) -> (f64, Duration, Duration) {
    let texts: Vec<&str> = corpus.iter().map(|(_, content)| content.as_str()).collect();
    let start = Instant::now();
    palace::embedder::embed_documents_with(spec, &texts).expect("embed corpus");
    let docs_per_sec = texts.len() as f64 / start.elapsed().as_secs_f64();
    let mut times: Vec<Duration> = (0..50)
        .map(|i| {
            let start = Instant::now();
            palace::embedder::embed_query_with(spec, &format!("why did we pick option {i}?"))
                .expect("query");
            start.elapsed()
        })
        .collect();
    times.sort();
    (
        docs_per_sec,
        percentile(&times, 0.5),
        percentile(&times, 0.95),
    )
}

fn fmt_recall(r: Option<Recall>) -> String {
    r.map(|r| format!("{:.3}/{:.3}", r.at_1, r.at_5))
        .unwrap_or_else(|| "-".to_string())
}

/// Score ranking variants of `spec` on the prebuilt benches.
fn tune(spec: &'static ModelSpec, coding: &Bench, probes: Option<&Bench>) {
    let no_prefix: &'static ModelSpec = Box::leak(Box::new(ModelSpec {
        query_prefix: "",
        ..*spec
    }));
    let prefixes: Vec<(&str, &'static ModelSpec)> = if spec.query_prefix.is_empty() {
        vec![("-", spec)]
    } else {
        vec![("prefix", spec), ("no-prefix", no_prefix)]
    };
    for (label, query_model) in prefixes {
        for floor in [0.0, 0.2, 0.3, 0.4, 0.5] {
            for weight in [0.5, 0.65, 0.8] {
                let ranking = ModelSpec {
                    cosine_floor: floor,
                    cosine_weight: weight,
                    bm25_weight: 1.0 - weight,
                    ..*spec
                };
                let (eval, _) = score(coding, query_model, &ranking);
                let probe = probes.map(|bench| score(bench, query_model, &ranking).0);
                eprintln!(
                    "  tune {:<14} {label:<9} floor {floor:.1} cos {weight:.2} | eval {} | probes {}",
                    spec.id,
                    fmt_recall(Some(eval)),
                    fmt_recall(probe),
                );
            }
        }
    }
}

#[test]
fn embedding_model_bakeoff() {
    if !enabled("PALACE_RUN_BAKEOFF") {
        eprintln!("PALACE_RUN_BAKEOFF is not set; embedding bake-off skipped.");
        return;
    }
    let corpus = corpus();
    let real_palace = std::env::var("PALACE_BAKEOFF_DB").is_ok();
    for spec in models() {
        // Load (and download) first so speed numbers exclude it.
        let cached = embedding_models::is_cached(spec);
        let start = Instant::now();
        palace::embedder::load_model(spec).expect("download/load model");
        let load = start.elapsed();
        if !cached {
            eprintln!("{}: downloaded and loaded in {load:.2?}", spec.id);
        }

        let coding = coding_bench(spec);
        let probes = real_palace.then(|| probe_bench(spec, &corpus));
        let (eval_hybrid, eval_vector) = score(&coding, spec, spec);
        let probe = probes.as_ref().map(|bench| score(bench, spec, spec));
        let (docs_per_sec, query_p50, query_p95) = speed(spec, &corpus);
        eprintln!(
            "{:<16} eval hybrid {} vector {} | probes hybrid {} vector {} | {:>6.1} docs/s | query p50 {:>8.2?} p95 {:>8.2?} | load {:.2?}",
            spec.id,
            fmt_recall(Some(eval_hybrid)),
            fmt_recall(Some(eval_vector)),
            fmt_recall(probe.map(|p| p.0)),
            fmt_recall(probe.map(|p| p.1)),
            docs_per_sec,
            query_p50,
            query_p95,
            load,
        );
        if enabled("PALACE_BAKEOFF_TUNE") {
            tune(spec, &coding, probes.as_ref());
        }
    }
    eprintln!("recall shown as @1/@5; corpus {} drawers", corpus.len());
}
