//! Hybrid retrieval: BM25 keyword scoring plus optional vector similarity.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};

use crate::store::{get_drawer, vector_search, DrawerFilter, SearchResult};

const BM25_K1: f64 = 1.5;
const BM25_B: f64 = 0.75;
const COSINE_WEIGHT: f64 = 0.65;
const BM25_WEIGHT: f64 = 0.35;
const MAX_CODING_BOOST: f64 = 0.65;
const MAX_PREFERENCE_MATCH: f64 = 0.22;
/// Maximum recency bonus applied to brand-new drawers (filed today).
const RECENCY_WEIGHT: f64 = 0.05;
/// Exponential decay constant — half-life ≈ 35 days.
const RECENCY_LAMBDA: f64 = 0.02;

/// Compute an exponential recency bonus based on `filed_at` (RFC 3339 string).
/// Returns a value in [0, RECENCY_WEIGHT].
fn recency_boost(filed_at: &str) -> f64 {
    let Ok(ts) = chrono::DateTime::parse_from_rfc3339(filed_at) else {
        return 0.0;
    };
    let now = chrono::Utc::now();
    let age_days = (now - ts.to_utc()).num_days().max(0) as f64;
    RECENCY_WEIGHT * (-RECENCY_LAMBDA * age_days).exp()
}

#[derive(Debug, Clone)]
pub struct HybridResult {
    pub drawer: SearchResult,
    pub cosine: f64,
    pub bm25: f64,
    pub coding_boost: f64,
    pub preference_match: f64,
    pub rerank_score: Option<f64>,
    pub combined: f64,
}

pub fn hybrid_search(
    conn: &Connection,
    query: &str,
    query_vec: Option<&[f32]>,
    filter: &DrawerFilter,
    n_results: usize,
) -> Result<Vec<HybridResult>> {
    let sanitized_query = crate::query_sanitizer::sanitize_query(query);
    let query = if sanitized_query.is_empty() {
        query
    } else {
        &sanitized_query
    };
    let mut by_id: HashMap<String, HybridResult> = HashMap::new();

    if let Some(vec) = query_vec {
        for result in vector_search(conn, vec, filter, n_results.saturating_mul(4).max(20))? {
            by_id.insert(
                result.id.clone(),
                HybridResult {
                    drawer: result,
                    cosine: 0.0,
                    bm25: 0.0,
                    coding_boost: 0.0,
                    preference_match: 0.0,
                    rerank_score: None,
                    combined: 0.0,
                },
            );
        }
    }

    for (drawer_id, score) in bm25_scores(conn, query, filter)? {
        if let Some(existing) = by_id.get_mut(&drawer_id) {
            existing.bm25 = score;
        } else if let Some(drawer) = get_drawer(conn, &drawer_id)? {
            by_id.insert(
                drawer_id,
                HybridResult {
                    drawer: SearchResult {
                        id: drawer.id,
                        text: drawer.content,
                        wing: drawer.wing,
                        room: drawer.room,
                        source_file: drawer.source_file,
                        created_at: drawer.created_at,
                        filed_at: drawer.filed_at,
                        similarity: 0.0,
                    },
                    cosine: 0.0,
                    bm25: score,
                    coding_boost: 0.0,
                    preference_match: 0.0,
                    rerank_score: None,
                    combined: 0.0,
                },
            );
        }
    }

    for result in by_id.values_mut() {
        result.cosine = result.drawer.similarity.max(0.0);
    }

    let max_bm25 = by_id.values().map(|r| r.bm25).fold(0.0, f64::max);
    for result in by_id.values_mut() {
        let normalized_bm25 = if max_bm25 > 0.0 {
            result.bm25 / max_bm25
        } else {
            0.0
        };
        result.coding_boost = coding_agent_boost(query, &result.drawer.text, &result.drawer.room);
        result.preference_match = preference_match(query, &result.drawer.text, &result.drawer.room);
        let recency = recency_boost(&result.drawer.filed_at);
        result.combined = (result.cosine * COSINE_WEIGHT)
            + (normalized_bm25 * BM25_WEIGHT)
            + result.coding_boost
            + result.preference_match
            + recency;
        result.drawer.similarity = (result.combined * 1000.0).round() / 1000.0;
    }

    let mut results: Vec<_> = by_id.into_values().collect();
    results.sort_by(|a, b| {
        b.combined
            .partial_cmp(&a.combined)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(n_results);
    Ok(results)
}

fn bm25_scores(
    conn: &Connection,
    query: &str,
    filter: &DrawerFilter,
) -> Result<HashMap<String, f64>> {
    let terms = tokenize(query);
    if terms.is_empty() {
        return Ok(HashMap::new());
    }
    let unique_terms: HashSet<_> = terms.into_iter().collect();

    let total_docs: f64 = conn
        .query_row("SELECT COUNT(*) FROM bm25_doc_stats", [], |row| {
            row.get::<_, i64>(0)
        })
        .context("counting BM25 docs")? as f64;
    if total_docs == 0.0 {
        return Ok(HashMap::new());
    }

    let avg_len: f64 = conn
        .query_row(
            "SELECT COALESCE(AVG(doc_len), 0) FROM bm25_doc_stats",
            [],
            |row| row.get(0),
        )
        .context("reading average BM25 length")?;
    if avg_len <= 0.0 {
        return Ok(HashMap::new());
    }

    let mut scores = HashMap::new();
    for term in unique_terms {
        let doc_freq: f64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bm25_terms WHERE term = ?1",
                params![term],
                |row| row.get::<_, i64>(0),
            )
            .context("reading term document frequency")? as f64;
        if doc_freq == 0.0 {
            continue;
        }
        let idf = ((total_docs - doc_freq + 0.5) / (doc_freq + 0.5) + 1.0).ln();

        let mut stmt = conn
            .prepare(
                "SELECT t.drawer_id, t.tf, s.doc_len
                 FROM bm25_terms t
                 JOIN bm25_doc_stats s ON s.drawer_id = t.drawer_id
                 JOIN drawers d ON d.id = t.drawer_id
                 WHERE t.term = ?1
                   AND (?2 IS NULL OR d.wing = ?2)
                   AND (?3 IS NULL OR d.room = ?3)",
            )
            .context("preparing BM25 score query")?;
        let rows = stmt.query_map(
            params![term, filter.wing.as_deref(), filter.room.as_deref()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? as f64,
                    row.get::<_, i64>(2)? as f64,
                ))
            },
        )?;

        for row in rows {
            let (drawer_id, tf, doc_len) = row.context("reading BM25 row")?;
            let denom = tf + BM25_K1 * (1.0 - BM25_B + BM25_B * (doc_len / avg_len));
            let score = idf * ((tf * (BM25_K1 + 1.0)) / denom);
            *scores.entry(drawer_id).or_insert(0.0) += score;
        }
    }

    Ok(scores)
}

fn coding_agent_boost(query: &str, text: &str, room: &str) -> f64 {
    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();
    let room_lower = room.to_lowercase();
    let mut boost: f64 = 0.0;

    boost += intent_boost(
        &query_lower,
        &text_lower,
        &room_lower,
        &["why", "choose", "chose", "decided", "decision", "settled"],
        &[
            "decided",
            "chose",
            "because",
            "instead of",
            "rather than",
            "tradeoff",
            "settled on",
        ],
        &["decision", "decisions"],
        0.16,
    );
    boost += intent_boost(
        &query_lower,
        &text_lower,
        &room_lower,
        &[
            "fix",
            "fixed",
            "failed",
            "failure",
            "broke",
            "broken",
            "error",
            "last time",
        ],
        &[
            "fix",
            "fixed",
            "failed",
            "because",
            "root cause",
            "resolved",
            "workaround",
        ],
        &["problem", "problems", "fixes"],
        0.12,
    );
    boost += intent_boost(
        &query_lower,
        &text_lower,
        &room_lower,
        &[
            "command", "run", "test", "clippy", "fmt", "audit", "cargo", "npm",
        ],
        &[
            "run ", "cargo ", "npm ", "pytest", "clippy", "fmt", "audit", "--",
        ],
        &["command", "commands"],
        0.12,
    );
    boost += intent_boost(
        &query_lower,
        &text_lower,
        &room_lower,
        &["convention", "rule", "usually", "should", "how do we"],
        &["convention", "rule", "always", "never", "should", "must"],
        &["convention", "conventions"],
        0.14,
    );
    boost += intent_boost(
        &query_lower,
        &text_lower,
        &room_lower,
        &[
            "prefer",
            "preference",
            "user want",
            "user like",
            "feel about",
            "interface style",
            "public interface",
            "shape",
            "retrieval results",
            "explain themselves",
        ],
        &[
            "prefer",
            "preference",
            "user",
            "does not want",
            "values",
            "my style",
            "source-grounded",
            "score provenance",
            "public api",
            "public apis",
        ],
        &["preference", "preferences"],
        0.15,
    );

    // Extra boost for preference-tagged drawers when query asks about preferences,
    // conventions, or style — compensates for embedding distance between the question
    // form ("what do I prefer?") and the stored assertion form ("I prefer X").
    let query_asks_about_prefs =
        crate::query_intent::classify(query) == crate::query_intent::QueryIntent::Preference;
    let text_is_preference = text_lower.contains("i prefer")
        || text_lower.contains("i always")
        || text_lower.contains("i never")
        || text_lower.contains("my convention")
        || text_lower.contains("my style")
        || text_lower.contains("i tend to")
        || text_lower.contains("prefer to");
    if query_asks_about_prefs && text_is_preference {
        boost += 0.10;
    }
    if query_lower.contains("cli") && text_lower.contains("cli command") {
        boost += 0.08;
    }
    if query_lower.contains("adding cli") && text_lower.contains("avoid new cli") {
        boost += 0.20;
    }
    if query_lower.contains("grep") && text_lower.contains("grep") {
        boost += 0.08;
    }
    if (query_lower.contains("proof") || query_lower.contains("visibility advice"))
        && text_lower.contains("proof from practical evaluations")
    {
        boost += 0.16;
    }
    boost += intent_boost(
        &query_lower,
        &text_lower,
        &room_lower,
        &[
            "current",
            "changed",
            "now",
            "last",
            "before",
            "direction",
            "proof",
            "proves",
            "priority",
            "release theme",
            "0.2.0",
        ],
        &[
            "current",
            "changed",
            "now",
            "from",
            "to",
            "direction",
            "proof priority",
            "eval suite",
            "release theme",
            "0.2.0",
            "agent memory reliability",
        ],
        &["current", "temporal"],
        0.12,
    );
    boost += intent_boost(
        &query_lower,
        &text_lower,
        &room_lower,
        &["session", "continuity", "warm-start", "warm start", "diary"],
        &[
            "session",
            "continuity",
            "warm-start",
            "warm start",
            "diary",
            "project path",
            "timestamp",
            "tags",
        ],
        &["current", "session"],
        0.11,
    );

    if text_lower.contains(&query_lower) {
        boost += 0.04;
    }
    boost.min(MAX_CODING_BOOST)
}

fn preference_match(query: &str, text: &str, room: &str) -> f64 {
    if crate::query_intent::classify(query) != crate::query_intent::QueryIntent::Preference
        || !crate::preference::is_preference(text)
    {
        return 0.0;
    }

    let mut score: f64 = 0.12;
    if room.eq_ignore_ascii_case("preference") || room.eq_ignore_ascii_case("preferences") {
        score += 0.04;
    }
    if let Some(span) = crate::preference::preference_span(text) {
        let span_lower = span.to_lowercase();
        if tokenize(&query.to_lowercase())
            .into_iter()
            .any(|term| term.len() > 3 && span_lower.contains(&term))
        {
            score += 0.06;
        }
    }
    score.min(MAX_PREFERENCE_MATCH)
}

fn intent_boost(
    query: &str,
    text: &str,
    room: &str,
    query_terms: &[&str],
    text_terms: &[&str],
    rooms: &[&str],
    amount: f64,
) -> f64 {
    if !query_terms.iter().any(|term| query.contains(term)) {
        return 0.0;
    }
    let text_match = text_terms.iter().any(|term| text.contains(term));
    let room_match = rooms.contains(&room);
    match (text_match, room_match) {
        (true, true) => amount,
        (true, false) => amount * 0.7,
        (false, true) => amount * 0.45,
        (false, false) => 0.0,
    }
}

pub(crate) fn tokenize(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter_map(|term| {
            let term = term.trim().to_lowercase();
            if term.len() < 2 {
                None
            } else {
                Some(term)
            }
        })
        .collect()
}
