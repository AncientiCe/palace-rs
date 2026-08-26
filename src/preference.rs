//! Preference sentence detection for improved agent memory recall.
//!
//! Drawers containing user preferences are tagged at write time so they can be
//! surfaced by a dedicated search pass even when BM25 has no keyword overlap.

static PREFERENCE_PATTERNS: &[&str] = &[
    "i prefer ",
    "i like ",
    "i always ",
    "i never ",
    "i want ",
    "i dislike ",
    "i avoid ",
    "i don't ",
    "i do not ",
    "i hate ",
    "i love ",
    "i tend to ",
    "i usually ",
    "i typically ",
    "i find it ",
    "i'm comfortable",
    "i'm not comfortable",
    "i am comfortable",
    "i am not comfortable",
    "in my experience",
    "my convention",
    "my favorite",
    "my go-to",
    "my preference",
    "my preferred",
    "my style",
    "my approach is",
    "always use ",
    "never use ",
    "prefer to use",
    "prefer to write",
    "prefer to keep",
    "prefer to avoid",
    "don't like ",
    "do not like ",
    "would rather ",
    "feel strongly",
    "strongly prefer",
    "it's important to me",
    "it is important to me",
];

fn has_preference_pattern(lowercased_text: &str) -> bool {
    PREFERENCE_PATTERNS
        .iter()
        .any(|p| lowercased_text.contains(p))
}

/// Return true if `text` contains a preference or personal convention signal.
pub fn is_preference(text: &str) -> bool {
    has_preference_pattern(&text.to_lowercase())
}

/// Return the first sentence-like span that contains a preference signal.
///
/// The stored drawer can contain a long conversation or file chunk; indexing the
/// local preference sentence separately gives preference-shaped queries a
/// tighter embedding target while preserving verbatim storage.
///
/// Lowercases `text` once upfront rather than once per sentence: `.`, `!`,
/// `?`, and `\n` are all case-invariant, so splitting the lowercased text on
/// the same delimiters yields the same segments (by position) as splitting
/// the original — letting the two be walked in lockstep instead of
/// re-lowercasing every sentence just to check it against 44 patterns.
pub fn preference_span(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for (original, lowered) in text
        .split_inclusive(['.', '!', '?', '\n'])
        .zip(lower.split_inclusive(['.', '!', '?', '\n']))
    {
        let trimmed = original.trim();
        if !trimmed.is_empty() && has_preference_pattern(lowered.trim()) {
            return Some(trimmed.to_string());
        }
    }

    let trimmed = text.trim();
    if !trimmed.is_empty() && has_preference_pattern(lower.trim()) {
        Some(trimmed.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_basic_preferences() {
        assert!(is_preference("I prefer using snake_case for variables"));
        assert!(is_preference("I always add tests before shipping"));
        assert!(is_preference("I never commit directly to main"));
        assert!(is_preference(
            "My convention is to use Result for error handling"
        ));
    }

    #[test]
    fn extracts_preference_span_from_mixed_text() {
        let span = preference_span(
            "We discussed the CLI. I prefer small public APIs through Palace. Then we moved on.",
        );
        assert_eq!(
            span.as_deref(),
            Some("I prefer small public APIs through Palace.")
        );
    }

    #[test]
    fn extracts_negative_preference_span() {
        let span = preference_span("Normal note. I don't like adding new commands for tiny flows.");
        assert_eq!(
            span.as_deref(),
            Some("I don't like adding new commands for tiny flows.")
        );
    }

    #[test]
    fn ignores_neutral_text() {
        assert!(!is_preference("The function returns a vector of strings"));
        assert!(!is_preference("cargo fmt runs the formatter"));
        assert!(!is_preference("This was fixed in commit abc123"));
    }
}
