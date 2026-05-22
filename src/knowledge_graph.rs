//! Temporal Entity-Relationship Knowledge Graph.
//!
//! Direct port of knowledge_graph.py.
//! Uses the same SQLite tables (entities + triples) in palace.db.
//!
//! - Entity nodes: people, projects, tools, concepts
//! - Typed relationship edges with temporal validity (valid_from → valid_to)
//! - Closet references linking back to source drawers

use anyhow::{Context, Result};
use chrono::Local;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Triple {
    pub direction: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub confidence: f64,
    pub source_closet: Option<String>,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KgStats {
    pub entities: i64,
    pub triples: i64,
    pub current_facts: i64,
    pub expired_facts: i64,
    pub relationship_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, Eq, PartialEq)]
pub struct SeedReport {
    pub inserted: usize,
    pub unchanged: usize,
    pub invalidated: usize,
}

fn entity_id(name: &str) -> String {
    name.to_lowercase().replace(' ', "_").replace('\'', "")
}

fn normalize_predicate(pred: &str) -> String {
    pred.to_lowercase().replace(' ', "_")
}

fn triple_id(sub_id: &str, pred: &str, obj_id: &str) -> String {
    let hash =
        blake3::hash(format!("{sub_id}/{pred}/{obj_id}/{}", Local::now().to_rfc3339()).as_bytes());
    format!("t_{sub_id}_{pred}_{obj_id}_{}", &hash.to_hex()[..8])
}

/// Add or update an entity node. Returns the entity ID.
pub fn add_entity(
    conn: &Connection,
    name: &str,
    entity_type: &str,
    properties: Option<&HashMap<String, serde_json::Value>>,
) -> Result<String> {
    let eid = entity_id(name);
    let props = serde_json::to_string(&properties.unwrap_or(&HashMap::new()))?;
    conn.execute(
        "INSERT OR REPLACE INTO entities (id, name, type, properties) VALUES (?1, ?2, ?3, ?4)",
        params![eid, name, entity_type, props],
    )
    .context("inserting entity")?;
    Ok(eid)
}

/// Ensure entity exists without overwriting existing data.
fn ensure_entity(conn: &Connection, name: &str) -> Result<String> {
    let eid = entity_id(name);
    conn.execute(
        "INSERT OR IGNORE INTO entities (id, name) VALUES (?1, ?2)",
        params![eid, name],
    )
    .context("ensuring entity")?;
    Ok(eid)
}

/// Add a relationship triple. Auto-creates entities if they don't exist.
/// Returns the triple ID.
#[allow(clippy::too_many_arguments)]
pub fn add_triple(
    conn: &Connection,
    subject: &str,
    predicate: &str,
    obj: &str,
    valid_from: Option<&str>,
    valid_to: Option<&str>,
    confidence: f64,
    source_closet: Option<&str>,
    source_file: Option<&str>,
) -> Result<String> {
    let sub_id = ensure_entity(conn, subject)?;
    let obj_id = ensure_entity(conn, obj)?;
    let pred = normalize_predicate(predicate);

    // Check for existing identical active triple
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM triples WHERE subject=?1 AND predicate=?2 AND object=?3 AND valid_to IS NULL",
            params![sub_id, pred, obj_id],
            |r| r.get(0),
        )
        .ok();

    if let Some(id) = existing {
        return Ok(id);
    }

    let tid = triple_id(&sub_id, &pred, &obj_id);
    conn.execute(
        "INSERT INTO triples (id, subject, predicate, object, valid_from, valid_to, confidence, source_closet, source_file)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![tid, sub_id, pred, obj_id, valid_from, valid_to, confidence, source_closet, source_file],
    )
    .context("inserting triple")?;
    Ok(tid)
}

pub fn seed_agent_adoption_facts(conn: &Connection, project: &str) -> Result<SeedReport> {
    let mut report = SeedReport::default();
    let project = project.trim();
    let project = if project.is_empty() {
        "current project"
    } else {
        project
    };

    let facts = [
        ("Palace", "supports_client", "Cursor"),
        ("Palace", "supports_client", "Codex"),
        ("Palace", "supports_client", "Claude Code"),
        ("Palace", "supports_client", "Claude Desktop"),
        ("Palace", "requires_protocol_step", "session status"),
        ("Palace", "requires_protocol_step", "diary warm-start"),
        (
            "Palace",
            "requires_protocol_step",
            "memory search before project answers",
        ),
        (
            "Palace",
            "requires_protocol_step",
            "KG query before durable facts",
        ),
        (
            "Palace",
            "requires_protocol_step",
            "diary write after substantive work",
        ),
        (
            "Palace",
            "requires_protocol_step",
            "invalidate old fact before replacement",
        ),
        (
            "Palace",
            "uses_memory_routing",
            "Palace for prior decisions and preferences",
        ),
        (
            "Palace",
            "uses_memory_routing",
            "code search for current symbols",
        ),
        (
            "User",
            "prefers_agent_behavior",
            "memory-first Palace usage",
        ),
        (project, "has_role", "Rust memory engine for coding agents"),
        (project, "requires_quality_gate", "cargo fmt --all --check"),
        (
            project,
            "requires_quality_gate",
            "cargo clippy --all-targets --all-features -- -D warnings",
        ),
        (project, "requires_quality_gate", "cargo audit"),
        (
            project,
            "requires_quality_gate",
            "cargo test --all-features",
        ),
    ];

    for (subject, predicate, object) in facts {
        if active_fact_exists(conn, subject, predicate, object)? {
            report.unchanged += 1;
        } else {
            add_triple(
                conn,
                subject,
                predicate,
                object,
                Some(&Local::now().format("%Y-%m-%d").to_string()),
                None,
                1.0,
                None,
                Some("palace adoption seed"),
            )?;
            report.inserted += 1;
        }
    }

    Ok(report)
}

pub fn seed_or_update_fact(
    conn: &Connection,
    subject: &str,
    predicate: &str,
    old_object: &str,
    new_object: &str,
) -> Result<SeedReport> {
    let mut report = SeedReport::default();
    if old_object == new_object {
        if active_fact_exists(conn, subject, predicate, new_object)? {
            report.unchanged = 1;
            return Ok(report);
        }
    } else if active_fact_exists(conn, subject, predicate, old_object)? {
        invalidate(conn, subject, predicate, old_object, None)?;
        report.invalidated = 1;
    }

    if active_fact_exists(conn, subject, predicate, new_object)? {
        report.unchanged += 1;
    } else {
        add_triple(
            conn,
            subject,
            predicate,
            new_object,
            Some(&Local::now().format("%Y-%m-%d").to_string()),
            None,
            1.0,
            None,
            Some("palace adoption seed"),
        )?;
        report.inserted += 1;
    }

    Ok(report)
}

fn active_fact_exists(
    conn: &Connection,
    subject: &str,
    predicate: &str,
    object: &str,
) -> Result<bool> {
    let sub_id = entity_id(subject);
    let obj_id = entity_id(object);
    let pred = normalize_predicate(predicate);
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM triples WHERE subject=?1 AND predicate=?2 AND object=?3 AND valid_to IS NULL",
            params![sub_id, pred, obj_id],
            |row| row.get(0),
        )
        .optional()
        .context("checking active fact")?;
    Ok(existing.is_some())
}

/// Mark an active relationship as no longer valid by setting valid_to.
pub fn invalidate(
    conn: &Connection,
    subject: &str,
    predicate: &str,
    obj: &str,
    ended: Option<&str>,
) -> Result<()> {
    let sub_id = entity_id(subject);
    let obj_id = entity_id(obj);
    let pred = normalize_predicate(predicate);
    let end_date = ended
        .map(String::from)
        .unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string());

    conn.execute(
        "UPDATE triples SET valid_to=?1 WHERE subject=?2 AND predicate=?3 AND object=?4 AND valid_to IS NULL",
        params![end_date, sub_id, pred, obj_id],
    )
    .context("invalidating triple")?;
    Ok(())
}

/// Query all relationships for an entity.
///
/// direction: "outgoing" (entity→?), "incoming" (?→entity), "both"
/// as_of: only return facts valid at this date (ISO format)
pub fn query_entity(
    conn: &Connection,
    name: &str,
    as_of: Option<&str>,
    direction: &str,
) -> Result<Vec<Triple>> {
    let eid = entity_id(name);
    let mut results = Vec::new();

    if direction == "outgoing" || direction == "both" {
        let mut q = String::from(
            "SELECT t.predicate, t.valid_from, t.valid_to, t.confidence, t.source_closet, e.name
             FROM triples t JOIN entities e ON t.object = e.id WHERE t.subject = ?1",
        );
        let mut p: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(eid.clone())];
        if let Some(d) = as_of {
            q.push_str(" AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3)");
            p.push(Box::new(d.to_string()));
            p.push(Box::new(d.to_string()));
        }
        let mut stmt = conn.prepare(&q)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(p.iter().map(|x| x.as_ref())),
            |r| {
                Ok(Triple {
                    direction: "outgoing".into(),
                    subject: name.to_string(),
                    predicate: r.get(0)?,
                    object: r.get(5)?,
                    valid_from: r.get(1)?,
                    valid_to: r.get(2)?,
                    confidence: r.get(3)?,
                    source_closet: r.get(4)?,
                    current: r.get::<_, Option<String>>(2)?.is_none(),
                })
            },
        )?;
        for row in rows {
            results.push(row.context("outgoing triple row")?);
        }
    }

    if direction == "incoming" || direction == "both" {
        let mut q = String::from(
            "SELECT t.predicate, t.valid_from, t.valid_to, t.confidence, t.source_closet, e.name
             FROM triples t JOIN entities e ON t.subject = e.id WHERE t.object = ?1",
        );
        let mut p: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(eid.clone())];
        if let Some(d) = as_of {
            q.push_str(" AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3)");
            p.push(Box::new(d.to_string()));
            p.push(Box::new(d.to_string()));
        }
        let mut stmt = conn.prepare(&q)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(p.iter().map(|x| x.as_ref())),
            |r| {
                Ok(Triple {
                    direction: "incoming".into(),
                    subject: r.get(5)?,
                    predicate: r.get(0)?,
                    object: name.to_string(),
                    valid_from: r.get(1)?,
                    valid_to: r.get(2)?,
                    confidence: r.get(3)?,
                    source_closet: r.get(4)?,
                    current: r.get::<_, Option<String>>(2)?.is_none(),
                })
            },
        )?;
        for row in rows {
            results.push(row.context("incoming triple row")?);
        }
    }

    Ok(results)
}

/// Get all triples with a given relationship type.
pub fn query_relationship(
    conn: &Connection,
    predicate: &str,
    as_of: Option<&str>,
) -> Result<Vec<Triple>> {
    let pred = normalize_predicate(predicate);
    let mut q = String::from(
        "SELECT t.predicate, t.valid_from, t.valid_to, t.confidence, s.name, o.name
         FROM triples t
         JOIN entities s ON t.subject = s.id
         JOIN entities o ON t.object = o.id
         WHERE t.predicate = ?1",
    );
    let mut p: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(pred.clone())];
    if let Some(d) = as_of {
        q.push_str(" AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3)");
        p.push(Box::new(d.to_string()));
        p.push(Box::new(d.to_string()));
    }

    let mut stmt = conn.prepare(&q)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(p.iter().map(|x| x.as_ref())),
        |r| {
            Ok(Triple {
                direction: "outgoing".into(),
                subject: r.get(4)?,
                predicate: r.get(0)?,
                object: r.get(5)?,
                valid_from: r.get(1)?,
                valid_to: r.get(2)?,
                confidence: r.get(3)?,
                source_closet: None,
                current: r.get::<_, Option<String>>(2)?.is_none(),
            })
        },
    )?;
    rows.map(|r| r.context("relationship row")).collect()
}

/// Get all facts in chronological order, optionally filtered by entity.
pub fn timeline(conn: &Connection, entity_name: Option<&str>) -> Result<Vec<Triple>> {
    let (q, p): (String, Vec<Box<dyn rusqlite::ToSql>>) = if let Some(name) = entity_name {
        let eid = entity_id(name);
        (
            "SELECT t.predicate, t.valid_from, t.valid_to, t.confidence, s.name, o.name
             FROM triples t
             JOIN entities s ON t.subject = s.id
             JOIN entities o ON t.object = o.id
             WHERE (t.subject = ?1 OR t.object = ?2)
             ORDER BY t.valid_from ASC NULLS LAST
             LIMIT 100"
                .into(),
            vec![Box::new(eid.clone()), Box::new(eid)],
        )
    } else {
        (
            "SELECT t.predicate, t.valid_from, t.valid_to, t.confidence, s.name, o.name
             FROM triples t
             JOIN entities s ON t.subject = s.id
             JOIN entities o ON t.object = o.id
             ORDER BY t.valid_from ASC NULLS LAST
             LIMIT 100"
                .into(),
            vec![],
        )
    };

    let mut stmt = conn.prepare(&q)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(p.iter().map(|x| x.as_ref())),
        |r| {
            Ok(Triple {
                direction: "outgoing".into(),
                subject: r.get(4)?,
                predicate: r.get(0)?,
                object: r.get(5)?,
                valid_from: r.get(1)?,
                valid_to: r.get(2)?,
                confidence: r.get(3)?,
                source_closet: None,
                current: r.get::<_, Option<String>>(2)?.is_none(),
            })
        },
    )?;
    rows.map(|r| r.context("timeline row")).collect()
}

/// Summary statistics about the knowledge graph.
pub fn stats(conn: &Connection) -> Result<KgStats> {
    let entities: i64 = conn.query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0))?;
    let triples: i64 = conn.query_row("SELECT COUNT(*) FROM triples", [], |r| r.get(0))?;
    let current: i64 = conn.query_row(
        "SELECT COUNT(*) FROM triples WHERE valid_to IS NULL",
        [],
        |r| r.get(0),
    )?;
    let mut stmt = conn.prepare("SELECT DISTINCT predicate FROM triples ORDER BY predicate")?;
    let predicates: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(KgStats {
        entities,
        triples,
        current_facts: current,
        expired_facts: triples - current,
        relationship_types: predicates,
    })
}

/// Seed the graph from a map of entity facts (port of seed_from_entity_facts).
pub fn seed_from_entity_facts(conn: &Connection, entity_facts: &serde_json::Value) -> Result<()> {
    let obj = match entity_facts.as_object() {
        Some(o) => o,
        None => return Ok(()),
    };

    for (key, facts) in obj {
        let name = facts
            .get("full_name")
            .and_then(|v| v.as_str())
            .unwrap_or(key)
            .to_string();
        let etype = facts
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("person");
        let mut props = HashMap::new();
        if let Some(g) = facts.get("gender").and_then(|v| v.as_str()) {
            props.insert(
                "gender".to_string(),
                serde_json::Value::String(g.to_string()),
            );
        }
        if let Some(b) = facts.get("birthday").and_then(|v| v.as_str()) {
            props.insert(
                "birthday".to_string(),
                serde_json::Value::String(b.to_string()),
            );
        }
        add_entity(conn, &name, etype, Some(&props))?;

        let birthday = facts.get("birthday").and_then(|v| v.as_str()).unwrap_or("");

        if let Some(parent) = facts.get("parent").and_then(|v| v.as_str()) {
            let parent_cap = capitalize(parent);
            add_triple(
                conn,
                &name,
                "child_of",
                &parent_cap,
                Some(birthday).filter(|s| !s.is_empty()),
                None,
                1.0,
                None,
                None,
            )?;
        }

        if let Some(partner) = facts.get("partner").and_then(|v| v.as_str()) {
            let partner_cap = capitalize(partner);
            add_triple(
                conn,
                &name,
                "married_to",
                &partner_cap,
                None,
                None,
                1.0,
                None,
                None,
            )?;
        }

        if let Some(interests) = facts.get("interests").and_then(|v| v.as_array()) {
            for interest in interests {
                if let Some(i) = interest.as_str() {
                    add_triple(
                        conn,
                        &name,
                        "loves",
                        &capitalize(i),
                        Some("2025-01-01"),
                        None,
                        1.0,
                        None,
                        None,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}
