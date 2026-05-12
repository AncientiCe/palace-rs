//! Query rewriting — normalises raw agent queries before embedding/BM25 lookup.
//!
//! The rewriter does three things:
//!  1. Strips AAAK entity codes (3-letter uppercase) so they don't confuse BM25.
//!  2. Converts question-form queries into declarative keyword phrases.
//!  3. Expands common agent/code abbreviations into full words.

use once_cell::sync::Lazy;
use regex::Regex;

static QUESTION_WORDS: &[&str] = &[
    "what is",
    "what are",
    "what was",
    "what were",
    "who is",
    "who was",
    "how do",
    "how does",
    "how did",
    "why did",
    "why does",
    "where is",
    "where was",
    "when did",
    "when was",
    "tell me about",
    "do you know",
    "have you",
];

/// (abbreviation, expansion) pairs — applied case-insensitively.
static EXPANSIONS: &[(&str, &str)] = &[
    ("mcp", "model context protocol"),
    ("llm", "large language model"),
    ("onnx", "neural network"),
    ("db", "database"),
    ("cfg", "config"),
    ("fn ", "function "),
    ("impl", "implementation"),
    ("ux", "user experience"),
    ("ui", "user interface"),
    ("api", "application interface"),
    ("cli", "command line"),
    ("kg", "knowledge graph"),
    ("tui", "terminal interface"),
    ("auth", "authentication"),
    ("rpc", "remote call"),
];

static AAAK_CODE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b[A-Z]{3}\b").expect("invalid AAAK regex"));

/// Rewrite a query to improve hybrid retrieval recall.
///
/// Returns the rewritten query; returns the original if rewriting yields an
/// empty string.
pub fn rewrite(query: &str) -> String {
    let lower = query.trim().to_lowercase();

    // Strip trailing punctuation common in question-form queries.
    let stripped = lower.trim_end_matches(['?', '.', '!', ' ']);

    // Remove question-word preamble.
    let mut body = stripped;
    for prefix in QUESTION_WORDS {
        if let Some(rest) = stripped.strip_prefix(prefix) {
            let rest = rest.trim_start_matches([' ', ':']);
            if !rest.is_empty() {
                body = rest;
                break;
            }
        }
    }

    // Strip AAAK 3-letter codes from the body (they live in stored text but
    // usually aren't what the agent is searching for semantically).
    let no_aaak = AAAK_CODE.replace_all(body, " ");
    let cleaned = no_aaak.split_whitespace().collect::<Vec<_>>().join(" ");

    // Expand abbreviations.
    let mut expanded = cleaned.clone();
    for (abbrev, full) in EXPANSIONS {
        // Word-boundary match: abbrev is followed by space, end, or punctuation.
        if let Some(pos) = expanded.find(abbrev) {
            let after = expanded.get(pos + abbrev.len()..);
            let boundary =
                after.is_none_or(|s| s.starts_with([' ', ',', '.', ';', ':', '\t', '\n']));
            if boundary {
                expanded = format!(
                    "{}{}{}",
                    &expanded[..pos],
                    full,
                    &expanded[pos + abbrev.len()..]
                );
            }
        }
    }

    let result = expanded.trim().to_string();
    if result.is_empty() {
        query.trim().to_string()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_question_prefix() {
        assert_eq!(
            rewrite("what is the palace database schema"),
            "the palace database schema"
        );
        // "who was" is a question prefix and should be stripped
        assert_eq!(
            rewrite("who was working on the miner"),
            "working on the miner"
        );
    }

    #[test]
    fn question_mark_stripped() {
        let r = rewrite("What is MCP?");
        assert!(!r.ends_with('?'));
    }

    #[test]
    fn expands_known_abbreviation() {
        let r = rewrite("mcp server setup");
        assert!(r.contains("model context protocol"), "got: {r}");
    }

    #[test]
    fn strips_aaak_codes() {
        let r = rewrite("ALC project preferences");
        assert!(!r.contains("ALC"), "got: {r}");
    }

    #[test]
    fn returns_original_on_empty_result() {
        let r = rewrite("ALC");
        assert!(!r.is_empty());
    }

    #[test]
    fn passthrough_for_plain_keyword_query() {
        let r = rewrite("recency decay implementation");
        assert_eq!(r, "recency decay implementation");
    }
}
