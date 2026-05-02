//! Lightweight entity detection for drawer metadata.

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Person,
    Project,
    Topic,
    AgentPersona,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    pub name: String,
    pub kind: EntityKind,
}

static CAMEL_CASE: Lazy<Regex> =
    Lazy::new(|| compile_regex(r"\b[A-Z][A-Za-z0-9]+(?:[A-Z][A-Za-z0-9]+)+\b"));
static DIALOGUE_NAME: Lazy<Regex> =
    Lazy::new(|| compile_regex(r"(?m)^([A-Z][\p{L}][\p{L}0-9_-]{1,40}):\s"));
static TITLE_NAME: Lazy<Regex> =
    Lazy::new(|| compile_regex(r"\b[A-Z][\p{L}]{2,}(?:\s+[A-Z][\p{L}]{2,})?\b"));

const PROJECT_HINTS: &[&str] = &["DB", "API", "SDK", "CLI", "MCP", "SQL", "HTTP"];
const AGENT_NAMES: &[&str] = &["Claude", "Gemini", "Codex", "Assistant"];
const STOPWORDS: &[&str] = &[
    "The", "This", "That", "When", "Where", "Because", "Rust", "SQLite",
];

pub fn detect_entities(text: &str) -> Vec<Entity> {
    let mut entities: BTreeMap<String, EntityKind> = BTreeMap::new();

    for capture in DIALOGUE_NAME.captures_iter(text) {
        if let Some(name) = capture.get(1) {
            insert_entity(&mut entities, name.as_str(), EntityKind::Person);
        }
    }

    for capture in CAMEL_CASE.captures_iter(text) {
        if let Some(name) = capture.get(0) {
            insert_entity(&mut entities, name.as_str(), classify_name(name.as_str()));
        }
    }

    for capture in TITLE_NAME.captures_iter(text) {
        if let Some(name) = capture.get(0) {
            insert_entity(&mut entities, name.as_str(), classify_name(name.as_str()));
        }
    }

    entities
        .into_iter()
        .map(|(name, kind)| Entity { name, kind })
        .collect()
}

pub fn entity_metadata(text: &str) -> serde_json::Value {
    let entities = detect_entities(text);
    serde_json::json!({
        "entities": entities,
    })
}

fn insert_entity(entities: &mut BTreeMap<String, EntityKind>, name: &str, kind: EntityKind) {
    let name = name.trim();
    if name.len() < 2 || STOPWORDS.contains(&name) {
        return;
    }
    entities.entry(name.to_string()).or_insert(kind);
}

fn classify_name(name: &str) -> EntityKind {
    if AGENT_NAMES
        .iter()
        .any(|agent| agent.eq_ignore_ascii_case(name))
    {
        return EntityKind::AgentPersona;
    }
    if PROJECT_HINTS.iter().any(|hint| name.contains(hint)) || name.contains('-') {
        return EntityKind::Project;
    }
    if name.split_whitespace().count() > 1 {
        EntityKind::Person
    } else {
        EntityKind::Topic
    }
}

fn compile_regex(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(err) => panic!("invalid built-in entity regex {pattern}: {err}"),
    }
}
