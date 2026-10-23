use palace::embedder::{embed_batch, embed_one};

/// Texts of very different lengths, so a batch mixes short and long inputs.
fn mixed_length_texts() -> Vec<String> {
    let sentence =
        "The palace stores verbatim drawers and ranks them with BM25 plus cosine similarity. ";
    (0..40)
        .map(|i| match i % 4 {
            0 => format!("short note {i}"),
            1 => sentence.repeat(2 + i % 3),
            2 => sentence.repeat(12 + i % 5),
            _ => sentence.repeat(60),
        })
        .collect()
}

#[test]
fn batched_embeddings_match_single_embeddings() {
    let texts = mixed_length_texts();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let batched = embed_batch(&refs).expect("batch embedding");
    assert_eq!(batched.len(), texts.len());
    for (text, batch_vec) in refs.iter().zip(&batched) {
        let single = embed_one(text).expect("single embedding");
        assert_eq!(single.len(), batch_vec.len());
        let max_diff = single
            .iter()
            .zip(batch_vec)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(max_diff < 1e-5, "batch drifted from single by {max_diff}");
    }
}
