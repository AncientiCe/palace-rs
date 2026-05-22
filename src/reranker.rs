//! Optional local reranking for top hybrid candidates.
//!
//! The default path stays unchanged. When enabled, the top candidates are
//! rescored with a deterministic query/document interaction score that favors
//! exact phrase overlap, rare-token overlap, and preference-span matches. The
//! module is intentionally dependency-light so enabling rerank never requires a
//! remote API.

use crate::ranker::{tokenize, HybridResult};
use std::collections::HashSet;

pub fn should_rerank(requested: bool) -> bool {
    requested || env_enabled("PALACE_RERANK")
}

pub fn model_name() -> String {
    std::env::var("PALACE_RERANK_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "local-token-interaction-v1".to_string())
}

pub fn rerank(query: &str, results: &mut [HybridResult]) {
    for result in results.iter_mut() {
        let score = interaction_score(query, &result.drawer.text);
        result.rerank_score = Some(score);
        result.combined = ((result.combined * 0.65) + (score * 0.35)).clamp(0.0, 1.5);
        result.drawer.similarity = (result.combined * 1000.0).round() / 1000.0;
    }
    results.sort_by(|a, b| {
        b.combined
            .partial_cmp(&a.combined)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

fn interaction_score(query: &str, text: &str) -> f64 {
    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();
    let query_terms = meaningful_terms(&query_lower);
    if query_terms.is_empty() {
        return 0.0;
    }

    let text_terms = meaningful_terms(&text_lower);
    let overlap = query_terms.intersection(&text_terms).count() as f64 / query_terms.len() as f64;
    let phrase = if text_lower.contains(&query_lower) {
        0.25
    } else {
        0.0
    };
    let preference = if crate::preference::is_preference(text)
        && crate::query_intent::classify(query) == crate::query_intent::QueryIntent::Preference
    {
        0.20
    } else {
        0.0
    };
    (overlap * 0.75 + phrase + preference).min(1.0)
}

fn meaningful_terms(text: &str) -> HashSet<String> {
    tokenize(text)
        .into_iter()
        .filter(|term| term.len() > 2 && !STOP_WORDS.contains(&term.as_str()))
        .collect()
}

fn env_enabled(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "what", "when", "where", "why", "how", "did", "does", "that",
    "this", "from", "are", "was", "were", "you", "your", "our", "about",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SearchResult;

    fn result(text: &str) -> HybridResult {
        HybridResult {
            drawer: SearchResult {
                id: text.to_string(),
                text: text.to_string(),
                wing: "w".to_string(),
                room: "r".to_string(),
                source_file: "s".to_string(),
                created_at: String::new(),
                filed_at: String::new(),
                similarity: 0.0,
            },
            cosine: 0.0,
            bm25: 0.0,
            coding_boost: 0.0,
            preference_match: 0.0,
            rerank_score: None,
            combined: 0.3,
        }
    }

    #[test]
    fn rerank_sets_scores_and_prefers_interaction_match() {
        let mut results = vec![
            result("unrelated database note"),
            result("run cargo clippy with all targets and all features"),
        ];

        rerank("how do I run cargo clippy", &mut results);

        assert!(results.iter().all(|result| result.rerank_score.is_some()));
        assert_eq!(
            results[0].drawer.text,
            "run cargo clippy with all targets and all features"
        );
    }
}
