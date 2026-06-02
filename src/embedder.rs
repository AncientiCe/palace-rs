//! Text embedding using ONNX Runtime with all-MiniLM-L6-v2 (384-dim).
//!
//! Uses OnceLock for lazy initialization — commands that don't need embeddings
//! (e.g. `palace status`) start instantly without loading the model.
//! The model is downloaded on first use and cached in ~/.cache/huggingface.

use anyhow::{Context, Result};
use ndarray::Array;
use once_cell::sync::OnceCell;
use ort::{
    session::{builder::GraphOptimizationLevel, Session},
    value::Value,
};
use std::collections::HashMap;
use std::sync::Mutex;
use std::thread::available_parallelism;
use std::time::Duration;
use unicode_normalization::UnicodeNormalization;

/// Embedding dimension for all-MiniLM-L6-v2.
pub const EMBEDDING_DIM: usize = 384;

const MODEL_REPO: &str = "Qdrant/all-MiniLM-L6-v2-onnx";
const MODEL_FILE: &str = "model.onnx";
const VOCAB_FILE: &str = "vocab.txt";
const MAX_SEQ_LEN: usize = 512;

// ── Minimal BERT WordPiece tokenizer ────────────────────────────────────────

struct BertTokenizer {
    token_to_id: HashMap<String, i64>,
    cls_id: i64,
    sep_id: i64,
    unk_id: i64,
    pad_id: i64,
}

impl BertTokenizer {
    fn from_vocab_file(path: &std::path::Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).context("reading vocab.txt")?;
        let token_to_id: HashMap<String, i64> = content
            .lines()
            .enumerate()
            .map(|(i, tok)| (tok.to_string(), i as i64))
            .collect();

        let cls_id = *token_to_id
            .get("[CLS]")
            .ok_or_else(|| anyhow::anyhow!("vocab missing [CLS]"))?;
        let sep_id = *token_to_id
            .get("[SEP]")
            .ok_or_else(|| anyhow::anyhow!("vocab missing [SEP]"))?;
        let unk_id = *token_to_id
            .get("[UNK]")
            .ok_or_else(|| anyhow::anyhow!("vocab missing [UNK]"))?;
        let pad_id = *token_to_id.get("[PAD]").unwrap_or(&0);

        Ok(Self {
            token_to_id,
            cls_id,
            sep_id,
            unk_id,
            pad_id,
        })
    }

    fn encode(&self, text: &str, max_len: usize) -> EncodedInput {
        let cleaned = clean_and_lower(text);
        let basic_tokens = basic_tokenize(&cleaned);

        let mut wordpieces = Vec::new();
        for token in &basic_tokens {
            wordpiece_tokenize(token, &self.token_to_id, self.unk_id, &mut wordpieces);
        }

        // Reserve 2 for [CLS] and [SEP]
        let effective_max = max_len.saturating_sub(2);
        if wordpieces.len() > effective_max {
            wordpieces.truncate(effective_max);
        }

        let mut input_ids = Vec::with_capacity(wordpieces.len() + 2);
        input_ids.push(self.cls_id);
        input_ids.extend_from_slice(&wordpieces);
        input_ids.push(self.sep_id);

        let seq_len = input_ids.len();
        let attention_mask = vec![1i64; seq_len];
        let token_type_ids = vec![0i64; seq_len];

        EncodedInput {
            input_ids,
            attention_mask,
            token_type_ids,
        }
    }
}

struct EncodedInput {
    input_ids: Vec<i64>,
    attention_mask: Vec<i64>,
    token_type_ids: Vec<i64>,
}

fn clean_and_lower(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.nfd() {
        if c.is_control() || c == '\0' {
            continue;
        }
        if is_combining_mark(c) {
            continue;
        }
        if is_whitespace_char(c) {
            out.push(' ');
        } else {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
        }
    }
    out
}

fn is_combining_mark(c: char) -> bool {
    unicode_normalization::char::canonical_combining_class(c) != 0
}

fn is_whitespace_char(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r') || {
        let cat = c as u32;
        // Unicode Zs category (space separators)
        cat == 0x00A0
            || cat == 0x1680
            || (0x2000..=0x200A).contains(&cat)
            || cat == 0x202F
            || cat == 0x205F
            || cat == 0x3000
    }
}

fn is_punctuation(c: char) -> bool {
    let cp = c as u32;
    (0x21..=0x2F).contains(&cp)
        || (0x3A..=0x40).contains(&cp)
        || (0x5B..=0x60).contains(&cp)
        || (0x7B..=0x7E).contains(&cp)
        || c.is_ascii_punctuation()
        || {
            // Unicode punctuation categories (P*)
            matches!(unicode_general_category(c), UnicodeCategory::Punctuation)
        }
}

fn is_cjk_character(c: char) -> bool {
    let cp = c as u32;
    (0x4E00..=0x9FFF).contains(&cp)
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x20000..=0x2A6DF).contains(&cp)
        || (0x2A700..=0x2B73F).contains(&cp)
        || (0x2B740..=0x2B81F).contains(&cp)
        || (0x2B820..=0x2CEAF).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0x2F800..=0x2FA1F).contains(&cp)
}

#[derive(PartialEq)]
enum UnicodeCategory {
    Punctuation,
    Other,
}

fn unicode_general_category(c: char) -> UnicodeCategory {
    // Check Unicode general category for punctuation (Pc, Pd, Pe, Pf, Pi, Po, Ps)
    // Using char ranges from Unicode
    if c.is_ascii_punctuation() {
        return UnicodeCategory::Punctuation;
    }
    let cp = c as u32;
    // Common Unicode punctuation ranges
    if (0x2010..=0x2027).contains(&cp)
        || (0x2030..=0x205E).contains(&cp)
        || (0x2308..=0x230B).contains(&cp)
        || (0x2768..=0x2775).contains(&cp)
        || (0x27E6..=0x27EF).contains(&cp)
        || (0x2983..=0x2998).contains(&cp)
        || (0x3001..=0x3003).contains(&cp)
        || (0x3008..=0x3011).contains(&cp)
        || (0x3014..=0x301F).contains(&cp)
        || (0xFE30..=0xFE4F).contains(&cp)
        || (0xFF01..=0xFF0F).contains(&cp)
        || (0xFF1A..=0xFF20).contains(&cp)
        || (0xFF3B..=0xFF3D).contains(&cp)
        || (0xFF5B..=0xFF65).contains(&cp)
    {
        UnicodeCategory::Punctuation
    } else {
        UnicodeCategory::Other
    }
}

fn basic_tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for word in text.split_whitespace() {
        let mut current = String::new();
        for c in word.chars() {
            if is_punctuation(c) || is_cjk_character(c) {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                tokens.push(c.to_string());
            } else {
                current.push(c);
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }
    }
    tokens
}

fn wordpiece_tokenize(
    token: &str,
    vocab: &HashMap<String, i64>,
    unk_id: i64,
    output: &mut Vec<i64>,
) {
    if token.is_empty() {
        return;
    }
    if let Some(&id) = vocab.get(token) {
        output.push(id);
        return;
    }

    let chars: Vec<char> = token.chars().collect();
    let mut start = 0;
    let mut is_bad = false;

    while start < chars.len() {
        let mut end = chars.len();
        let mut found = false;

        while start < end {
            let substr: String = if start > 0 {
                let mut s = String::from("##");
                s.extend(&chars[start..end]);
                s
            } else {
                chars[start..end].iter().collect()
            };

            if let Some(&id) = vocab.get(&substr) {
                output.push(id);
                found = true;
                start = end;
                break;
            }
            end -= 1;
        }

        if !found {
            is_bad = true;
            break;
        }
    }

    if is_bad {
        // Clear any partial results and push UNK
        output.truncate(output.len().saturating_sub(chars.len()));
        output.push(unk_id);
    }
}

// ── Embedder ────────────────────────────────────────────────────────────────

struct Embedder {
    tokenizer: BertTokenizer,
    session: Mutex<Session>,
    need_token_type_ids: bool,
}

static EMBEDDER: OnceCell<Embedder> = OnceCell::new();

/// How many times to attempt each model-file download before giving up.
const DOWNLOAD_ATTEMPTS: usize = 4;
/// Base delay for exponential backoff between download attempts.
const DOWNLOAD_BASE_DELAY: Duration = Duration::from_millis(500);

/// Retry `f` up to `attempts` times with exponential backoff, returning the
/// last error if every attempt fails.
///
/// This guards model downloads against transient HuggingFace failures —
/// notably HTTP 429 rate limiting, which otherwise fails CI runs that fetch the
/// model fresh.
fn retry<T, F>(attempts: usize, base_delay: Duration, mut f: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let attempts = attempts.max(1);
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..attempts {
        match f() {
            Ok(value) => return Ok(value),
            Err(err) => last_err = Some(err),
        }
        if attempt + 1 < attempts {
            let factor = 2u32.saturating_pow(attempt as u32);
            let delay = base_delay.checked_mul(factor).unwrap_or(base_delay);
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("retry: no attempts were made")))
}

fn download_model_files() -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    use hf_hub::api::sync::Api;

    let api = Api::new().context("initializing HuggingFace API")?;
    let repo = api.model(MODEL_REPO.to_string());

    let model_path = retry(DOWNLOAD_ATTEMPTS, DOWNLOAD_BASE_DELAY, || {
        repo.get(MODEL_FILE).context("downloading ONNX model file")
    })?;
    let vocab_path = retry(DOWNLOAD_ATTEMPTS, DOWNLOAD_BASE_DELAY, || {
        repo.get(VOCAB_FILE).context("downloading vocab file")
    })?;

    Ok((model_path, vocab_path))
}

fn init_embedder() -> Result<Embedder> {
    let (model_path, vocab_path) = download_model_files()?;

    let threads = available_parallelism().map(|n| n.get()).unwrap_or(1);

    let session = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(threads)?
        .commit_from_file(&model_path)
        .context("loading ONNX session")?;

    let need_token_type_ids = session
        .inputs()
        .iter()
        .any(|input| input.name() == "token_type_ids");

    let tokenizer =
        BertTokenizer::from_vocab_file(&vocab_path).context("loading BERT tokenizer")?;

    Ok(Embedder {
        tokenizer,
        session: Mutex::new(session),
        need_token_type_ids,
    })
}

fn get_embedder() -> Result<&'static Embedder> {
    EMBEDDER.get_or_try_init(|| init_embedder().context("initializing embedder (all-MiniLM-L6-v2)"))
}

fn run_inference(embedder: &Embedder, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(vec![]);
    }

    let encodings: Vec<_> = texts
        .iter()
        .map(|text| embedder.tokenizer.encode(text, MAX_SEQ_LEN))
        .collect();

    let max_len = encodings
        .iter()
        .map(|e| e.input_ids.len())
        .max()
        .unwrap_or(0);
    let batch_size = encodings.len();
    let pad_id = embedder.tokenizer.pad_id;

    let mut ids_flat = Vec::with_capacity(batch_size * max_len);
    let mut mask_flat = Vec::with_capacity(batch_size * max_len);
    let mut type_ids_flat = Vec::with_capacity(batch_size * max_len);

    for enc in &encodings {
        let seq_len = enc.input_ids.len();
        ids_flat.extend_from_slice(&enc.input_ids);
        mask_flat.extend_from_slice(&enc.attention_mask);
        type_ids_flat.extend_from_slice(&enc.token_type_ids);

        let padding = max_len.saturating_sub(seq_len);
        ids_flat.extend(std::iter::repeat_n(pad_id, padding));
        mask_flat.extend(std::iter::repeat_n(0i64, padding));
        type_ids_flat.extend(std::iter::repeat_n(0i64, padding));
    }

    let ids_array =
        Array::from_shape_vec((batch_size, max_len), ids_flat).context("shaping input_ids")?;
    let mask_array = Array::from_shape_vec((batch_size, max_len), mask_flat.clone())
        .context("shaping attention_mask")?;

    let mut session_inputs = ort::inputs![
        "input_ids" => Value::from_array(ids_array)?,
        "attention_mask" => Value::from_array(mask_array)?,
    ];

    if embedder.need_token_type_ids {
        let type_ids_array = Array::from_shape_vec((batch_size, max_len), type_ids_flat)
            .context("shaping token_type_ids")?;
        session_inputs.push((
            "token_type_ids".into(),
            Value::from_array(type_ids_array)?.into(),
        ));
    }

    let mut session = embedder
        .session
        .lock()
        .map_err(|e| anyhow::anyhow!("session lock poisoned: {e}"))?;
    let outputs = session
        .run(session_inputs)
        .context("running ONNX inference")?;

    let output_tensor = outputs
        .values()
        .next()
        .ok_or_else(|| anyhow::anyhow!("model returned no outputs"))?;

    let output_array = output_tensor
        .try_extract_array::<f32>()
        .context("extracting output tensor")?;

    let shape = output_array.shape();
    let embeddings = match shape.len() {
        2 => (0..batch_size)
            .map(|i| {
                let row: Vec<f32> = output_array
                    .slice(ndarray::s![i, ..])
                    .iter()
                    .copied()
                    .collect();
                l2_normalize(&row)
            })
            .collect(),
        3 => {
            let dim = shape[2];
            (0..batch_size)
                .map(|i| {
                    let mut result = vec![0.0f32; dim];
                    let mut mask_sum = vec![0.0f32; dim];
                    let offset = i * max_len;
                    for j in 0..max_len {
                        let m = mask_flat[offset + j] as f32;
                        for k in 0..dim {
                            let val = output_array[[i, j, k]];
                            result[k] += val * m;
                            mask_sum[k] += m;
                        }
                    }
                    for k in 0..dim {
                        if mask_sum[k] > 0.0 {
                            result[k] /= mask_sum[k];
                        }
                    }
                    l2_normalize(&result)
                })
                .collect()
        }
        _ => return Err(anyhow::anyhow!("unexpected output tensor shape: {shape:?}")),
    };

    Ok(embeddings)
}

fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let epsilon = 1e-12;
    v.iter().map(|&x| x / (norm + epsilon)).collect()
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Embed a single piece of text, returning a 384-dimensional vector.
pub fn embed_one(text: &str) -> Result<Vec<f32>> {
    let embedder = get_embedder()?;
    let mut results = run_inference(embedder, &[text]).context("embedding text")?;
    results
        .pop()
        .ok_or_else(|| anyhow::anyhow!("embedding returned empty result"))
}

/// Embed multiple texts in one batch. Returns a Vec<Vec<f32>>.
pub fn embed_batch(texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(vec![]);
    }
    let embedder = get_embedder()?;
    run_inference(embedder, texts).context("batch embedding")
}

/// Serialize a f32 vector to little-endian bytes for SQLite BLOB storage.
pub fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Deserialize a BLOB back to a f32 vector.
pub fn blob_to_vec(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

/// Cosine similarity between two equal-length f32 vectors.
/// Returns a value in [-1.0, 1.0]; higher is more similar.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "vectors must have equal length");
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn retry_recovers_from_transient_failures() {
        let calls = Cell::new(0);
        let result: Result<u32> = retry(4, Duration::ZERO, || {
            let n = calls.get() + 1;
            calls.set(n);
            if n < 3 {
                Err(anyhow::anyhow!("http status: 429"))
            } else {
                Ok(n)
            }
        });
        assert_eq!(result.unwrap(), 3, "should succeed once the call recovers");
        assert_eq!(
            calls.get(),
            3,
            "should stop retrying after the first success"
        );
    }

    #[test]
    fn retry_gives_up_after_exhausting_attempts() {
        let calls = Cell::new(0);
        let result: Result<u32> = retry(3, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            Err(anyhow::anyhow!("persistent 429"))
        });
        assert!(result.is_err(), "should surface the failure after retries");
        assert_eq!(calls.get(), 3, "should attempt exactly `attempts` times");
    }
}
