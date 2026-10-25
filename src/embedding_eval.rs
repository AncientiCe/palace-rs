//! Known-item retrieval probes for comparing embedding models on a palace.
//!
//! Palace does not keep raw queries (usage events store only a hash), so a
//! model is judged on probes made from the palace itself: take one inner
//! sentence of a drawer, drop about 30% of its words, and check whether a
//! search for that text returns the drawer. Probes are lexical slices, so
//! they favour BM25; vector-only recall is reported next to hybrid recall to
//! show what the model alone contributes.

/// Small deterministic PRNG (xorshift64*), so probe sets are reproducible
/// from a seed without pulling in a random-number crate.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform value in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// Choose `k` distinct indices from `0..n`, in ascending order.
    pub fn sample_indices(&mut self, n: usize, k: usize) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..n).collect();
        let k = k.min(n);
        for i in 0..k {
            let j = i + self.below(n - i);
            indices.swap(i, j);
        }
        let mut chosen = indices[..k].to_vec();
        chosen.sort_unstable();
        chosen
    }
}

/// Fewest words a sentence needs to make a probe.
const MIN_PROBE_WORDS: usize = 6;
/// Most words kept in a probe; long sentences are cut to a window.
const MAX_PROBE_WORDS: usize = 24;

/// Build a known-item probe from `content`: one inner sentence with roughly
/// 30% of its words dropped. Returns `None` when no sentence is long enough.
pub fn known_item_probe(content: &str, rng: &mut Rng) -> Option<String> {
    let sentences: Vec<Vec<&str>> = content
        .split(['.', '!', '?', '\n'])
        .map(|sentence| sentence.split_whitespace().collect::<Vec<_>>())
        .filter(|words| words.len() >= MIN_PROBE_WORDS)
        .collect();
    if sentences.is_empty() {
        return None;
    }
    // Prefer inner sentences: titles and first lines are the easiest hits.
    let pick = if sentences.len() > 2 {
        1 + rng.below(sentences.len() - 2)
    } else {
        rng.below(sentences.len())
    };
    let words = &sentences[pick];
    let start = if words.len() > MAX_PROBE_WORDS {
        rng.below(words.len() - MAX_PROBE_WORDS + 1)
    } else {
        0
    };
    let window = &words[start..(start + MAX_PROBE_WORDS).min(words.len())];
    let kept: Vec<&str> = window
        .iter()
        .copied()
        .filter(|_| rng.below(10) >= 3)
        .collect();
    if kept.len() < MIN_PROBE_WORDS / 2 {
        return None;
    }
    Some(kept.join(" "))
}

/// Recall at 1 and at 5 over a set of ranked result lists.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Recall {
    pub at_1: f64,
    pub at_5: f64,
}

/// Recall@1/@5 for `(gold_id, ranked_ids)` pairs.
pub fn recall<'a>(cases: impl IntoIterator<Item = (&'a str, Vec<String>)>) -> Recall {
    let mut total = 0usize;
    let mut top1 = 0usize;
    let mut top5 = 0usize;
    for (gold, ranked) in cases {
        total += 1;
        if ranked.first().is_some_and(|id| id == gold) {
            top1 += 1;
        }
        if ranked.iter().take(5).any(|id| id == gold) {
            top5 += 1;
        }
    }
    if total == 0 {
        return Recall::default();
    }
    Recall {
        at_1: top1 as f64 / total as f64,
        at_5: top5 as f64 / total as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_are_deterministic_per_seed() {
        let text = "Title line here.\nWe chose bundled SQLite because users should not install a system database. Short.";
        let a = known_item_probe(text, &mut Rng::new(7));
        let b = known_item_probe(text, &mut Rng::new(7));
        assert_eq!(a, b);
        let probe = a.expect("a sentence is long enough");
        assert!(probe.split_whitespace().count() < 13);
    }

    #[test]
    fn probe_needs_a_long_enough_sentence() {
        assert!(known_item_probe("too short. also short", &mut Rng::new(1)).is_none());
    }

    #[test]
    fn sample_indices_are_distinct_and_bounded() {
        let picked = Rng::new(42).sample_indices(10, 4);
        assert_eq!(picked.len(), 4);
        assert!(picked.windows(2).all(|w| w[0] < w[1]));
        assert!(picked.iter().all(|&i| i < 10));
        assert_eq!(Rng::new(1).sample_indices(3, 10).len(), 3);
    }

    #[test]
    fn recall_counts_hits() {
        let r = recall([
            ("a", vec!["a".to_string(), "b".to_string()]),
            ("b", vec!["a".to_string(), "b".to_string()]),
            ("c", vec!["a".to_string()]),
            ("d", vec![]),
        ]);
        assert_eq!(r.at_1, 0.25);
        assert_eq!(r.at_5, 0.5);
    }
}
