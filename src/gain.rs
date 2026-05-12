//! Aggregation and rendering for `palace gain`.

use anyhow::{bail, Context, Result};
use chrono::{Duration, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub enum SinceWindow {
    Days(i64),
    Hours(i64),
    All,
}

#[derive(Debug, Clone)]
pub struct GainOptions {
    pub project: Option<String>,
    pub since: SinceWindow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GainReport {
    pub window: String,
    pub project: Option<String>,
    pub tool_calls: i64,
    pub sessions: i64,
    pub search_calls: i64,
    pub search_hits: i64,
    pub hit_rate: f64,
    pub errors: i64,
    pub tokens_saved_est: i64,
    pub duplicate_skips: i64,
    pub kg_facts_recalled: i64,
    pub diary_recalls: i64,
    pub repeat_questions_avoided: i64,
    pub p50_latency_ms: i64,
    pub p95_latency_ms: i64,
    pub top_wings: Vec<NamedCount>,
    pub top_rooms: Vec<NamedCount>,
    pub top_reused_queries: Vec<NamedCount>,
    pub per_project: Vec<ProjectGain>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectGain {
    pub project: String,
    pub tool_calls: i64,
    pub search_hits: i64,
    pub tokens_saved_est: i64,
    pub duplicate_skips: i64,
    pub kg_facts_recalled: i64,
    pub diary_recalls: i64,
    pub repeat_questions_avoided: i64,
    pub value_score: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedCount {
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GainHistoryEvent {
    pub ts: String,
    pub project: String,
    pub tool: String,
    pub outcome: String,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub result_count: i64,
    pub tokens_saved_est: i64,
    pub duration_ms: i64,
}

#[derive(Debug, Clone)]
struct UsageRow {
    session_id: String,
    project: String,
    tool: String,
    wing: Option<String>,
    room: Option<String>,
    query_hash: Option<String>,
    result_count: i64,
    est_tokens_saved: i64,
    duration_ms: i64,
    outcome: String,
    meta: Value,
}

impl SinceWindow {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("all") {
            return Ok(Self::All);
        }
        if let Some(days) = trimmed.strip_suffix('d') {
            let days = days
                .parse::<i64>()
                .with_context(|| format!("invalid --since value: {input}"))?;
            if days <= 0 {
                bail!("--since days must be greater than zero");
            }
            return Ok(Self::Days(days));
        }
        if let Some(hours) = trimmed.strip_suffix('h') {
            let hours = hours
                .parse::<i64>()
                .with_context(|| format!("invalid --since value: {input}"))?;
            if hours <= 0 {
                bail!("--since hours must be greater than zero");
            }
            return Ok(Self::Hours(hours));
        }
        bail!("--since must be like 7d, 24h, or all")
    }

    fn cutoff(&self) -> Option<String> {
        match self {
            Self::Days(days) => Some((Utc::now() - Duration::days(*days)).to_rfc3339()),
            Self::Hours(hours) => Some((Utc::now() - Duration::hours(*hours)).to_rfc3339()),
            Self::All => None,
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Days(days) => format!("last {days}d"),
            Self::Hours(hours) => format!("last {hours}h"),
            Self::All => "all time".to_string(),
        }
    }
}

pub fn summarize(conn: &Connection, options: &GainOptions) -> Result<GainReport> {
    let events = read_events(conn, options)?;
    Ok(build_report(events, options))
}

pub fn history(
    conn: &Connection,
    options: &GainOptions,
    limit: usize,
) -> Result<Vec<GainHistoryEvent>> {
    let cutoff = options.since.cutoff();
    let project = options.project.as_deref();
    let mut stmt = conn
        .prepare(
            "SELECT ts, project, tool, outcome, wing, room, result_count,
                    est_tokens_saved, duration_ms
             FROM usage_events
             WHERE (?1 IS NULL OR project = ?1)
               AND (?2 IS NULL OR datetime(ts) >= datetime(?2))
             ORDER BY datetime(ts) DESC
             LIMIT ?3",
        )
        .context("preparing gain history query")?;
    let rows = stmt.query_map(params![project, cutoff.as_deref(), limit as i64], |row| {
        Ok(GainHistoryEvent {
            ts: row.get(0)?,
            project: row.get(1)?,
            tool: row.get(2)?,
            outcome: row.get(3)?,
            wing: row.get(4)?,
            room: row.get(5)?,
            result_count: row.get(6)?,
            tokens_saved_est: row.get(7)?,
            duration_ms: row.get(8)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("reading gain history")
}

pub fn reset(conn: &Connection, project: Option<&str>) -> Result<usize> {
    let rows = if let Some(project) = project {
        conn.execute(
            "DELETE FROM usage_events WHERE project = ?1",
            params![project],
        )?
    } else {
        conn.execute("DELETE FROM usage_events", [])?
    };
    Ok(rows)
}

pub fn render_text(report: &GainReport) -> String {
    let project = report.project.as_deref().unwrap_or("all projects");
    let top_wings = render_named_counts(&report.top_wings);
    let top_projects = report
        .per_project
        .iter()
        .take(5)
        .map(|project| {
            format!(
                "{}({} calls, ~{} tok)",
                project.project,
                project.tool_calls,
                format_number(project.tokens_saved_est)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "MemPalace gain - {} ({})\n  Tool calls         : {}   (sessions: {})\n  Hit rate           : {:.0}%   (search hits {}/{})\n  Tokens saved (est) : ~{}\n  Re-index skipped   : {}    (duplicate drawers avoided)\n  KG facts recalled  : {}\n  Diary recalls      : {}\n  Repeat Qs avoided  : {}\n  p95 latency        : {} ms\n  Top wings          : {}\n  Top projects       : {}\n",
        report.window,
        project,
        format_number(report.tool_calls),
        report.sessions,
        report.hit_rate * 100.0,
        report.search_hits,
        report.search_calls,
        format_number(report.tokens_saved_est),
        report.duplicate_skips,
        report.kg_facts_recalled,
        report.diary_recalls,
        report.repeat_questions_avoided,
        report.p95_latency_ms,
        if top_wings.is_empty() {
            "none".to_string()
        } else {
            top_wings
        },
        if top_projects.is_empty() {
            "none".to_string()
        } else {
            top_projects
        }
    )
}

pub fn render_history(events: &[GainHistoryEvent]) -> String {
    if events.is_empty() {
        return "No MemPalace gain history yet.\n".to_string();
    }
    let mut lines = vec!["MemPalace gain history".to_string()];
    for event in events {
        let scope = match (&event.wing, &event.room) {
            (Some(wing), Some(room)) => format!(" {wing}/{room}"),
            (Some(wing), None) => format!(" {wing}"),
            _ => String::new(),
        };
        lines.push(format!(
            "  {}  {}  {}{}  results={}  ~{} tok  {} ms",
            event.ts,
            event.project,
            event.outcome,
            scope,
            event.result_count,
            format_number(event.tokens_saved_est),
            event.duration_ms
        ));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn read_events(conn: &Connection, options: &GainOptions) -> Result<Vec<UsageRow>> {
    let cutoff = options.since.cutoff();
    let project = options.project.as_deref();
    let mut stmt = conn
        .prepare(
            "SELECT session_id, project, tool, wing, room, query_hash, result_count,
                    est_tokens_saved, duration_ms, outcome, meta
             FROM usage_events
             WHERE (?1 IS NULL OR project = ?1)
               AND (?2 IS NULL OR datetime(ts) >= datetime(?2))
             ORDER BY datetime(ts) ASC",
        )
        .context("preparing gain summary query")?;
    let rows = stmt.query_map(params![project, cutoff.as_deref()], |row| {
        let meta_text: String = row.get(10)?;
        let meta =
            serde_json::from_str(&meta_text).unwrap_or_else(|_| Value::Object(Default::default()));
        Ok(UsageRow {
            session_id: row.get(0)?,
            project: row.get(1)?,
            tool: row.get(2)?,
            wing: row.get(3)?,
            room: row.get(4)?,
            query_hash: row.get(5)?,
            result_count: row.get(6)?,
            est_tokens_saved: row.get(7)?,
            duration_ms: row.get(8)?,
            outcome: row.get(9)?,
            meta,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("reading usage events")
}

fn build_report(events: Vec<UsageRow>, options: &GainOptions) -> GainReport {
    let mut sessions = HashSet::new();
    let mut latencies = Vec::new();
    let mut top_wings: HashMap<String, i64> = HashMap::new();
    let mut top_rooms: HashMap<String, i64> = HashMap::new();
    let mut query_counts: HashMap<String, i64> = HashMap::new();
    let mut seen_queries: HashSet<String> = HashSet::new();
    let mut seen_project_queries: HashMap<String, HashSet<String>> = HashMap::new();
    let mut project_acc: HashMap<String, ProjectAccumulator> = HashMap::new();

    let mut search_calls = 0;
    let mut search_hits = 0;
    let mut errors = 0;
    let mut tokens_saved_est = 0;
    let mut duplicate_skips = 0;
    let mut kg_facts_recalled = 0;
    let mut diary_recalls = 0;
    let mut repeat_questions_avoided = 0;

    for event in &events {
        sessions.insert(event.session_id.clone());
        latencies.push(event.duration_ms);
        tokens_saved_est += event.est_tokens_saved;

        if event.tool == "palace_search" {
            search_calls += 1;
        }
        if event.outcome == "hit" {
            search_hits += 1;
        }
        if event.outcome == "error" {
            errors += 1;
        }
        if event.outcome == "duplicate_skip" {
            duplicate_skips += 1;
        }
        if event.outcome == "kg_fact" {
            kg_facts_recalled += event.result_count;
        }
        if event.outcome == "diary_recall" {
            diary_recalls += event.result_count;
        }
        let repeated_by_meta = event
            .meta
            .get("is_repeat")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let repeated_by_hash = event
            .query_hash
            .as_ref()
            .map(|hash| !seen_queries.insert(hash.clone()))
            .unwrap_or(false);
        if repeated_by_meta || repeated_by_hash {
            repeat_questions_avoided += 1;
        }
        if is_value_outcome(&event.outcome) {
            if let Some(wing) = &event.wing {
                *top_wings.entry(wing.clone()).or_default() += 1;
            }
            if let Some(room) = &event.room {
                *top_rooms.entry(room.clone()).or_default() += 1;
            }
        }
        if let Some(hash) = &event.query_hash {
            let prefix = hash.chars().take(12).collect::<String>();
            *query_counts.entry(prefix).or_default() += 1;
        }

        let project = project_acc.entry(event.project.clone()).or_default();
        project.tool_calls += 1;
        project.tokens_saved_est += event.est_tokens_saved;
        if event.outcome == "hit" {
            project.search_hits += 1;
        }
        if event.outcome == "duplicate_skip" {
            project.duplicate_skips += 1;
        }
        if event.outcome == "kg_fact" {
            project.kg_facts_recalled += event.result_count;
        }
        if event.outcome == "diary_recall" {
            project.diary_recalls += event.result_count;
        }
        let repeated_for_project_by_hash = event
            .query_hash
            .as_ref()
            .map(|hash| {
                let seen = seen_project_queries
                    .entry(event.project.clone())
                    .or_default();
                !seen.insert(hash.clone())
            })
            .unwrap_or(false);
        if repeated_by_meta || repeated_for_project_by_hash {
            project.repeat_questions_avoided += 1;
        }
    }

    GainReport {
        window: options.since.label(),
        project: options.project.clone(),
        tool_calls: events.len() as i64,
        sessions: sessions.len() as i64,
        search_calls,
        search_hits,
        hit_rate: if search_calls == 0 {
            0.0
        } else {
            search_hits as f64 / search_calls as f64
        },
        errors,
        tokens_saved_est,
        duplicate_skips,
        kg_facts_recalled,
        diary_recalls,
        repeat_questions_avoided,
        p50_latency_ms: percentile(latencies.clone(), 50.0),
        p95_latency_ms: percentile(latencies, 95.0),
        top_wings: top_counts(top_wings, 5),
        top_rooms: top_counts(top_rooms, 5),
        top_reused_queries: top_counts(query_counts, 5)
            .into_iter()
            .filter(|item| item.count > 1)
            .collect(),
        per_project: project_acc
            .into_iter()
            .map(|(project, acc)| acc.finish(project))
            .collect::<Vec<_>>()
            .tap_sort_projects(),
    }
}

#[derive(Default)]
struct ProjectAccumulator {
    tool_calls: i64,
    search_hits: i64,
    tokens_saved_est: i64,
    duplicate_skips: i64,
    kg_facts_recalled: i64,
    diary_recalls: i64,
    repeat_questions_avoided: i64,
}

impl ProjectAccumulator {
    fn finish(self, project: String) -> ProjectGain {
        ProjectGain {
            project,
            tool_calls: self.tool_calls,
            search_hits: self.search_hits,
            tokens_saved_est: self.tokens_saved_est,
            duplicate_skips: self.duplicate_skips,
            kg_facts_recalled: self.kg_facts_recalled,
            diary_recalls: self.diary_recalls,
            repeat_questions_avoided: self.repeat_questions_avoided,
            value_score: self.tokens_saved_est / 100
                + self.duplicate_skips * 25
                + self.kg_facts_recalled * 5
                + self.diary_recalls * 5
                + self.repeat_questions_avoided * 10,
        }
    }
}

trait SortProjects {
    fn tap_sort_projects(self) -> Self;
}

impl SortProjects for Vec<ProjectGain> {
    fn tap_sort_projects(mut self) -> Self {
        self.sort_by(|a, b| {
            b.value_score
                .cmp(&a.value_score)
                .then_with(|| b.tokens_saved_est.cmp(&a.tokens_saved_est))
                .then_with(|| a.project.cmp(&b.project))
        });
        self
    }
}

fn top_counts(counts: HashMap<String, i64>, limit: usize) -> Vec<NamedCount> {
    let mut items = counts
        .into_iter()
        .map(|(name, count)| NamedCount { name, count })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    items.truncate(limit);
    items
}

fn percentile(mut values: Vec<i64>, percentile: f64) -> i64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let index = ((values.len() as f64 - 1.0) * percentile / 100.0).ceil() as usize;
    values[index.min(values.len() - 1)]
}

fn is_value_outcome(outcome: &str) -> bool {
    matches!(
        outcome,
        "hit" | "duplicate_skip" | "kg_fact" | "diary_recall"
    )
}

fn render_named_counts(items: &[NamedCount]) -> String {
    items
        .iter()
        .map(|item| format!("{}({})", item.name, item.count))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_number(value: i64) -> String {
    let mut chars = value.abs().to_string().chars().rev().collect::<Vec<_>>();
    let mut out = String::new();
    for (index, ch) in chars.drain(..).enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    let formatted = out.chars().rev().collect::<String>();
    if value < 0 {
        format!("-{formatted}")
    } else {
        formatted
    }
}
