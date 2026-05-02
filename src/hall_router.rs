//! Route drawer content into broad palace halls.

const HALL_KEYWORDS: &[(&str, &[&str])] = &[
    (
        "technical",
        &[
            "code", "rust", "python", "database", "sqlite", "api", "bug", "error",
        ],
    ),
    (
        "emotions",
        &["feel", "happy", "sad", "angry", "afraid", "worried", "love"],
    ),
    (
        "family",
        &[
            "family", "parent", "mother", "father", "daughter", "son", "children",
        ],
    ),
    (
        "identity",
        &["identity", "persona", "name", "self", "who am i"],
    ),
    (
        "consciousness",
        &["conscious", "aware", "real", "alive", "soul"],
    ),
    (
        "creative",
        &["design", "story", "music", "art", "game", "player"],
    ),
    (
        "memory",
        &["memory", "remember", "recall", "forget", "archive"],
    ),
];

pub fn detect_hall(content: &str) -> String {
    let content = content.to_lowercase();
    HALL_KEYWORDS
        .iter()
        .map(|(hall, keywords)| {
            let score = keywords
                .iter()
                .filter(|keyword| content.contains(**keyword))
                .count();
            (*hall, score)
        })
        .max_by_key(|(_, score)| *score)
        .and_then(|(hall, score)| {
            if score > 0 {
                Some(hall.to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "memory".to_string())
}
