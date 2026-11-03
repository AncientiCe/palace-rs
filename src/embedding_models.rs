//! Registry of embedding models Palace can run.
//!
//! Every entry is a BERT WordPiece model exported to ONNX, so one tokenizer
//! and one inference path serve them all; entries differ in where the files
//! live, how token vectors are pooled, and which text prefixes the model was
//! trained with. A `(model, precision)` pair is one entry: `bge-small` and
//! `bge-small-q8` are separate ids because they produce different vectors.

use std::path::PathBuf;

/// How token embeddings are reduced to one sentence vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pooling {
    /// Average over non-padding tokens (sentence-transformers models).
    Mean,
    /// Take the `[CLS]` token (BGE models).
    Cls,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelSpec {
    /// Stable id stored in `meta.active_embedding_model` and used on the CLI.
    pub id: &'static str,
    /// Upstream model name, for display.
    pub name: &'static str,
    /// Weight precision: `fp32` or `int8`.
    pub precision: &'static str,
    /// HuggingFace repository holding the ONNX export.
    pub repo: &'static str,
    /// Path of the ONNX file inside `repo`.
    pub model_file: &'static str,
    /// Path of the WordPiece vocabulary inside `repo`.
    pub vocab_file: &'static str,
    pub pooling: Pooling,
    /// Vector length.
    pub dim: usize,
    /// Token cap, including `[CLS]` and `[SEP]`.
    pub max_seq_len: usize,
    /// Prepended to search queries (asymmetric retrieval models).
    pub query_prefix: &'static str,
    /// Prepended to stored documents.
    pub doc_prefix: &'static str,
    /// Size of the ONNX file plus vocabulary, in bytes.
    pub download_bytes: u64,
    /// Cosine similarity typical of unrelated texts; hybrid ranking rescales
    /// cosine from this floor to 1.0 so models with a compressed range
    /// (BGE) contribute as much spread as MiniLM.
    pub cosine_floor: f64,
    /// Hybrid-ranking weight of cosine similarity for this model.
    pub cosine_weight: f64,
    /// Hybrid-ranking weight of normalized BM25 for this model.
    pub bm25_weight: f64,
    /// Did not pass the release gate (coding-agent eval recall at least
    /// MiniLM's); offered for users who want to try it on their palace,
    /// where `verify` still guards the switch.
    pub experimental: bool,
    /// One-line description for `palace embeddings list`.
    pub summary: &'static str,
}

/// Id of the model new palaces start on unless configured otherwise.
pub const DEFAULT_MODEL_ID: &str = "minilm-l6-v2-q8";

/// Id of the model every palace used before palaces recorded their model
/// (before 0.18.0). A palace with vectors and no recorded model is on it.
pub const LEGACY_MODEL_ID: &str = "minilm-l6-v2";

const BGE_QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

/// Every model Palace can run. The first entry is the default.
///
/// Ranking settings come from the bake-off (`tests/embedding_bakeoff.rs`,
/// `PALACE_BAKEOFF_TUNE=1`): BGE scores unrelated text near 0.5 cosine, so
/// it ranks best with a 0.5 floor and an even cosine/BM25 split.
pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: DEFAULT_MODEL_ID,
        name: "all-MiniLM-L6-v2",
        precision: "int8",
        repo: "Xenova/all-MiniLM-L6-v2",
        model_file: "onnx/model_quantized.onnx",
        vocab_file: "vocab.txt",
        pooling: Pooling::Mean,
        dim: 384,
        max_seq_len: 512,
        query_prefix: "",
        doc_prefix: "",
        download_bytes: 23_203_878,
        cosine_floor: 0.0,
        cosine_weight: 0.65,
        bm25_weight: 0.35,
        experimental: false,
        summary: "default; MiniLM in int8, same recall as fp32, smaller download, faster indexing",
    },
    ModelSpec {
        id: LEGACY_MODEL_ID,
        name: "all-MiniLM-L6-v2",
        precision: "fp32",
        repo: "Qdrant/all-MiniLM-L6-v2-onnx",
        model_file: "model.onnx",
        vocab_file: "vocab.txt",
        pooling: Pooling::Mean,
        dim: 384,
        max_seq_len: 512,
        query_prefix: "",
        doc_prefix: "",
        download_bytes: 90_619_138,
        cosine_floor: 0.0,
        cosine_weight: 0.65,
        bm25_weight: 0.35,
        experimental: false,
        summary: "MiniLM in fp32; palaces made before 0.19 run on it",
    },
    ModelSpec {
        id: "bge-small",
        name: "bge-small-en-v1.5",
        precision: "fp32",
        repo: "Xenova/bge-small-en-v1.5",
        model_file: "onnx/model.onnx",
        vocab_file: "vocab.txt",
        pooling: Pooling::Cls,
        dim: 384,
        max_seq_len: 512,
        query_prefix: BGE_QUERY_PREFIX,
        doc_prefix: "",
        download_bytes: 133_324_998,
        cosine_floor: 0.5,
        cosine_weight: 0.5,
        bm25_weight: 0.5,
        experimental: true,
        summary: "12 layers, ~2x slower indexing; below MiniLM on the coding eval",
    },
    ModelSpec {
        id: "bge-small-q8",
        name: "bge-small-en-v1.5",
        precision: "int8",
        repo: "Xenova/bge-small-en-v1.5",
        model_file: "onnx/model_quantized.onnx",
        vocab_file: "vocab.txt",
        pooling: Pooling::Cls,
        dim: 384,
        max_seq_len: 512,
        query_prefix: BGE_QUERY_PREFIX,
        doc_prefix: "",
        download_bytes: 34_245_934,
        cosine_floor: 0.5,
        cosine_weight: 0.5,
        bm25_weight: 0.5,
        experimental: true,
        summary: "bge-small in int8; ~1.7x slower indexing than MiniLM",
    },
    ModelSpec {
        id: "bge-base-q8",
        name: "bge-base-en-v1.5",
        precision: "int8",
        repo: "Xenova/bge-base-en-v1.5",
        model_file: "onnx/model_quantized.onnx",
        vocab_file: "vocab.txt",
        pooling: Pooling::Cls,
        dim: 768,
        max_seq_len: 512,
        query_prefix: BGE_QUERY_PREFIX,
        doc_prefix: "",
        download_bytes: 110_314_845,
        cosine_floor: 0.5,
        cosine_weight: 0.5,
        bm25_weight: 0.5,
        experimental: true,
        summary: "768-dim, largest download, ~3x slower indexing",
    },
];

/// Look a model up by id.
pub fn find(id: &str) -> Option<&'static ModelSpec> {
    let id = id.trim();
    MODELS.iter().find(|spec| spec.id.eq_ignore_ascii_case(id))
}

/// The default model.
pub fn default_model() -> &'static ModelSpec {
    &MODELS[0]
}

/// Position of `spec` in [`MODELS`].
pub(crate) fn index_of(spec: &ModelSpec) -> usize {
    MODELS
        .iter()
        .position(|candidate| candidate.id == spec.id)
        .unwrap_or(0)
}

/// Comma-separated model ids, for error messages.
pub fn ids() -> String {
    MODELS
        .iter()
        .map(|spec| spec.id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// HuggingFace hub cache directory (`HF_HUB_CACHE` or `~/.cache/huggingface/hub`).
fn hub_cache_dir() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("HF_HUB_CACHE") {
        return Some(PathBuf::from(path));
    }
    directories::UserDirs::new().map(|user| {
        user.home_dir()
            .join(".cache")
            .join("huggingface")
            .join("hub")
    })
}

/// Whether the model's ONNX file is already in the HuggingFace cache.
pub fn is_cached(spec: &ModelSpec) -> bool {
    let Some(cache) = hub_cache_dir() else {
        return false;
    };
    let snapshots = cache
        .join(format!("models--{}", spec.repo.replace('/', "--")))
        .join("snapshots");
    let Ok(entries) = std::fs::read_dir(snapshots) else {
        return false;
    };
    entries
        .filter_map(Result::ok)
        .any(|entry| entry.path().join(spec.model_file).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_default_comes_first() {
        let mut ids: Vec<&str> = MODELS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids[0], DEFAULT_MODEL_ID);
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MODELS.len());
    }

    #[test]
    fn find_ignores_case_and_whitespace() {
        assert_eq!(find(" BGE-Small ").map(|spec| spec.id), Some("bge-small"));
        assert!(find("gpt-embedding").is_none());
    }

    #[test]
    fn default_model_is_not_experimental() {
        assert!(!default_model().experimental);
    }

    #[test]
    fn ranking_weights_sum_to_one() {
        for spec in MODELS {
            assert!(
                (spec.cosine_weight + spec.bm25_weight - 1.0).abs() < 1e-9,
                "{} weights",
                spec.id
            );
        }
    }
}
