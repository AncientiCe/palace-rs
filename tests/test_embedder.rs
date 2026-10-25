use palace::embedder::{embed_document, embed_documents, embed_query};

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

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max)
}

#[test]
fn batched_embeddings_match_single_embeddings() {
    let texts = mixed_length_texts();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let batched = embed_documents(&refs).expect("batch embedding");
    assert_eq!(batched.len(), texts.len());
    for (text, batch_vec) in refs.iter().zip(&batched) {
        let single = embed_document(text).expect("single embedding");
        let diff = max_diff(&single, batch_vec);
        assert!(diff < 1e-5, "batch drifted from single by {diff}");
    }
}

#[test]
fn default_model_embeds_queries_and_documents_alike() {
    // MiniLM has no query prefix, so a query and a document of the same
    // text must give the same vector.
    let text = "why did we choose bundled sqlite?";
    let query = embed_query(text).expect("query embedding");
    let document = embed_document(text).expect("document embedding");
    assert_eq!(query.len(), palace::embedder::EMBEDDING_DIM);
    assert!(max_diff(&query, &document) < 1e-6);
}

#[test]
#[allow(deprecated)]
fn deprecated_wrappers_still_embed_documents() {
    let text = "downstream crates still call embed_one and embed_batch";
    let one = palace::embedder::embed_one(text).expect("embed_one");
    let batch = palace::embedder::embed_batch(&[text]).expect("embed_batch");
    let document = embed_document(text).expect("embed_document");
    assert!(max_diff(&one, &document) < 1e-6);
    assert!(max_diff(&batch[0], &document) < 1e-6);
}
