//! Lightweight entity detection for drawer metadata.

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

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

const CAMEL_CASE_PATTERN: &str = r"\b[A-Z][A-Za-z0-9]+(?:[A-Z][A-Za-z0-9]+)+\b";
const DIALOGUE_NAME_PATTERN: &str = r"(?m)^([A-Z][\p{L}][\p{L}0-9_-]{1,40}):\s";
const TITLE_NAME_PATTERN: &str = r"\b[A-Z][\p{L}]{2,}(?:\s+[A-Z][\p{L}]{2,})?\b";
const NEVER_MATCHING_PATTERN: &str = r"[^\s\S]";

static CAMEL_CASE: Lazy<Regex> = Lazy::new(|| compile_regex(CAMEL_CASE_PATTERN));
static DIALOGUE_NAME: Lazy<Regex> = Lazy::new(|| compile_regex(DIALOGUE_NAME_PATTERN));
static TITLE_NAME: Lazy<Regex> = Lazy::new(|| compile_regex(TITLE_NAME_PATTERN));

const PROJECT_HINTS: &[&str] = &["DB", "API", "SDK", "CLI", "MCP", "SQL", "HTTP"];
const AGENT_NAMES: &[&str] = &["Claude", "Gemini", "Codex", "Assistant"];
const STOPWORDS: &[&str] = &["Rust", "SQLite"];
/// Words that are capitalized because they start a sentence, label a line
/// ("Note:", "Decision:") or shout ("DONE", "NOT"), not because they name
/// anything. Matched in any case.
const COMMON_WORDS: &[&str] = &[
    "the",
    "this",
    "that",
    "these",
    "those",
    "there",
    "here",
    "when",
    "where",
    "while",
    "because",
    "since",
    "then",
    "else",
    "but",
    "and",
    "not",
    "yes",
    "also",
    "still",
    "only",
    "just",
    "now",
    "never",
    "always",
    "must",
    "should",
    "could",
    "would",
    "can",
    "may",
    "might",
    "will",
    "does",
    "did",
    "done",
    "are",
    "was",
    "were",
    "been",
    "has",
    "have",
    "had",
    "its",
    "you",
    "they",
    "she",
    "our",
    "your",
    "their",
    "for",
    "from",
    "with",
    "without",
    "into",
    "onto",
    "over",
    "under",
    "after",
    "before",
    "until",
    "once",
    "both",
    "each",
    "every",
    "all",
    "any",
    "some",
    "most",
    "more",
    "many",
    "few",
    "new",
    "old",
    "how",
    "why",
    "what",
    "who",
    "which",
    "first",
    "second",
    "third",
    "next",
    "last",
    "however",
    "instead",
    "currently",
    "possible",
    "related",
    "note",
    "notes",
    "todo",
    "fixme",
    "warning",
    "important",
    "example",
    "examples",
    "summary",
    "result",
    "results",
    "status",
    "update",
    "updated",
    "decision",
    "design",
    "plan",
    "goal",
    "problem",
    "solution",
    "reason",
    "context",
    "question",
    "answer",
    "test",
    "tests",
    "fix",
    "fixed",
    "added",
    "changed",
    "removed",
    "deprecated",
    "keep",
    "follow",
    "use",
    "using",
    "see",
    "check",
    "run",
    "add",
    "make",
    "let",
    "user",
    "users",
    "consumer",
    "document",
];

pub fn detect_entities(text: &str) -> Vec<Entity> {
    let mut entities: BTreeMap<String, EntityKind> = BTreeMap::new();

    for capture in DIALOGUE_NAME.captures_iter(text) {
        if let Some(name) = capture.get(1) {
            if !is_common_word(name.as_str()) {
                insert_entity(&mut entities, name.as_str(), EntityKind::Person);
            }
        }
    }

    for capture in CAMEL_CASE.captures_iter(text) {
        if let Some(name) = capture.get(0) {
            // The pattern also matches words in capitals ("DONE").
            if !is_common_word(name.as_str()) {
                insert_entity(&mut entities, name.as_str(), classify_name(name.as_str()));
            }
        }
    }

    // Words the text also writes in lower case are ordinary words.
    let lowercase_words: HashSet<&str> = text
        .split(|ch: char| !ch.is_alphabetic())
        .filter(|word| word.chars().next().is_some_and(char::is_lowercase))
        .collect();
    for capture in TITLE_NAME.captures_iter(text) {
        if let Some(found) = capture.get(0) {
            let name = proper_name(text, found.start(), found.as_str(), &lowercase_words);
            if !name.is_empty() {
                insert_entity(&mut entities, &name, classify_name(&name));
            }
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

fn is_common_word(word: &str) -> bool {
    COMMON_WORDS.contains(&word.to_lowercase().as_str())
}

/// The part of a capitalized match that names something. Common words are
/// dropped from both ends. A lone word that starts a sentence is dropped when
/// the text also writes it in lower case ("Pushed to main ... pushed again"):
/// it is capitalized for its position, not because it is a name.
fn proper_name(text: &str, start: usize, found: &str, lowercase_words: &HashSet<&str>) -> String {
    let mut words: Vec<&str> = found.split_whitespace().collect();
    let mut leads = true;
    while words.first().is_some_and(|word| is_common_word(word)) {
        words.remove(0);
        leads = false;
    }
    while words.last().is_some_and(|word| is_common_word(word)) {
        words.pop();
    }
    if let [word] = words[..] {
        if leads
            && starts_sentence(text, start)
            && lowercase_words.contains(word.to_lowercase().as_str())
        {
            return String::new();
        }
    }
    words.join(" ")
}

/// True when the word at byte `start` opens the text, a line, a sentence or
/// a list item.
fn starts_sentence(text: &str, start: usize) -> bool {
    let before = text[..start].trim_end_matches([' ', '\t', '-', '*', '•', '#', '>', '(']);
    before
        .chars()
        .next_back()
        .is_none_or(|ch| matches!(ch, '\n' | '\r' | '.' | '!' | '?' | ':' | ';'))
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
        Err(err) => {
            // Built-in patterns are static and verified by tests below; if one
            // is ever invalid, degrade to a never-matching regex instead of
            // crashing the whole CLI/MCP process.
            eprintln!("warning: invalid built-in entity regex {pattern}: {err}");
            #[allow(clippy::unwrap_used)]
            Regex::new(NEVER_MATCHING_PATTERN).unwrap()
        }
    }
}

#[cfg(test)]
mod regex_tests {
    use super::*;

    #[test]
    fn built_in_entity_patterns_compile() {
        for pattern in [
            CAMEL_CASE_PATTERN,
            DIALOGUE_NAME_PATTERN,
            TITLE_NAME_PATTERN,
            NEVER_MATCHING_PATTERN,
        ] {
            assert!(
                Regex::new(pattern).is_ok(),
                "built-in regex must compile: {pattern}"
            );
        }
    }

    #[test]
    fn compile_regex_falls_back_to_never_matching() {
        let regex = compile_regex(r"([unclosed");
        assert!(!regex.is_match("anything at all"));
    }
}
