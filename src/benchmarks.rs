//! Retrieval benchmark adapters and runner.
//!
//! This module is intentionally behind the non-default `benchmarks` feature.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Instant;

use crate::embedding_models::ModelSpec;
use crate::ranker::hybrid_search_in;
use crate::store::{add_drawer_with_id, DrawerFilter, VectorSource};

const TOP_K: usize = 10;

/// Session vectors for one model, keyed by a hash of the session text.
/// LongMemEval haystacks share about a fifth of their sessions, so each
/// distinct session is embedded once per run.
#[derive(Default)]
struct EmbeddingCache {
    vectors: HashMap<u64, Vec<f32>>,
}

impl EmbeddingCache {
    fn embed(&mut self, model: &'static ModelSpec, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let keys: Vec<u64> = texts.iter().map(|text| text_key(text)).collect();
        let mut missing_keys = Vec::new();
        let mut missing_texts = Vec::new();
        for (key, text) in keys.iter().zip(texts) {
            if !self.vectors.contains_key(key) && !missing_keys.contains(key) {
                missing_keys.push(*key);
                missing_texts.push(*text);
            }
        }
        if !missing_texts.is_empty() {
            let embeddings = crate::embedder::embed_documents_with(model, &missing_texts)?;
            if embeddings.len() != missing_texts.len() {
                anyhow::bail!(
                    "embed_documents_with returned {} vectors for {} inputs",
                    embeddings.len(),
                    missing_texts.len()
                );
            }
            self.vectors
                .extend(missing_keys.into_iter().zip(embeddings));
        }
        Ok(keys.iter().map(|key| self.vectors[key].clone()).collect())
    }
}

fn text_key(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkSession {
    pub session_id: String,
    pub date: Option<String>,
    pub content: String,
    /// The session as the dataset stores it (LongMemEval: a list of
    /// role/content turns), for chunking modes that need the original turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkInstance {
    pub question_id: String,
    pub question_type: String,
    pub question: String,
    pub question_date: Option<String>,
    pub conversation_id: Option<String>,
    pub sessions: Vec<BenchmarkSession>,
    pub answer_session_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitEvaluation {
    pub hit_at_1: bool,
    pub hit_at_5: bool,
    pub hit_at_10: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeSummary {
    pub evaluated_questions: usize,
    pub hits_at_1: usize,
    pub hits_at_5: usize,
    pub hits_at_10: usize,
    pub recall_at_1: f64,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
}

impl TypeSummary {
    fn new() -> Self {
        Self {
            evaluated_questions: 0,
            hits_at_1: 0,
            hits_at_5: 0,
            hits_at_10: 0,
            recall_at_1: 0.0,
            recall_at_5: 0.0,
            recall_at_10: 0.0,
        }
    }

    fn record(&mut self, hits: &HitEvaluation) {
        self.evaluated_questions += 1;
        self.hits_at_1 += usize::from(hits.hit_at_1);
        self.hits_at_5 += usize::from(hits.hit_at_5);
        self.hits_at_10 += usize::from(hits.hit_at_10);
        self.recompute();
    }

    fn recompute(&mut self) {
        if self.evaluated_questions == 0 {
            return;
        }
        let denominator = self.evaluated_questions as f64;
        self.recall_at_1 = self.hits_at_1 as f64 / denominator;
        self.recall_at_5 = self.hits_at_5 as f64 / denominator;
        self.recall_at_10 = self.hits_at_10 as f64 / denominator;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkSummary {
    pub benchmark: String,
    /// Embedding model id, or `bm25-only`.
    pub model: String,
    pub evaluated_questions: usize,
    pub skipped_questions: usize,
    pub unscored_questions: usize,
    pub failed_questions: usize,
    pub drawers_indexed: usize,
    pub ingest_ms: u128,
    pub query_ms: u128,
    pub hits_at_1: usize,
    pub hits_at_5: usize,
    pub hits_at_10: usize,
    pub recall_at_1: f64,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub per_question_type: BTreeMap<String, TypeSummary>,
}

impl BenchmarkSummary {
    pub fn new(benchmark: impl Into<String>) -> Self {
        Self {
            benchmark: benchmark.into(),
            model: String::new(),
            evaluated_questions: 0,
            skipped_questions: 0,
            unscored_questions: 0,
            failed_questions: 0,
            drawers_indexed: 0,
            ingest_ms: 0,
            query_ms: 0,
            hits_at_1: 0,
            hits_at_5: 0,
            hits_at_10: 0,
            recall_at_1: 0.0,
            recall_at_5: 0.0,
            recall_at_10: 0.0,
            per_question_type: BTreeMap::new(),
        }
    }

    pub fn record(&mut self, question_type: &str, hits: &HitEvaluation) {
        self.evaluated_questions += 1;
        self.hits_at_1 += usize::from(hits.hit_at_1);
        self.hits_at_5 += usize::from(hits.hit_at_5);
        self.hits_at_10 += usize::from(hits.hit_at_10);
        self.per_question_type
            .entry(question_type.to_string())
            .or_insert_with(TypeSummary::new)
            .record(hits);
        self.recompute();
    }

    fn recompute(&mut self) {
        if self.evaluated_questions == 0 {
            return;
        }
        let denominator = self.evaluated_questions as f64;
        self.recall_at_1 = self.hits_at_1 as f64 / denominator;
        self.recall_at_5 = self.hits_at_5 as f64 / denominator;
        self.recall_at_10 = self.hits_at_10 as f64 / denominator;
    }
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub benchmark: String,
    pub input_path: std::path::PathBuf,
    pub output_dir: std::path::PathBuf,
    pub limit: Option<usize>,
    pub bm25_only: bool,
    /// Embeds sessions and questions, and sets the cosine floor and
    /// cosine/BM25 weights for ranking. Ignored with `bm25_only`.
    pub model: &'static ModelSpec,
    pub chunking: Chunking,
}

/// How sessions become drawers. Chunked modes rank a session by its best chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Chunking {
    /// One drawer per session, the usual LongMemEval granularity.
    #[default]
    Session,
    /// The project miner's `chunk_text`: 800-character chunks.
    Text,
    /// The conversation miner's path: `normalize_content`, then one chunk per
    /// user turn and its reply (`chunk_exchanges`).
    Exchange,
}

impl Chunking {
    pub fn as_str(self) -> &'static str {
        match self {
            Chunking::Session => "session",
            Chunking::Text => "text",
            Chunking::Exchange => "exchange",
        }
    }
}

#[derive(Debug, Serialize)]
struct CaseOutput {
    question_id: String,
    question_type: String,
    scored: bool,
    answer_session_ids: Vec<String>,
    ranked_session_ids: Vec<String>,
    hit_at_1: Option<bool>,
    hit_at_5: Option<bool>,
    hit_at_10: Option<bool>,
    ingest_ms: u128,
    query_ms: u128,
    drawer_count: usize,
    error: Option<String>,
}

pub fn parse_longmemeval(raw: &str) -> Result<Vec<BenchmarkInstance>> {
    let values: Vec<Value> = serde_json::from_str(raw).context("parsing LongMemEval JSON")?;
    let mut instances = Vec::new();

    for value in values {
        let question_id = string_field(&value, "question_id").unwrap_or_default();
        if question_id.ends_with("_abs") {
            continue;
        }

        let question_type = string_field(&value, "question_type").unwrap_or_default();
        let question = string_field(&value, "question").unwrap_or_default();
        let question_date = string_field(&value, "question_date");
        let session_ids = string_array_field(&value, "haystack_session_ids");
        let dates = string_array_field(&value, "haystack_dates");
        let answer_session_ids = string_array_field(&value, "answer_session_ids");
        let haystack_sessions = value
            .get("haystack_sessions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut sessions = Vec::with_capacity(haystack_sessions.len());
        for (index, session_value) in haystack_sessions.iter().enumerate() {
            let session_id = session_ids
                .get(index)
                .cloned()
                .unwrap_or_else(|| format!("session-{index}"));
            sessions.push(BenchmarkSession {
                session_id,
                date: dates.get(index).cloned(),
                content: session_content(session_value),
                raw: Some(session_value.clone()),
            });
        }

        instances.push(BenchmarkInstance {
            question_id,
            question_type,
            question,
            question_date,
            conversation_id: None,
            sessions,
            answer_session_ids,
        });
    }

    Ok(instances)
}

pub fn parse_beam_jsonl(raw: &str) -> Result<Vec<BenchmarkInstance>> {
    let mut instances = Vec::new();
    for (line_index, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(trimmed)
            .with_context(|| format!("parsing BEAM JSONL line {}", line_index + 1))?;
        instances.push(parse_beam_instance(&value, line_index)?);
    }
    Ok(instances)
}

pub fn canonicalize_beam_jsonl(raw: &str) -> Result<String> {
    let mut out = String::new();
    for instance in parse_beam_jsonl(raw)? {
        out.push_str(&serde_json::to_string(&instance)?);
        out.push('\n');
    }
    Ok(out)
}

pub fn benchmark_source_file(benchmark: &str, question_id: &str, session_id: &str) -> String {
    format!(
        "{}__{}__{}",
        sanitize_id(benchmark),
        sanitize_id(question_id),
        sanitize_id(session_id)
    )
}

pub fn evaluate_hits(
    ranked_session_ids: &[String],
    answer_session_ids: &[String],
) -> HitEvaluation {
    let answers: HashSet<&str> = answer_session_ids.iter().map(String::as_str).collect();
    HitEvaluation {
        hit_at_1: ranked_session_ids
            .iter()
            .take(1)
            .any(|session_id| answers.contains(session_id.as_str())),
        hit_at_5: ranked_session_ids
            .iter()
            .take(5)
            .any(|session_id| answers.contains(session_id.as_str())),
        hit_at_10: ranked_session_ids
            .iter()
            .take(10)
            .any(|session_id| answers.contains(session_id.as_str())),
    }
}

pub fn run_benchmark(options: &RunOptions) -> Result<BenchmarkSummary> {
    fs::create_dir_all(&options.output_dir)
        .with_context(|| format!("creating output dir {}", options.output_dir.display()))?;
    let raw = fs::read_to_string(&options.input_path)
        .with_context(|| format!("reading {}", options.input_path.display()))?;
    let skipped_questions = match options.benchmark.as_str() {
        "longmemeval" => count_longmemeval_abstentions(&raw)?,
        "beam" => 0,
        other => anyhow::bail!("unsupported benchmark: {other}"),
    };
    let mut instances = match options.benchmark.as_str() {
        "longmemeval" => parse_longmemeval(&raw)?,
        "beam" => parse_beam_jsonl(&raw)?,
        other => anyhow::bail!("unsupported benchmark: {other}"),
    };
    if let Some(limit) = options.limit {
        instances.truncate(limit);
    }

    let mut summary = BenchmarkSummary::new(&options.benchmark);
    summary.model = if options.bm25_only {
        "bm25-only".to_string()
    } else {
        options.model.id.to_string()
    };
    summary.skipped_questions = skipped_questions;
    let cases_path = options
        .output_dir
        .join(format!("{}_cases.jsonl", options.benchmark));
    let cases_file = File::create(&cases_path)
        .with_context(|| format!("creating cases file {}", cases_path.display()))?;
    let mut writer = BufWriter::new(cases_file);

    let total = instances.len();
    let progress_interval = progress_interval_for(total);
    let started = Instant::now();
    eprintln!(
        "[{}] starting benchmark: {} questions ({}, {})",
        options.benchmark,
        total,
        if options.bm25_only {
            "bm25-only".to_string()
        } else {
            format!("hybrid, {}", options.model.id)
        },
        match options.chunking {
            Chunking::Session => "one drawer per session",
            Chunking::Text => "text chunks, best chunk per session",
            Chunking::Exchange => "exchange chunks, best chunk per session",
        }
    );
    let mut cache = EmbeddingCache::default();
    for (index, instance) in instances.iter().enumerate() {
        let case = match run_case(instance, options, &mut cache) {
            Ok(case) => {
                summary.drawers_indexed += case.drawer_count;
                summary.ingest_ms += case.ingest_ms;
                summary.query_ms += case.query_ms;
                if case.scored {
                    let hits = HitEvaluation {
                        hit_at_1: case.hit_at_1.unwrap_or(false),
                        hit_at_5: case.hit_at_5.unwrap_or(false),
                        hit_at_10: case.hit_at_10.unwrap_or(false),
                    };
                    summary.record(&instance.question_type, &hits);
                } else {
                    summary.unscored_questions += 1;
                }
                case
            }
            Err(error) => {
                summary.failed_questions += 1;
                CaseOutput {
                    question_id: instance.question_id.clone(),
                    question_type: instance.question_type.clone(),
                    scored: false,
                    answer_session_ids: instance.answer_session_ids.clone(),
                    ranked_session_ids: Vec::new(),
                    hit_at_1: None,
                    hit_at_5: None,
                    hit_at_10: None,
                    ingest_ms: 0,
                    query_ms: 0,
                    drawer_count: 0,
                    error: Some(error.to_string()),
                }
            }
        };
        serde_json::to_writer(&mut writer, &case)?;
        writer.write_all(b"\n")?;

        let done = index + 1;
        if done == total || done % progress_interval == 0 {
            let elapsed_secs = started.elapsed().as_secs_f64().max(0.001);
            let rate = done as f64 / elapsed_secs;
            let eta_secs = if rate > 0.0 {
                ((total - done) as f64 / rate) as u64
            } else {
                0
            };
            eprintln!(
                "[{} {}] {}/{}  recall@5={:.3}  failed={}  unscored={}  {:.1}q/s  eta={}",
                options.benchmark,
                summary.model,
                done,
                total,
                summary.recall_at_5,
                summary.failed_questions,
                summary.unscored_questions,
                rate,
                format_duration(eta_secs),
            );
        }
    }
    writer.flush()?;

    let summary_path = options
        .output_dir
        .join(format!("{}_summary.json", options.benchmark));
    fs::write(&summary_path, serde_json::to_string_pretty(&summary)?)
        .with_context(|| format!("writing {}", summary_path.display()))?;

    Ok(summary)
}

pub fn prepare_beam_jsonl(input_path: &Path, output_path: &Path) -> Result<usize> {
    let raw = fs::read_to_string(input_path)
        .with_context(|| format!("reading {}", input_path.display()))?;
    let canonical = canonicalize_beam_jsonl(&raw)?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating output dir {}", parent.display()))?;
    }
    fs::write(output_path, canonical)
        .with_context(|| format!("writing {}", output_path.display()))?;
    Ok(parse_beam_jsonl(&fs::read_to_string(output_path)?)?.len())
}

fn run_case(
    instance: &BenchmarkInstance,
    options: &RunOptions,
    cache: &mut EmbeddingCache,
) -> Result<CaseOutput> {
    let benchmark = options.benchmark.as_str();
    let conn = crate::db::open_in_memory()?;
    let mut source_to_session = HashMap::new();

    let ingest_start = Instant::now();
    // (session index, chunk index, text): one entry per drawer.
    let drawers: Vec<(usize, usize, String)> = instance
        .sessions
        .iter()
        .enumerate()
        .flat_map(|(session_index, session)| {
            let chunks = match options.chunking {
                Chunking::Session => Vec::new(),
                Chunking::Text => crate::miner::chunk_text(&session.content),
                Chunking::Exchange => {
                    let transcript = match &session.raw {
                        Some(raw) => crate::normalize::normalize_content(&raw.to_string(), "json"),
                        None => session.content.clone(),
                    };
                    crate::convo_miner::chunk_exchanges(&transcript)
                }
            };
            if chunks.is_empty() {
                // Sessions too short to chunk stay whole.
                vec![(session_index, 0, session.content.clone())]
            } else {
                chunks
                    .into_iter()
                    .map(|(text, chunk_index)| (session_index, chunk_index, text))
                    .collect()
            }
        })
        .collect();
    let drawer_embeddings = if options.bm25_only {
        Vec::new()
    } else {
        let texts: Vec<&str> = drawers.iter().map(|(_, _, text)| text.as_str()).collect();
        cache.embed(options.model, &texts)?
    };
    for (index, (session_index, chunk_index, text)) in drawers.iter().enumerate() {
        let session = &instance.sessions[*session_index];
        let source_file =
            benchmark_source_file(benchmark, &instance.question_id, &session.session_id);
        source_to_session.insert(source_file.clone(), session.session_id.clone());
        let embedding = drawer_embeddings.get(index).map(Vec::as_slice);
        let metadata = serde_json::json!({
            "benchmark": benchmark,
            "question_id": instance.question_id,
            "question_type": instance.question_type,
            "question_date": instance.question_date,
            "conversation_id": instance.conversation_id,
            "session_id": session.session_id,
            "session_date": session.date,
            "turn_count": session.content.lines().count(),
            "chunk_index": chunk_index,
        });
        let drawer_id = if options.chunking == Chunking::Session {
            source_file.clone()
        } else {
            format!("{source_file}#{chunk_index}")
        };
        add_drawer_with_id(
            &conn,
            &drawer_id,
            benchmark,
            &instance.question_type,
            text,
            embedding,
            &source_file,
            "mempalace-bench",
            Some(&metadata),
        )?;
    }
    let ingest_ms = ingest_start.elapsed().as_millis();

    let query_embedding = if options.bm25_only {
        None
    } else {
        Some(crate::embedder::embed_query_with(
            options.model,
            &instance.question,
        )?)
    };
    let query_start = Instant::now();
    // Chunked runs rank every chunk so each session can surface through its
    // best one; results arrive best first, so the first chunk seen per
    // session is that session's score.
    let n_results = if options.chunking == Chunking::Session {
        TOP_K
    } else {
        drawers.len()
    };
    let results = hybrid_search_in(
        &conn,
        &instance.question,
        query_embedding.as_deref(),
        VectorSource::Hot,
        options.model,
        &DrawerFilter::default(),
        n_results,
    )?;
    let query_ms = query_start.elapsed().as_millis();

    let mut seen = HashSet::new();
    let ranked_session_ids: Vec<String> = results
        .into_iter()
        .filter_map(|result| source_to_session.get(&result.drawer.source_file).cloned())
        .filter(|session_id| seen.insert(session_id.clone()))
        .take(TOP_K)
        .collect();
    let scored = !instance.answer_session_ids.is_empty();
    let hits = scored.then(|| evaluate_hits(&ranked_session_ids, &instance.answer_session_ids));

    Ok(CaseOutput {
        question_id: instance.question_id.clone(),
        question_type: instance.question_type.clone(),
        scored,
        answer_session_ids: instance.answer_session_ids.clone(),
        ranked_session_ids,
        hit_at_1: hits.as_ref().map(|hit| hit.hit_at_1),
        hit_at_5: hits.as_ref().map(|hit| hit.hit_at_5),
        hit_at_10: hits.as_ref().map(|hit| hit.hit_at_10),
        ingest_ms,
        query_ms,
        drawer_count: drawers.len(),
        error: None,
    })
}

fn parse_beam_instance(value: &Value, line_index: usize) -> Result<BenchmarkInstance> {
    let question_id = string_field(value, "question_id")
        .or_else(|| string_field(value, "id"))
        .unwrap_or_else(|| format!("beam-{}", line_index + 1));
    let question_type = string_field(value, "question_type")
        .or_else(|| string_field(value, "ability"))
        .unwrap_or_else(|| "unknown".to_string());
    let question = string_field(value, "question")
        .or_else(|| string_field(value, "query"))
        .context("BEAM instance missing question")?;
    let conversation_id = string_field(value, "conversation_id");
    let answer_session_ids = string_array_field(value, "answer_session_ids");

    let sessions = if let Some(sessions) = value.get("sessions").and_then(Value::as_array) {
        sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                let session_id = string_field(session, "session_id")
                    .or_else(|| string_field(session, "id"))
                    .unwrap_or_else(|| format!("session-{index}"));
                let content = string_field(session, "content").unwrap_or_else(|| {
                    session
                        .get("turns")
                        .map(session_content)
                        .unwrap_or_else(|| session_content(session))
                });
                BenchmarkSession {
                    session_id,
                    date: string_field(session, "date"),
                    content,
                    raw: session.get("turns").cloned(),
                }
            })
            .collect()
    } else if let Some(chat) = value.get("chat") {
        vec![BenchmarkSession {
            session_id: "chat".to_string(),
            date: None,
            content: session_content(chat),
            raw: Some(chat.clone()),
        }]
    } else {
        anyhow::bail!("BEAM instance missing sessions/chat");
    };

    Ok(BenchmarkInstance {
        question_id,
        question_type,
        question,
        question_date: string_field(value, "question_date"),
        conversation_id,
        sessions,
        answer_session_ids,
    })
}

fn count_longmemeval_abstentions(raw: &str) -> Result<usize> {
    let values: Vec<Value> = serde_json::from_str(raw).context("parsing LongMemEval JSON")?;
    Ok(values
        .iter()
        .filter(|value| {
            string_field(value, "question_id")
                .as_deref()
                .is_some_and(|question_id| question_id.ends_with("_abs"))
        })
        .count())
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn string_array_field(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn session_content(value: &Value) -> String {
    match value {
        Value::Array(turns) => turns
            .iter()
            .map(turn_content)
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(_) => turn_content(value),
        Value::String(text) => text.clone(),
        _ => String::new(),
    }
}

fn turn_content(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    let role = string_field(value, "role").unwrap_or_else(|| "message".to_string());
    let content = string_field(value, "content")
        .or_else(|| string_field(value, "text"))
        .unwrap_or_default();
    if content.is_empty() {
        String::new()
    } else {
        format!("{role}: {content}")
    }
}

fn progress_interval_for(total: usize) -> usize {
    match total {
        0..=20 => 1,
        21..=200 => 10,
        201..=2_000 => 25,
        _ => 50,
    }
}

fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let secs = seconds % 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m{secs:02}s")
    } else if minutes > 0 {
        format!("{minutes}m{secs:02}s")
    } else {
        format!("{secs}s")
    }
}

fn sanitize_id(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_sep = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('_');
            last_was_sep = true;
        }
    }
    out.trim_matches('_').to_string()
}
