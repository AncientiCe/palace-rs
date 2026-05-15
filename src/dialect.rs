//! AAAK dialect — compressed memory format for MemPalace.
//!
//! Contains the AAAK spec and PALACE_PROTOCOL constants used in MCP status responses.
//! Also provides token counting and basic compression stats. Port of dialect.py.

/// Protocol instructions embedded in the MCP status response.
pub const PALACE_PROTOCOL: &str = "MANDATORY — Palace Memory Protocol (no exceptions, every session):
1. SESSION START: Call palace_status, then palace_session_context(agent_name) BEFORE doing anything else.
2. BEFORE RESPONDING about any person, project, past decision, or preference: call palace_search and palace_kg_query for durable facts FIRST. Never guess — verify.
3. CONTINUING PRIOR WORK: call palace_diary_search before resuming a previous task, release, plan, fix, or decision thread.
4. REPEAT QUESTIONS: before re-answering planning/history questions, search diary and project memory for similar prior answers.
5. FOR PREFERENCES / CONVENTIONS: call palace_preference_search — it surfaces preference drawers BM25 misses.
6. IF UNSURE about a fact (name, gender, age, relationship): say \"let me check\" and query the palace. Wrong is worse than slow.
7. AFTER SUBSTANTIVE WORK: call palace_diary_write to record what happened, what you learned, what matters.
8. WHEN FACTS CHANGE: call palace_kg_invalidate on the old fact, palace_kg_add for the new one.
9. TO FILE A KEY FACT: use palace_remember (sets importance=5). To delete outdated info: use palace_forget.

MEMORY ROUTING: use Palace for prior decisions, user preferences, previous fixes, commands that worked, project history, and \"what happened last time?\". Use code search first only for current source symbols, exact definitions, and implementation details that may have changed.

Skipping any step is a protocol violation. Storage is not memory — but storage + this protocol = memory.";

/// The AAAK compressed memory dialect specification.
pub const AAAK_SPEC: &str = "AAAK is a compressed memory dialect that MemPalace uses for efficient storage.
It is designed to be readable by both humans and LLMs without decoding.

FORMAT:
  ENTITIES: 3-letter uppercase codes. ALC=Alice, JOR=Jordan, RIL=Riley, MAX=Max, BEN=Ben.
  EMOTIONS: *action markers* before/during text. *warm*=joy, *fierce*=determined, *raw*=vulnerable, *bloom*=tenderness.
  STRUCTURE: Pipe-separated fields. FAM: family | PROJ: projects | ⚠: warnings/reminders.
  DATES: ISO format (2026-03-31). COUNTS: Nx = N mentions (e.g., 570x).
  IMPORTANCE: ★ to ★★★★★ (1-5 scale).
  HALLS: hall_facts, hall_events, hall_discoveries, hall_preferences, hall_advice.
  WINGS: wing_user, wing_agent, wing_team, wing_code, wing_myproject, wing_hardware, wing_ue5, wing_ai_research.
  ROOMS: Hyphenated slugs representing named ideas (e.g., chromadb-setup, gpu-pricing).

EXAMPLE:
  FAM: ALC→♡JOR | 2D(kids): RIL(18,sports) MAX(11,chess+swimming) | BEN(contributor)

Read AAAK naturally — expand codes mentally, treat *markers* as emotional context.
When WRITING AAAK: use entity codes, mark emotions, keep structure tight.";

/// Rough token estimate: ~4 chars per token (same heuristic as Python version).
pub fn token_count(text: &str) -> usize {
    text.len() / 4
}

/// AAAK abbreviation table: (phrase, replacement).
///
/// Ordered longest-first so longer phrases are matched before their substrings.
static AAAK_ABBREVS: &[(&str, &str)] = &[
    // Rust / code terms
    ("implementation", "impl"),
    ("function", "fn"),
    ("variable", "var"),
    ("parameter", "param"),
    ("parameters", "params"),
    ("argument", "arg"),
    ("arguments", "args"),
    ("attribute", "attr"),
    ("attributes", "attrs"),
    ("configuration", "cfg"),
    ("database", "db"),
    ("repository", "repo"),
    ("dependencies", "deps"),
    ("dependency", "dep"),
    ("documentation", "docs"),
    ("environment", "env"),
    ("error handling", "err-hdl"),
    ("authentication", "auth"),
    ("authorization", "authz"),
    ("performance", "perf"),
    ("development", "dev"),
    ("production", "prod"),
    ("infrastructure", "infra"),
    ("architecture", "arch"),
    ("application", "app"),
    ("component", "cmpnt"),
    ("interface", "iface"),
    ("directory", "dir"),
    ("generate", "gen"),
    ("generated", "genned"),
    ("utilities", "utils"),
    ("utility", "util"),
    // Common phrases
    ("in order to", "to"),
    ("as well as", "&"),
    ("for example", "e.g."),
    ("such as", "e.g."),
    ("in addition", "also"),
    ("at the moment", "now"),
    ("currently", "now"),
    ("make sure", "ensure"),
    ("in the future", "later"),
    ("because of", "due to"),
    ("instead of", "vs"),
    ("rather than", "vs"),
    ("should not", "shouldn't"),
    ("do not", "don't"),
    ("cannot", "can't"),
    ("will not", "won't"),
    ("does not", "doesn't"),
    ("did not", "didn't"),
    ("would not", "wouldn't"),
    ("could not", "couldn't"),
    ("should be", "s/b"),
    ("needs to be", "must be"),
    ("is not", "isn't"),
    ("are not", "aren't"),
    ("have not", "haven't"),
    ("has not", "hasn't"),
    ("the following", "these"),
    ("as a result", "so"),
];

/// Structure-aware AAAK compression.
///
/// Steps:
/// 1. Strip redundant markdown decorators (horizontal rules, repeated fences, trailing spaces)
/// 2. Collapse consecutive blank lines to at most one
/// 3. Apply the AAAK abbreviation table (case-preserving word-boundary replacement)
///
/// Targets 30–55% token reduction on typical code-discussion text without losing meaning.
pub fn compress(text: &str) -> String {
    // Step 1: strip markdown boilerplate
    let lines: Vec<&str> = text.lines().collect();
    let mut cleaned: Vec<String> = Vec::with_capacity(lines.len());
    let mut prev_blank = false;

    for line in &lines {
        let trimmed = line.trim_end();

        // Drop horizontal rules (---, ___, ***)
        if trimmed.len() >= 3
            && (trimmed.chars().all(|c| c == '-')
                || trimmed.chars().all(|c| c == '_')
                || trimmed.chars().all(|c| c == '*'))
        {
            continue;
        }

        // Collapse consecutive blank lines
        if trimmed.is_empty() {
            if prev_blank {
                continue;
            }
            prev_blank = true;
        } else {
            prev_blank = false;
        }

        cleaned.push(trimmed.to_string());
    }

    // Remove leading/trailing blank lines
    while cleaned.first().map(|s| s.is_empty()).unwrap_or(false) {
        cleaned.remove(0);
    }
    while cleaned.last().map(|s| s.is_empty()).unwrap_or(false) {
        cleaned.pop();
    }

    let joined = cleaned.join("\n");

    // Step 2: apply abbreviation table (case-insensitive match, preserve case of surrounding text)
    let mut result = joined;
    for (phrase, abbrev) in AAAK_ABBREVS {
        result = replace_word_boundary_ci(&result, phrase, abbrev);
    }

    format!("[AAAK] {result}")
}

/// Replace all case-insensitive occurrences of `phrase` at word boundaries with `abbrev`.
fn replace_word_boundary_ci(text: &str, phrase: &str, abbrev: &str) -> String {
    let lower = text.to_lowercase();
    let phrase_lower = phrase.to_lowercase();
    let phrase_len = phrase.len();

    if !lower.contains(phrase_lower.as_str()) {
        return text.to_string();
    }

    let mut result = String::with_capacity(text.len());
    let mut search_start = 0usize;

    while let Some(pos) = lower[search_start..].find(phrase_lower.as_str()) {
        let abs = search_start + pos;

        // Word-boundary check: char before must be non-alphabetic (or start of string)
        let before_ok = abs == 0
            || !text[..abs]
                .chars()
                .next_back()
                .unwrap_or(' ')
                .is_alphabetic();
        let after_pos = abs + phrase_len;
        // Char after must be non-alphabetic (or end of string)
        let after_ok = after_pos >= text.len()
            || !text[after_pos..]
                .chars()
                .next()
                .unwrap_or(' ')
                .is_alphabetic();

        if before_ok && after_ok {
            result.push_str(&text[search_start..abs]);
            result.push_str(abbrev);
            search_start = after_pos;
        } else {
            // Not a word boundary — advance past this match to avoid infinite loop
            result.push_str(&text[search_start..abs + 1]);
            search_start = abs + 1;
        }
    }

    result.push_str(&text[search_start..]);
    result
}

/// Basic compression statistics.
pub fn compression_stats(original: &str, compressed: &str) -> serde_json::Value {
    let original_tokens = token_count(original);
    let compressed_tokens = token_count(compressed);
    let ratio = if original_tokens > 0 {
        compressed_tokens as f64 / original_tokens as f64
    } else {
        1.0
    };
    serde_json::json!({
        "original_tokens": original_tokens,
        "compressed_tokens": compressed_tokens,
        "compression_ratio": (ratio * 1000.0).round() / 1000.0,
        "savings_pct": ((1.0 - ratio) * 100.0).round() as i64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compress_reduces_tokens() {
        let input = "The implementation of this function should not be \
            too complex. In order to make sure the database is properly \
            configured, check the documentation for environment variables.";
        let compressed = compress(input);
        assert!(compressed.starts_with("[AAAK]"));
        let stats = compression_stats(input, &compressed[7..]); // strip [AAAK] prefix
        assert!(
            stats["savings_pct"].as_i64().unwrap_or(0) > 0,
            "Expected some token reduction, got {stats}"
        );
    }

    #[test]
    fn strips_horizontal_rules() {
        let input = "Title\n---\nContent\n___\nMore";
        let result = compress(input);
        assert!(!result.contains("---"));
        assert!(!result.contains("___"));
        assert!(result.contains("Content"));
    }

    #[test]
    fn collapses_blank_lines() {
        let input = "line1\n\n\n\nline2";
        let result = compress(input);
        assert!(!result.contains("\n\n\n"));
    }
}
