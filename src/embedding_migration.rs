//! Switching a palace to another embedding model without losing search.
//!
//! The hot columns (`drawers.embedding`, `drawers.pref_embedding`,
//! `closets.embedding`) always hold the *active* model's vectors, so search
//! never mixes models. A migration fills `embedding_vectors` with the
//! target model's vectors in resumable batches while search keeps using the
//! hot columns, [`verify`] compares both models on known-item probes, and
//! [`cutover`] swaps them in one transaction, archiving the old vectors so
//! [`rollback`] is instant. [`cleanup`] drops archived vectors.
//!
//! Every vector derives from stored text (`drawers.content`, the drawer's
//! preference span, `closets.topic`), so no history is lost by switching.
//! Drawers written during a migration get only an active-model vector; the
//! backfill picks them up on its next pass, and cutover embeds any
//! stragglers inside its transaction.

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::time::{Duration, Instant};

use crate::embedding_eval::{known_item_probe, recall, Recall, Rng};
use crate::embedding_models::{self, ModelSpec};
use crate::store::{DrawerFilter, VectorSource};

pub const META_ACTIVE: &str = "active_embedding_model";
pub const META_PREVIOUS: &str = "previous_embedding_model";
pub const META_TARGET: &str = "migration_target";
pub const META_STARTED: &str = "migration_started_at";
pub const META_VERIFIED: &str = "migration_verified";

/// Texts embedded and written per backfill transaction.
pub const BACKFILL_BATCH: usize = 32;
/// Known-item probes used by [`verify`].
pub const VERIFY_SAMPLE: usize = 200;
/// Seed for [`verify`] and [`estimate`] sampling, so reruns compare alike.
pub const VERIFY_SEED: u64 = 4040;
/// Largest recall@5 drop [`verify`] accepts.
pub const MAX_RECALL_DROP: f64 = 0.01;
/// Floor for the default latency budget, so sub-millisecond noise on a fast
/// machine cannot fail verification on its own.
const MIN_LATENCY_BUDGET: Duration = Duration::from_millis(25);
/// Probes [`estimate`] times to project the verify step.
const VERIFY_TIMING_PROBES: usize = 10;
/// Drawers embedded by [`estimate`] to measure throughput.
const ESTIMATE_SAMPLE: usize = 200;
/// Bytes per stored vector row beyond the floats: the key is stored in the
/// table and in its primary-key index, plus the model index.
const ROW_OVERHEAD_BYTES: u64 = 200;

/// Disk used by one stored vector of `dim` floats. Rows of 1.5 KB and more
/// fill SQLite pages to roughly three quarters.
fn stored_row_bytes(dim: usize) -> u64 {
    (dim * std::mem::size_of::<f32>()) as u64 * 4 / 3 + ROW_OVERHEAD_BYTES
}

/// What a stored vector belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerKind {
    /// A drawer's content vector (`drawers.embedding`).
    Drawer,
    /// A drawer's preference-span vector (`drawers.pref_embedding`).
    Pref,
    /// A closet's topic vector (`closets.embedding`).
    Closet,
}

impl OwnerKind {
    pub const ALL: [OwnerKind; 3] = [OwnerKind::Drawer, OwnerKind::Pref, OwnerKind::Closet];

    pub fn as_str(self) -> &'static str {
        match self {
            OwnerKind::Drawer => "drawer",
            OwnerKind::Pref => "pref",
            OwnerKind::Closet => "closet",
        }
    }

    /// Owners that need a vector for any model: every drawer, and the
    /// preference spans and closets that hold an active vector today.
    fn owners_sql(self) -> &'static str {
        match self {
            OwnerKind::Drawer => "SELECT id FROM drawers",
            OwnerKind::Pref => "SELECT id FROM drawers WHERE pref_embedding IS NOT NULL",
            OwnerKind::Closet => "SELECT id FROM closets WHERE embedding IS NOT NULL",
        }
    }

    /// Owners missing a vector for model `?1`, shortest text first, up to `?2`.
    fn missing_sql(self) -> &'static str {
        match self {
            OwnerKind::Drawer => {
                "SELECT d.id, d.content FROM drawers d
                 WHERE NOT EXISTS (SELECT 1 FROM embedding_vectors v
                     WHERE v.owner_kind = 'drawer' AND v.owner_id = d.id AND v.model = ?1)
                 ORDER BY length(d.content) LIMIT ?2"
            }
            OwnerKind::Pref => {
                "SELECT d.id, d.content FROM drawers d
                 WHERE d.pref_embedding IS NOT NULL AND NOT EXISTS (SELECT 1 FROM embedding_vectors v
                     WHERE v.owner_kind = 'pref' AND v.owner_id = d.id AND v.model = ?1)
                 ORDER BY length(d.content) LIMIT ?2"
            }
            OwnerKind::Closet => {
                "SELECT c.id, c.topic FROM closets c
                 WHERE c.embedding IS NOT NULL AND NOT EXISTS (SELECT 1 FROM embedding_vectors v
                     WHERE v.owner_kind = 'closet' AND v.owner_id = c.id AND v.model = ?1)
                 ORDER BY length(c.topic) LIMIT ?2"
            }
        }
    }

    /// Text that is embedded for an owner, given the column `missing_sql` returns.
    fn embed_text(self, text: String) -> String {
        match self {
            OwnerKind::Pref => crate::preference::preference_span(&text).unwrap_or(text),
            OwnerKind::Drawer | OwnerKind::Closet => text,
        }
    }
}

fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
        row.get(0)
    })
    .optional()
    .with_context(|| format!("reading meta.{key}"))
}

fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .with_context(|| format!("writing meta.{key}"))?;
    Ok(())
}

fn meta_delete(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM meta WHERE key = ?1", [key])
        .with_context(|| format!("clearing meta.{key}"))?;
    Ok(())
}

fn require_model(id: &str) -> Result<&'static ModelSpec> {
    embedding_models::find(id).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown embedding model '{id}' (known: {})",
            embedding_models::ids()
        )
    })
}

/// Record the active model for a palace that has none yet. A palace with no
/// drawers starts on `wanted` (the configured model); one that already has
/// vectors made before models were recorded is marked as the default, which
/// is the only model older releases used. Stored vectors are not touched.
pub fn ensure_active_model(conn: &Connection, wanted: Option<&str>) -> Result<()> {
    if meta_get(conn, META_ACTIVE)?.is_some() {
        return Ok(());
    }
    let has_drawers: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM drawers)", [], |row| row.get(0))
        .context("checking for drawers")?;
    let id = match wanted.and_then(embedding_models::find) {
        Some(spec) if !has_drawers => spec.id,
        _ => embedding_models::DEFAULT_MODEL_ID,
    };
    meta_set(conn, META_ACTIVE, id)
}

/// The palace's active model id as recorded in `meta`.
pub fn active_model_id(conn: &Connection) -> Result<String> {
    Ok(meta_get(conn, META_ACTIVE)?
        .unwrap_or_else(|| embedding_models::DEFAULT_MODEL_ID.to_string()))
}

/// Point the process embedder at the palace's active model. Long-running
/// processes call this per request so a cutover made by another process is
/// picked up. Unknown ids (a palace written by a newer release) keep the
/// current model and are reported.
pub fn sync_active_model(conn: &Connection) -> Result<&'static ModelSpec> {
    let id = active_model_id(conn)?;
    if crate::embedder::active_model().id == id {
        return Ok(crate::embedder::active_model());
    }
    crate::embedder::set_active_model(&id)
}

/// Warn when the hot vectors do not have the active model's length. Search
/// skips mismatched vectors, so the palace degrades to BM25 instead of
/// comparing vectors from different models.
pub fn check_vector_dimensions(conn: &Connection) -> Result<()> {
    let spec = crate::embedder::active_model();
    let stored: Option<i64> = conn
        .query_row(
            "SELECT length(embedding) FROM drawers WHERE embedding IS NOT NULL LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("sampling stored vector size")?;
    if let Some(bytes) = stored {
        let expected = (spec.dim * std::mem::size_of::<f32>()) as i64;
        if bytes != expected {
            tracing::warn!(
                model = spec.id,
                stored_bytes = bytes,
                expected_bytes = expected,
                "Palace: stored vectors do not match the active embedding model; vector search \
                 is disabled until `palace embeddings` repairs it (keyword search still works)"
            );
        }
    }
    Ok(())
}

/// How many owners of each kind have a vector for one model.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Coverage {
    pub drawers_done: u64,
    pub drawers_total: u64,
    pub prefs_done: u64,
    pub prefs_total: u64,
    pub closets_done: u64,
    pub closets_total: u64,
}

impl Coverage {
    pub fn done(&self) -> u64 {
        self.drawers_done + self.prefs_done + self.closets_done
    }

    pub fn total(&self) -> u64 {
        self.drawers_total + self.prefs_total + self.closets_total
    }

    pub fn complete(&self) -> bool {
        self.done() >= self.total()
    }

    pub fn percent(&self) -> f64 {
        if self.total() == 0 {
            100.0
        } else {
            self.done() as f64 * 100.0 / self.total() as f64
        }
    }
}

/// Vectors stored in `embedding_vectors` for `model`, against what it needs.
pub fn coverage(conn: &Connection, model: &str) -> Result<Coverage> {
    let mut counts = [(0u64, 0u64); 3];
    for (slot, kind) in counts.iter_mut().zip(OwnerKind::ALL) {
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM ({})", kind.owners_sql()),
            [],
            |row| row.get(0),
        )?;
        let done: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM embedding_vectors
                 WHERE owner_kind = ?1 AND model = ?2 AND owner_id IN ({})",
                kind.owners_sql()
            ),
            params![kind.as_str(), model],
            |row| row.get(0),
        )?;
        *slot = (done as u64, total as u64);
    }
    Ok(Coverage {
        drawers_done: counts[0].0,
        drawers_total: counts[0].1,
        prefs_done: counts[1].0,
        prefs_total: counts[1].1,
        closets_done: counts[2].0,
        closets_total: counts[2].1,
    })
}

/// Embed every owner that has no `spec` vector yet, one transaction per
/// batch. Safe to interrupt and rerun: it only ever fills gaps. `progress`
/// sees the number of vectors written so far.
pub fn backfill(
    conn: &Connection,
    spec: &'static ModelSpec,
    progress: &mut dyn FnMut(u64),
) -> Result<u64> {
    let mut written = 0u64;
    for kind in OwnerKind::ALL {
        loop {
            let rows: Vec<(String, String)> = {
                let mut stmt = conn.prepare_cached(kind.missing_sql())?;
                let rows = stmt
                    .query_map(params![spec.id, BACKFILL_BATCH as i64], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            if rows.is_empty() {
                break;
            }
            written += write_vectors(conn, spec, kind, rows)?;
            progress(written);
        }
    }
    Ok(written)
}

/// Embed `rows` (`(owner_id, text)`) with `spec` and store them in one transaction.
fn write_vectors(
    conn: &Connection,
    spec: &'static ModelSpec,
    kind: OwnerKind,
    rows: Vec<(String, String)>,
) -> Result<u64> {
    let (ids, texts): (Vec<String>, Vec<String>) = rows
        .into_iter()
        .map(|(id, text)| (id, kind.embed_text(text)))
        .unzip();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let vectors = crate::embedder::embed_documents_with(spec, &refs)
        .with_context(|| format!("embedding {} vectors with {}", kind.as_str(), spec.id))?;
    crate::store::with_write_batch(conn, |conn| {
        let mut stmt = conn.prepare_cached(
            "INSERT OR REPLACE INTO embedding_vectors (owner_kind, owner_id, model, vec)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (id, vector) in ids.iter().zip(&vectors) {
            stmt.execute(params![
                kind.as_str(),
                id,
                spec.id,
                crate::embedder::vec_to_blob(vector)
            ])?;
        }
        Ok(())
    })?;
    Ok(ids.len() as u64)
}

/// Begin migrating to `target`: records it in `meta`. The backfill itself
/// is [`backfill`]; nothing changes for search until [`cutover`].
pub fn start(conn: &Connection, target: &str) -> Result<&'static ModelSpec> {
    let spec = require_model(target)?;
    let active = active_model_id(conn)?;
    if spec.id == active {
        bail!("{} is already the active embedding model", spec.id);
    }
    if let Some(current) = meta_get(conn, META_TARGET)? {
        if current != spec.id {
            bail!("a migration to {current} is in progress; run `palace embeddings cancel` first");
        }
        return Ok(spec);
    }
    meta_set(conn, META_TARGET, spec.id)?;
    meta_set(conn, META_STARTED, &chrono::Utc::now().to_rfc3339())?;
    meta_delete(conn, META_VERIFIED)?;
    Ok(spec)
}

/// The model being migrated to, if any.
pub fn target(conn: &Connection) -> Result<Option<&'static ModelSpec>> {
    meta_get(conn, META_TARGET)?
        .map(|id| require_model(&id))
        .transpose()
}

/// Abandon the migration in progress. Its vectors stay until [`cleanup`],
/// so restarting the same migration resumes where it stopped.
pub fn cancel(conn: &Connection) -> Result<Option<String>> {
    let target = meta_get(conn, META_TARGET)?;
    for key in [META_TARGET, META_STARTED, META_VERIFIED] {
        meta_delete(conn, key)?;
    }
    Ok(target)
}

/// Snapshot of a palace's embedding state.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub active: String,
    /// Model chosen in config (`embedding_model` / `PALACE_EMBEDDING_MODEL`).
    pub wanted: Option<String>,
    /// The configured model differs from the active one.
    pub migration_pending: bool,
    pub target: Option<String>,
    pub started_at: Option<String>,
    pub coverage: Option<Coverage>,
    /// Verification result for the target: `"passed"` or `"failed"`.
    pub verified: Option<String>,
    /// Model whose vectors cutover archived; [`rollback`] returns to it.
    pub previous: Option<String>,
    /// Vector rows per model held in `embedding_vectors`.
    pub stored: Vec<(String, u64)>,
}

pub fn status(conn: &Connection, wanted: Option<&str>) -> Result<Status> {
    let active = active_model_id(conn)?;
    let wanted = wanted
        .and_then(embedding_models::find)
        .map(|spec| spec.id.to_string());
    let target = meta_get(conn, META_TARGET)?;
    let coverage = target
        .as_deref()
        .map(|model| coverage(conn, model))
        .transpose()?;
    let verified = meta_get(conn, META_VERIFIED)?.and_then(|value| {
        let (model, outcome) = value.split_once(':')?;
        (Some(model) == target.as_deref()).then(|| outcome.to_string())
    });
    let stored = {
        let mut stmt = conn.prepare(
            "SELECT model, COUNT(*) FROM embedding_vectors GROUP BY model ORDER BY model",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    Ok(Status {
        migration_pending: wanted.as_deref().is_some_and(|w| w != active),
        active,
        wanted,
        target,
        started_at: meta_get(conn, META_STARTED)?,
        coverage,
        verified,
        previous: meta_get(conn, META_PREVIOUS)?,
        stored,
    })
}

/// Query latency percentiles in milliseconds.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct Latency {
    pub p50_ms: f64,
    pub p95_ms: f64,
}

fn latency(mut samples: Vec<Duration>) -> Latency {
    if samples.is_empty() {
        return Latency::default();
    }
    samples.sort();
    let pick = |p: f64| {
        samples[((samples.len() as f64 - 1.0) * p).round() as usize].as_secs_f64() * 1000.0
    };
    Latency {
        p50_ms: pick(0.5),
        p95_ms: pick(0.95),
    }
}

/// Short queries for latency measurements.
fn latency_queries() -> Vec<String> {
    (0..50)
        .map(|i| format!("why did we choose option {i} for the release?"))
        .collect()
}

fn time_queries(spec: &'static ModelSpec, queries: &[String]) -> Result<Latency> {
    crate::embedder::embed_query_with(spec, "warm up")?;
    let mut samples = Vec::with_capacity(queries.len());
    for query in queries {
        let start = Instant::now();
        crate::embedder::embed_query_with(spec, query)?;
        samples.push(start.elapsed());
    }
    Ok(latency(samples))
}

/// Projected cost of migrating a palace to one model, measured on this machine.
#[derive(Debug, Clone, Serialize)]
pub struct Estimate {
    pub model: String,
    pub drawers: u64,
    pub prefs: u64,
    pub closets: u64,
    pub text_bytes: u64,
    pub download_bytes: u64,
    /// The model was already downloaded before this estimate.
    pub was_cached: bool,
    pub sampled_drawers: usize,
    pub drawers_per_sec: f64,
    /// Projected time to build the missing vectors.
    pub estimated_seconds: f64,
    /// Projected time for the verify step that `migrate` runs afterwards.
    pub verify_seconds: f64,
    pub extra_disk_bytes: u64,
    pub target_query: Latency,
    pub active_query: Latency,
}

/// Measure what migrating to `spec` would cost: downloads and loads the
/// model, embeds a random sample of drawers, times short queries on both
/// models, then extrapolates by text volume.
pub fn estimate(conn: &Connection, spec: &'static ModelSpec) -> Result<Estimate> {
    let coverage = coverage(conn, spec.id)?;
    let text_bytes: i64 = conn.query_row(
        "SELECT COALESCE(SUM(length(content)), 0) FROM drawers",
        [],
        |row| row.get(0),
    )?;
    let was_cached = embedding_models::is_cached(spec);
    crate::embedder::load_model(spec)?;

    let ids: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM drawers ORDER BY rowid")?;
        let rows = stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let picked = Rng::new(VERIFY_SEED).sample_indices(ids.len(), ESTIMATE_SAMPLE);
    let mut sample = Vec::with_capacity(picked.len());
    for index in picked {
        let content: String = conn.query_row(
            "SELECT content FROM drawers WHERE id = ?1",
            [&ids[index]],
            |row| row.get(0),
        )?;
        sample.push(content);
    }
    let sample_bytes: usize = sample.iter().map(String::len).sum();
    let refs: Vec<&str> = sample.iter().map(String::as_str).collect();
    let start = Instant::now();
    crate::embedder::embed_documents_with(spec, &refs)?;
    let elapsed = start.elapsed().as_secs_f64().max(1e-6);
    let drawers_per_sec = sample.len() as f64 / elapsed;
    let bytes_per_sec = sample_bytes as f64 / elapsed;

    let remaining = coverage.total() - coverage.done();
    let remaining_fraction = if coverage.total() == 0 {
        0.0
    } else {
        remaining as f64 / coverage.total() as f64
    };
    let estimated_seconds = if bytes_per_sec > 0.0 {
        // Preference spans and closet topics are short; count them as one
        // average drawer each so the projection stays conservative.
        let avg_drawer = text_bytes as f64 / coverage.drawers_total.max(1) as f64;
        let extra = (coverage.prefs_total + coverage.closets_total) as f64 * avg_drawer;
        (text_bytes as f64 + extra) / bytes_per_sec * remaining_fraction
    } else {
        0.0
    };

    // Verify runs a hybrid search per probe on each model; time a few now.
    let mut rng = Rng::new(VERIFY_SEED);
    let probes: Vec<String> = sample
        .iter()
        .filter_map(|content| known_item_probe(content, &mut rng))
        .take(VERIFY_TIMING_PROBES)
        .collect();
    let start = Instant::now();
    for probe in &probes {
        let qvec = crate::embedder::embed_query(probe)?;
        crate::ranker::hybrid_search(conn, probe, Some(&qvec), &DrawerFilter::default(), 5)?;
    }
    let per_probe = start.elapsed().as_secs_f64() / probes.len().max(1) as f64;
    let verify_seconds = per_probe * (VERIFY_SAMPLE * 2) as f64;

    let queries = latency_queries();
    let active = crate::embedder::active_model();
    Ok(Estimate {
        model: spec.id.to_string(),
        drawers: coverage.drawers_total,
        prefs: coverage.prefs_total,
        closets: coverage.closets_total,
        text_bytes: text_bytes as u64,
        download_bytes: spec.download_bytes,
        was_cached,
        sampled_drawers: sample.len(),
        drawers_per_sec,
        estimated_seconds,
        verify_seconds,
        extra_disk_bytes: coverage.total() * stored_row_bytes(spec.dim),
        target_query: time_queries(spec, &queries)?,
        active_query: time_queries(active, &queries)?,
    })
}

/// One model's results in [`verify`].
#[derive(Debug, Clone, Serialize)]
pub struct ModelScore {
    pub model: String,
    pub hybrid_recall_at_1: f64,
    pub hybrid_recall_at_5: f64,
    pub vector_recall_at_1: f64,
    pub vector_recall_at_5: f64,
    pub query: Latency,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    pub probes: usize,
    pub active: ModelScore,
    pub target: ModelScore,
    pub latency_budget_ms: f64,
    pub passed: bool,
    pub reasons: Vec<String>,
}

/// Compare the active and target models on known-item probes drawn from
/// this palace: full hybrid search and vector-only search, recall@1/@5 and
/// query latency. Passes when target recall@5 is within
/// [`MAX_RECALL_DROP`] of the active model and target p95 query latency
/// is within `latency_budget_ms` (default: twice the active p95, at least
/// 25 ms). The result is recorded; [`cutover`] requires a pass.
pub fn verify(
    conn: &Connection,
    sample: usize,
    latency_budget_ms: Option<f64>,
) -> Result<VerifyReport> {
    let target = target(conn)?.context("no migration in progress")?;
    let progress = coverage(conn, target.id)?;
    if progress.drawers_done < progress.drawers_total {
        bail!(
            "{} has vectors for {}/{} drawers; finish `palace embeddings migrate {}` first",
            target.id,
            progress.drawers_done,
            progress.drawers_total,
            target.id
        );
    }
    let active = crate::embedder::active_model();

    let drawers: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT id, content FROM drawers ORDER BY rowid")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    // Visit drawers in a seeded random order until enough probes are made.
    let mut rng = Rng::new(VERIFY_SEED);
    let mut positions: Vec<usize> = (0..drawers.len()).collect();
    for i in (1..positions.len()).rev() {
        positions.swap(i, rng.below(i + 1));
    }
    let mut probes: Vec<(&str, String)> = Vec::new();
    for index in positions {
        if probes.len() >= sample {
            break;
        }
        let (id, content) = &drawers[index];
        if let Some(probe) = known_item_probe(content, &mut rng) {
            probes.push((id.as_str(), probe));
        }
    }
    if probes.is_empty() {
        bail!("no drawer has a sentence long enough to build a probe");
    }

    let score = |spec: &'static ModelSpec, source: VectorSource| -> Result<ModelScore> {
        crate::embedder::embed_query_with(spec, "warm up")?;
        let mut hybrid = Vec::with_capacity(probes.len());
        let mut vector_only = Vec::with_capacity(probes.len());
        let mut times = Vec::with_capacity(probes.len());
        for (gold, probe) in &probes {
            let start = Instant::now();
            let qvec = crate::embedder::embed_query_with(spec, probe)?;
            times.push(start.elapsed());
            let ranked = crate::ranker::hybrid_search_in(
                conn,
                probe,
                Some(&qvec),
                source,
                spec,
                &DrawerFilter::default(),
                5,
            )?;
            hybrid.push((*gold, ranked.into_iter().map(|r| r.drawer.id).collect()));
            let ranked =
                crate::store::vector_search_in(conn, &qvec, source, &DrawerFilter::default(), 5)?;
            vector_only.push((*gold, ranked.into_iter().map(|r| r.id).collect()));
        }
        let hybrid: Recall = recall(hybrid);
        let vector_only: Recall = recall(vector_only);
        Ok(ModelScore {
            model: spec.id.to_string(),
            hybrid_recall_at_1: hybrid.at_1,
            hybrid_recall_at_5: hybrid.at_5,
            vector_recall_at_1: vector_only.at_1,
            vector_recall_at_5: vector_only.at_5,
            query: latency(times),
        })
    };
    // Each probe searches every vector; read them once per model.
    let all = DrawerFilter::default();
    let active_vectors = crate::store::load_vectors(conn, VectorSource::Hot, &all)?;
    let target_vectors = crate::store::load_vectors(conn, VectorSource::Stored(target.id), &all)?;
    let active_score = score(active, VectorSource::Loaded(&active_vectors))?;
    let target_score = score(target, VectorSource::Loaded(&target_vectors))?;

    let budget = latency_budget_ms.unwrap_or_else(|| {
        (active_score.query.p95_ms * 2.0).max(MIN_LATENCY_BUDGET.as_secs_f64() * 1000.0)
    });
    let mut reasons = Vec::new();
    if target_score.hybrid_recall_at_5 < active_score.hybrid_recall_at_5 - MAX_RECALL_DROP {
        reasons.push(format!(
            "recall@5 {:.3} is below the active model's {:.3}",
            target_score.hybrid_recall_at_5, active_score.hybrid_recall_at_5
        ));
    }
    if target_score.query.p95_ms > budget {
        reasons.push(format!(
            "query p95 {:.1} ms is over the {budget:.1} ms budget",
            target_score.query.p95_ms
        ));
    }
    let passed = reasons.is_empty();
    meta_set(
        conn,
        META_VERIFIED,
        &format!("{}:{}", target.id, if passed { "passed" } else { "failed" }),
    )?;
    Ok(VerifyReport {
        probes: probes.len(),
        active: active_score,
        target: target_score,
        latency_budget_ms: budget,
        passed,
        reasons,
    })
}

/// Outcome of [`run`]: backfill, verify, and cutover when verify passed.
#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub written: u64,
    pub verify: VerifyReport,
    pub switched: Option<SwitchReport>,
}

/// Finish a started migration: backfill, verify, and cut over if verify
/// passes (or `force`). With `cutover_when_passed: false` it stops after
/// verify.
pub fn run(
    conn: &Connection,
    latency_budget_ms: Option<f64>,
    cutover_when_passed: bool,
    force: bool,
    progress: &mut dyn FnMut(u64),
) -> Result<RunReport> {
    let spec = target(conn)?.context("no migration in progress")?;
    let written = backfill(conn, spec, progress)?;
    let verify = verify(conn, VERIFY_SAMPLE, latency_budget_ms)?;
    let switched = if cutover_when_passed && (verify.passed || force) {
        Some(cutover(conn, true)?)
    } else {
        None
    };
    Ok(RunReport {
        written,
        verify,
        switched,
    })
}

/// Commands that change a palace's embeddings run where the palace lives.
/// In remote mode the server owns the model, so local commands only report.
pub fn ensure_local(config: &crate::config::PalaceConfig) -> Result<()> {
    if config.mcp_mode() == "remote" {
        bail!(
            "remote mode: the palace server owns its embedding model; run `palace embeddings` \
             on the server"
        );
    }
    Ok(())
}

/// Result of switching the active model.
#[derive(Debug, Clone, Serialize)]
pub struct SwitchReport {
    pub from: String,
    pub to: String,
    /// Vectors embedded to close gaps before switching.
    pub caught_up: u64,
}

/// Make the migration target active. Requires a passing [`verify`] unless
/// `force`.
pub fn cutover(conn: &Connection, force: bool) -> Result<SwitchReport> {
    let target = target(conn)?.context("no migration in progress")?;
    let verified = meta_get(conn, META_VERIFIED)?;
    let passed = verified.as_deref() == Some(&format!("{}:passed", target.id));
    if !passed && !force {
        bail!(
            "{} has not passed `palace embeddings verify`; pass --force to switch anyway",
            target.id
        );
    }
    switch_to(conn, target)
}

/// Return to the model the last cutover replaced.
pub fn rollback(conn: &Connection) -> Result<SwitchReport> {
    let previous = meta_get(conn, META_PREVIOUS)?
        .context("no previous model to roll back to (cleanup may have removed it)")?;
    let spec = require_model(&previous)?;
    switch_to(conn, spec)
}

/// Swap `spec` into the hot columns in one transaction, archiving the
/// current vectors under the current model id.
fn switch_to(conn: &Connection, spec: &'static ModelSpec) -> Result<SwitchReport> {
    let from = active_model_id(conn)?;
    if from == spec.id {
        bail!("{} is already the active embedding model", spec.id);
    }
    // Fill gaps outside the write lock first; writes that land in between
    // are caught inside the transaction below.
    let mut caught_up = backfill(conn, spec, &mut |_| {})?;

    let tx = conn
        .unchecked_transaction()
        .context("starting model switch")?;
    caught_up += backfill(&tx, spec, &mut |_| {})?;
    if !coverage(&tx, spec.id)?.complete() {
        bail!("{} is missing vectors; rerun migrate", spec.id);
    }
    tx.execute(
        "INSERT OR REPLACE INTO embedding_vectors (owner_kind, owner_id, model, vec)
         SELECT 'drawer', id, ?1, embedding FROM drawers WHERE embedding IS NOT NULL",
        [&from],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO embedding_vectors (owner_kind, owner_id, model, vec)
         SELECT 'pref', id, ?1, pref_embedding FROM drawers WHERE pref_embedding IS NOT NULL",
        [&from],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO embedding_vectors (owner_kind, owner_id, model, vec)
         SELECT 'closet', id, ?1, embedding FROM closets WHERE embedding IS NOT NULL",
        [&from],
    )?;
    tx.execute(
        "UPDATE drawers SET embedding = (SELECT v.vec FROM embedding_vectors v
             WHERE v.owner_kind = 'drawer' AND v.owner_id = drawers.id AND v.model = ?1)",
        [spec.id],
    )?;
    tx.execute(
        "UPDATE drawers SET pref_embedding = (SELECT v.vec FROM embedding_vectors v
             WHERE v.owner_kind = 'pref' AND v.owner_id = drawers.id AND v.model = ?1)
         WHERE pref_embedding IS NOT NULL",
        [spec.id],
    )?;
    tx.execute(
        "UPDATE closets SET embedding = (SELECT v.vec FROM embedding_vectors v
             WHERE v.owner_kind = 'closet' AND v.owner_id = closets.id AND v.model = ?1)
         WHERE embedding IS NOT NULL",
        [spec.id],
    )?;
    tx.execute("DELETE FROM embedding_vectors WHERE model = ?1", [spec.id])?;
    meta_set(&tx, META_ACTIVE, spec.id)?;
    meta_set(&tx, META_PREVIOUS, &from)?;
    for key in [META_TARGET, META_STARTED, META_VERIFIED] {
        meta_delete(&tx, key)?;
    }
    tx.commit().context("committing model switch")?;
    crate::embedder::set_active_model(spec.id)?;
    Ok(SwitchReport {
        from,
        to: spec.id.to_string(),
        caught_up,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanupReport {
    pub rows_deleted: u64,
    pub bytes_freed: u64,
}

/// Drop stored vectors of every model except the migration target (if a
/// migration is in progress), forget the rollback model, and `VACUUM`.
pub fn cleanup(conn: &Connection) -> Result<CleanupReport> {
    let size = |conn: &Connection| -> Result<u64> {
        let pages: i64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        Ok((pages * page_size) as u64)
    };
    let before = size(conn)?;
    let keep = meta_get(conn, META_TARGET)?.unwrap_or_default();
    let rows_deleted = conn.execute("DELETE FROM embedding_vectors WHERE model != ?1", [&keep])?;
    meta_delete(conn, META_PREVIOUS)?;
    conn.execute_batch("VACUUM").context("vacuuming palace")?;
    let after = size(conn)?;
    Ok(CleanupReport {
        rows_deleted: rows_deleted as u64,
        bytes_freed: before.saturating_sub(after),
    })
}
