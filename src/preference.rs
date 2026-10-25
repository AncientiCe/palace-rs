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

/// Third-person and imperative phrasings agents use when filing the user's
/// rules ("Standing instruction from ...: do NOT use ..."). Only applied to
/// drawers an agent filed, never to mined files, where they mostly match code.
static DIRECTIVE_PATTERNS: &[&str] = &[
    "standing instruction",
    "standing rule",
    "preference from",
    "the user prefers",
    "user prefers",
    "the user wants",
    "user wants",
    "does not want",
    "doesn't want",
    "do not use",
    "don't use",
    "do not add",
    "don't add",
    "must not",
    "must always",
    "must never",
    "should always",
    "should never",
];

/// Conversation transcript extensions (mirrors the conversation miner).
const CONVERSATION_EXTENSIONS: &[&str] = &["txt", "md", "json", "jsonl"];

/// Where a drawer's text came from, which decides how it may be tagged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferenceSource {
    /// A mined project file. Never tagged: phrase matches there are code.
    Code,
    /// A mined conversation transcript. First-person phrasing only.
    Conversation,
    /// Filed by an agent or a person (remember, add_drawer, diary, import).
    Filed,
}

impl PreferenceSource {
    /// Infer the source from a drawer's `source_file`: mined drawers carry an
    /// absolute filesystem path, filed ones an empty or synthetic one.
    pub fn infer(source_file: &str) -> Self {
        let path = std::path::Path::new(source_file);
        let absolute = path.is_absolute() || source_file.starts_with('/');
        if !absolute {
            return Self::Filed;
        }
        let conversation = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                CONVERSATION_EXTENSIONS
                    .iter()
                    .any(|c| ext.eq_ignore_ascii_case(c))
            });
        if conversation {
            Self::Conversation
        } else {
            Self::Code
        }
    }
}

fn is_preference_room(room: &str) -> bool {
    room.eq_ignore_ascii_case("preferences") || room.eq_ignore_ascii_case("preference")
}

fn has_preference_pattern(lowercased_text: &str) -> bool {
    PREFERENCE_PATTERNS
        .iter()
        .any(|p| lowercased_text.contains(p))
}

fn has_directive_pattern(lowercased_text: &str) -> bool {
    has_preference_pattern(lowercased_text)
        || DIRECTIVE_PATTERNS
            .iter()
            .any(|p| lowercased_text.contains(p))
}

/// The preference span to index for a drawer being written, or `None` when it
/// should not be tagged. A drawer filed into a `preferences` room is always a
/// preference, whatever its wording, and its whole text is the span.
pub fn drawer_preference_span(text: &str, room: &str, source: PreferenceSource) -> Option<String> {
    match source {
        PreferenceSource::Code => None,
        PreferenceSource::Conversation => preference_span(text),
        PreferenceSource::Filed => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else if is_preference_room(room) {
                Some(trimmed.to_string())
            } else {
                span_matching(text, has_directive_pattern)
            }
        }
    }
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
    span_matching(text, has_preference_pattern)
}

fn span_matching(text: &str, matches: fn(&str) -> bool) -> Option<String> {
    let lower = text.to_lowercase();
    for (original, lowered) in text
        .split_inclusive(['.', '!', '?', '\n'])
        .zip(lower.split_inclusive(['.', '!', '?', '\n']))
    {
        let trimmed = original.trim();
        if !trimmed.is_empty() && matches(lowered.trim()) {
            return Some(trimmed.to_string());
        }
    }

    let trimmed = text.trim();
    if !trimmed.is_empty() && matches(lower.trim()) {
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
    fn preferences_room_is_tagged_whatever_the_wording() {
        let rule = "Commit rule from the team: small commits, no trailers.";
        assert_eq!(
            drawer_preference_span(rule, "preferences", PreferenceSource::Filed).as_deref(),
            Some(rule)
        );
    }

    #[test]
    fn filed_drawers_match_third_person_rules() {
        let span = drawer_preference_span(
            "Context first. Standing instruction from Ionut: do NOT use file memory.",
            "decisions",
            PreferenceSource::Filed,
        );
        assert_eq!(
            span.as_deref(),
            Some("Standing instruction from Ionut: do NOT use file memory.")
        );
    }

    #[test]
    fn mined_code_is_never_tagged() {
        let code = "// I prefer this. Always use the facade; never use raw SQL.";
        assert_eq!(
            drawer_preference_span(code, "preferences", PreferenceSource::Code),
            None
        );
    }

    #[test]
    fn conversations_keep_first_person_only() {
        let source = PreferenceSource::infer("/home/me/chats/session.jsonl");
        assert_eq!(source, PreferenceSource::Conversation);
        assert!(drawer_preference_span("You must not do that.", "general", source).is_none());
        assert!(drawer_preference_span("I prefer tabs.", "general", source).is_some());
    }

    #[test]
    fn infers_source_from_source_file() {
        assert_eq!(PreferenceSource::infer(""), PreferenceSource::Filed);
        assert_eq!(
            PreferenceSource::infer("palace_remember/abc123"),
            PreferenceSource::Filed
        );
        assert_eq!(
            PreferenceSource::infer("/repo/src/main.rs"),
            PreferenceSource::Code
        );
    }

    #[test]
    fn ignores_neutral_text() {
        assert!(!is_preference("The function returns a vector of strings"));
        assert!(!is_preference("cargo fmt runs the formatter"));
        assert!(!is_preference("This was fixed in commit abc123"));
    }
}
