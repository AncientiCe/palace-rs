//! Query cleanup for agent-originated memory searches.
//!
//! Coding agents sometimes pass an entire prompt/tool dump as the search query.
//! The retriever works better when it searches the actual memory question.

const MAX_QUERY_WORDS: usize = 80;
const QUERY_MARKERS: &[&str] = &[
    "search query:",
    "memory query:",
    "query:",
    "question:",
    "user question:",
    "ask:",
];

const INTENT_TERMS: &[&str] = &[
    "why",
    "choose",
    "chose",
    "decided",
    "decision",
    "fix",
    "fixed",
    "failed",
    "broke",
    "command",
    "run",
    "test",
    "prefer",
    "convention",
    "rule",
    "last time",
    "current",
    "changed",
];

/// Return the smallest useful natural-language query from an agent prompt.
pub fn sanitize_query(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }

    let mut marker_hits = Vec::new();
    for line in raw.lines() {
        let compact = compact_whitespace(line);
        let lower = compact.to_lowercase();
        for marker in QUERY_MARKERS {
            if let Some(index) = lower.find(marker) {
                let value = compact[index + marker.len()..].trim();
                if !value.is_empty() {
                    marker_hits.push(value.to_string());
                }
            }
        }
    }
    if let Some(hit) = marker_hits.last() {
        return cap_words(hit);
    }

    let mut in_fence = false;
    let mut useful_lines = Vec::new();
    let mut fallback_lines = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || should_drop_line(trimmed) {
            continue;
        }
        let compact = compact_whitespace(trimmed);
        if compact.is_empty() {
            continue;
        }
        let lower = compact.to_lowercase();
        if compact.contains('?') || INTENT_TERMS.iter().any(|term| lower.contains(term)) {
            useful_lines.push(compact);
        } else {
            fallback_lines.push(compact);
        }
    }

    let selected = if useful_lines.is_empty() {
        fallback_lines.join(" ")
    } else {
        useful_lines.join(" ")
    };
    cap_words(&selected)
}

fn should_drop_line(line: &str) -> bool {
    if line.is_empty() {
        return true;
    }
    let lower = line.to_lowercase();
    if lower.starts_with("you are ")
        || lower.starts_with("<")
        || lower.starts_with('{')
        || lower.starts_with('}')
        || lower.starts_with('[')
        || lower.starts_with(']')
        || lower.contains("\"tool\"")
        || lower.contains("\"output\"")
        || lower.contains("jsonrpc")
    {
        return true;
    }

    let punctuation = line
        .chars()
        .filter(|ch| !ch.is_alphanumeric() && !ch.is_whitespace())
        .count();
    line.len() > 240 && punctuation > line.len() / 8
}

fn compact_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cap_words(text: &str) -> String {
    text.split_whitespace()
        .take(MAX_QUERY_WORDS)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::sanitize_query;

    #[test]
    fn marker_wins_over_prompt_dump() {
        let query = sanitize_query(
            r#"You are an agent.
            ```json
            {"tool":"shell","output":"noise"}
            ```
            Search query: why did we choose sqlite?"#,
        );
        assert_eq!(query, "why did we choose sqlite?");
    }
}
