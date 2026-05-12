//! Semantic search over the palace.
//!
//! Embeds a query, does cosine similarity over all drawer embeddings,
//! returns verbatim text with similarity scores. Port of searcher.py.

use crate::embedder::embed_one;
use crate::ranker::{hybrid_search, HybridResult};
use crate::store::{preference_search_filtered, source_context, DrawerFilter, SearchResult};
use anyhow::Result;
use rusqlite::Connection;

/// Print search results to stdout in the same format as the Python version.
pub fn search_and_print(
    conn: &Connection,
    query: &str,
    wing: Option<&str>,
    room: Option<&str>,
    n_results: usize,
) -> Result<()> {
    let rewritten = crate::query_rewriter::rewrite(query);
    let sanitized_query = crate::query_sanitizer::sanitize_query(&rewritten);
    let effective_query = if sanitized_query.is_empty() {
        query
    } else {
        &sanitized_query
    };
    let embedding = embed_one(effective_query)?;
    let filter = DrawerFilter {
        wing: wing.map(String::from),
        room: room.map(String::from),
    };
    let results = hybrid_search(conn, effective_query, Some(&embedding), &filter, n_results)?;

    println!("\n{}", "=".repeat(60));
    println!("  Results for: \"{query}\"");
    if let Some(w) = wing {
        println!("  Wing: {w}");
    }
    if let Some(r) = room {
        println!("  Room: {r}");
    }
    println!("{}\n", "=".repeat(60));

    if results.is_empty() {
        println!("  No results found for: \"{query}\"");
        return Ok(());
    }

    for (i, result) in results.iter().enumerate() {
        println!(
            "  [{}] {} / {}",
            i + 1,
            result.drawer.wing,
            result.drawer.room
        );
        println!("      Source: {}", result.drawer.source_file);
        println!("      Match:  {:.3}", result.combined);
        println!("      cosine={:.3} bm25={:.3}", result.cosine, result.bm25);
        println!();
        for line in result.drawer.text.trim().lines() {
            println!("      {line}");
        }
        println!();
        println!("  {}", "-".repeat(56));
    }
    println!();
    Ok(())
}

/// Programmatic search returning structured results (used by MCP).
pub fn search_memories(
    conn: &Connection,
    query: &str,
    wing: Option<&str>,
    room: Option<&str>,
    n_results: usize,
) -> serde_json::Value {
    let rewritten = crate::query_rewriter::rewrite(query);
    let sanitized_query = crate::query_sanitizer::sanitize_query(&rewritten);
    let effective_query = if sanitized_query.is_empty() {
        query
    } else {
        &sanitized_query
    };
    let embedding = match embed_one(effective_query) {
        Ok(e) => e,
        Err(e) => {
            return serde_json::json!({
                "error": format!("Embedding error: {e}"),
                "hint": "Run: palace mine <dir>"
            })
        }
    };

    let filter = DrawerFilter {
        wing: wing.map(String::from),
        room: room.map(String::from),
    };

    let mut results =
        match hybrid_search(conn, effective_query, Some(&embedding), &filter, n_results) {
            Ok(r) => r,
            Err(e) => {
                return serde_json::json!({
                    "error": format!("Search error: {e}"),
                })
            }
        };

    // Merge dedicated preference recall pass — surfaces preference drawers even
    // when BM25 has no keyword overlap (the known R@1 weakness).
    merge_preference_results(conn, &embedding, &filter, &mut results, n_results);

    let hits: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            let source_context = crate::store::get_drawer(conn, &r.drawer.id)
                .ok()
                .flatten()
                .and_then(|drawer| {
                    source_context(conn, &drawer.source_file, drawer.chunk_index, 1).ok()
                })
                .unwrap_or_default()
                .into_iter()
                .map(|drawer| {
                    serde_json::json!({
                        "id": drawer.id,
                        "text": drawer.content,
                        "wing": drawer.wing,
                        "room": drawer.room,
                        "source_file": drawer.source_file,
                        "chunk_index": drawer.chunk_index,
                        "created_at": drawer.created_at,
                        "filed_at": drawer.filed_at,
                    })
                })
                .collect::<Vec<_>>();
            let filed_at = crate::store::get_drawer(conn, &r.drawer.id)
                .ok()
                .flatten()
                .map(|drawer| drawer.filed_at)
                .unwrap_or_default();
            serde_json::json!({
                "id": r.drawer.id,
                "text": r.drawer.text,
                "wing": r.drawer.wing,
                "room": r.drawer.room,
                "source_file": r.drawer.source_file,
                "created_at": r.drawer.created_at,
                "filed_at": filed_at,
                "similarity": r.combined,
                "combined": r.combined,
                "cosine": r.cosine,
                "bm25": r.bm25,
                "coding_boost": r.coding_boost,
                "source_context": source_context,
            })
        })
        .collect();

    serde_json::json!({
        "query": query,
        "sanitized_query": effective_query,
        "filters": {
            "wing": wing,
            "room": room,
        },
        "results": hits,
    })
}

/// Merge preference-tagged drawers into hybrid results, deduplicating by ID.
///
/// Preference drawers are weighted at 0.4 of their cosine similarity so they
/// supplement (rather than displace) strong non-preference matches. Only the top
/// half of preference candidates (cosine ≥ 0.25) are considered.
fn merge_preference_results(
    conn: &Connection,
    query_vec: &[f32],
    filter: &DrawerFilter,
    results: &mut Vec<HybridResult>,
    n_results: usize,
) {
    let pref_candidates = match preference_search_filtered(conn, query_vec, filter, 10) {
        Ok(v) => v,
        Err(_) => return,
    };

    let existing_ids: std::collections::HashSet<String> =
        results.iter().map(|r| r.drawer.id.clone()).collect();

    for pref in pref_candidates {
        if pref.similarity < 0.25 {
            break; // already sorted descending — skip low-relevance preferences
        }
        if existing_ids.contains(&pref.id) {
            continue;
        }
        let combined = pref.similarity * 0.4;
        results.push(HybridResult {
            cosine: pref.similarity,
            bm25: 0.0,
            coding_boost: 0.0,
            combined,
            drawer: SearchResult {
                similarity: (combined * 1000.0).round() / 1000.0,
                ..pref
            },
        });
    }

    results.sort_by(|a, b| {
        b.combined
            .partial_cmp(&a.combined)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(n_results);
}
