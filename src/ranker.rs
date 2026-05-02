//! Hybrid retrieval: BM25 keyword scoring plus optional vector similarity.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};

use crate::store::{get_drawer, vector_search, DrawerFilter, SearchResult};

const BM25_K1: f64 = 1.5;
const BM25_B: f64 = 0.75;
const COSINE_WEIGHT: f64 = 0.65;
const BM25_WEIGHT: f64 = 0.35;

#[derive(Debug, Clone)]
pub struct HybridResult {
    pub drawer: SearchResult,
    pub cosine: f64,
    pub bm25: f64,
    pub combined: f64,
}

pub fn hybrid_search(
    conn: &Connection,
    query: &str,
    query_vec: Option<&[f32]>,
    filter: &DrawerFilter,
    n_results: usize,
) -> Result<Vec<HybridResult>> {
    let mut by_id: HashMap<String, HybridResult> = HashMap::new();

    if let Some(vec) = query_vec {
        for result in vector_search(conn, vec, filter, n_results.saturating_mul(4).max(20))? {
            by_id.insert(
                result.id.clone(),
                HybridResult {
                    drawer: result,
                    cosine: 0.0,
                    bm25: 0.0,
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
                        source_file: std::path::Path::new(&drawer.source_file)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or(drawer.source_file),
                        created_at: drawer.created_at,
                        similarity: 0.0,
                    },
                    cosine: 0.0,
                    bm25: score,
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
        let preference_boost = preference_boost(query, &result.drawer.text);
        result.combined =
            (result.cosine * COSINE_WEIGHT) + (normalized_bm25 * BM25_WEIGHT) + preference_boost;
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

fn preference_boost(query: &str, text: &str) -> f64 {
    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();
    if query_lower.contains("prefer") && text_lower.contains("prefer") {
        0.05
    } else {
        0.0
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
