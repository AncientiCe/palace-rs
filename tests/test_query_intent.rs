use palace::query_intent::{classify, QueryIntent};

#[test]
fn classifies_preference_queries() {
    assert_eq!(
        classify("what do I prefer for API design?"),
        QueryIntent::Preference
    );
    assert_eq!(
        classify("what is my favorite editor?"),
        QueryIntent::Preference
    );
}

#[test]
fn classifies_decision_queries() {
    assert_eq!(
        classify("why did we choose bundled sqlite?"),
        QueryIntent::Decision
    );
}

#[test]
fn classifies_how_to_queries() {
    assert_eq!(classify("how do I run clippy?"), QueryIntent::HowTo);
}

#[test]
fn classifies_definition_queries() {
    assert_eq!(classify("what is Palace gain?"), QueryIntent::Definition);
}

#[test]
fn classifies_temporal_queries() {
    assert_eq!(
        classify("what changed last session?"),
        QueryIntent::Temporal
    );
}

#[test]
fn classifies_unknown_queries() {
    assert_eq!(classify("blue ceramic angle"), QueryIntent::Unknown);
}
