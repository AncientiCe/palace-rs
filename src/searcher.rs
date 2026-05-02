//! Semantic search over the palace.
//!
//! Embeds a query, does cosine similarity over all drawer embeddings,
//! returns verbatim text with similarity scores. Port of searcher.py.

use crate::embedder::embed_one;
use crate::ranker::hybrid_search;
use crate::store::DrawerFilter;
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
    let embedding = embed_one(query)?;
    let filter = DrawerFilter {
        wing: wing.map(String::from),
        room: room.map(String::from),
    };
    let results = hybrid_search(conn, query, Some(&embedding), &filter, n_results)?;

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
    let embedding = match embed_one(query) {
        Ok(e) => e,
        Err(e) => {
            return serde_json::json!({
                "error": format!("Embedding error: {e}"),
                "hint": "Run: mempalace mine <dir>"
            })
        }
    };

    let filter = DrawerFilter {
        wing: wing.map(String::from),
        room: room.map(String::from),
    };

    let results = match hybrid_search(conn, query, Some(&embedding), &filter, n_results) {
        Ok(r) => r,
        Err(e) => {
            return serde_json::json!({
                "error": format!("Search error: {e}"),
            })
        }
    };

    let hits: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "text": r.drawer.text,
                "wing": r.drawer.wing,
                "room": r.drawer.room,
                "source_file": r.drawer.source_file,
                "created_at": r.drawer.created_at,
                "similarity": r.combined,
                "cosine": r.cosine,
                "bm25": r.bm25,
            })
        })
        .collect();

    serde_json::json!({
        "query": query,
        "filters": {
            "wing": wing,
            "room": room,
        },
        "results": hits,
    })
}
