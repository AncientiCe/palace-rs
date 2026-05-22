//! Basic Palace usage example.
//!
//! Run: cargo run --example basic_usage

use palace::db;
use palace::knowledge_graph;
use palace::store;

fn main() -> anyhow::Result<()> {
    // 1. Open an in-memory palace (use db::open(&config.palace_db_path()) for production)
    let conn = db::open_in_memory()?;

    println!("=== Palace Basic Usage ===\n");

    // 2. Add some drawers
    let (added, id) = store::add_drawer(
        &conn,
        "wing_code",
        "backend",
        "We use SQLite with WAL mode for the palace database because it supports concurrent reads.",
        None, // embedding (None = no vector, use embed_one() for real usage)
        "architecture.md",
        0,
        "example",
        4.0,
    )?;
    println!("Added drawer: {} (id: {})", added, &id[..20]);

    // 3. Add a knowledge graph fact
    let triple_id = knowledge_graph::add_triple(
        &conn,
        "Palace",
        "stores_data_in",
        "SQLite",
        Some("2026-04-01"),
        None,
        1.0,
        Some(&id),
        None,
    )?;
    println!("Added triple: {}", &triple_id[..20]);

    // 4. Query the KG
    let facts = knowledge_graph::query_entity(&conn, "Palace", None, "outgoing")?;
    println!("\nFacts about Palace:");
    for fact in &facts {
        println!("  {} → {} → {}", fact.subject, fact.predicate, fact.object);
    }

    // 5. Palace stats
    let total = store::count_drawers(&conn)?;
    let wings = store::wing_counts(&conn)?;
    println!("\nPalace stats:");
    println!("  Total drawers: {total}");
    println!("  Wings: {:?}", wings);

    let kg_stats = knowledge_graph::stats(&conn)?;
    println!("  KG entities: {}", kg_stats.entities);
    println!("  KG triples: {}", kg_stats.triples);

    println!("\nDone! For real usage, run: palace init <dir> && palace mine <dir>");
    Ok(())
}
