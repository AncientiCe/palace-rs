use chrono::{Duration, Utc};
use palace::gain::{
    history, record_feedback, render_history, render_text, summarize, FeedbackRecord, GainOptions,
    SinceWindow,
};
use palace::usage::{insert_event, UsageEvent, UsageSession};
use serde_json::json;
use std::time::Duration as StdDuration;

fn event(ts: String, project: &str, tool: &str, outcome: &str) -> UsageEvent {
    UsageEvent {
        ts,
        session_id: "session_test".to_string(),
        project: project.to_string(),
        tool: tool.to_string(),
        wing: Some("wing_alpha".to_string()),
        room: Some("room_alpha".to_string()),
        query_hash: None,
        result_count: 0,
        top_similarity: None,
        bytes_returned: 0,
        est_tokens_saved: 0,
        duration_ms: 10,
        outcome: outcome.to_string(),
        meta: json!({}),
    }
}

fn options(project: Option<&str>, since: SinceWindow) -> GainOptions {
    GainOptions {
        project: project.map(str::to_string),
        since,
    }
}

#[test]
fn empty_gain_report_is_zeroed() {
    let conn = palace::db::open_in_memory().expect("open test db");
    let report = summarize(&conn, &options(None, SinceWindow::All)).expect("summarize");

    assert_eq!(report.tool_calls, 0);
    assert_eq!(report.sessions, 0);
    assert_eq!(report.hit_rate, 0.0);
    assert_eq!(report.tokens_saved_est, 0);
    assert!(report.per_project.is_empty());
}

#[test]
fn rendered_gain_summary_uses_palace_branding() {
    let conn = palace::db::open_in_memory().expect("open test db");
    let report = summarize(&conn, &options(None, SinceWindow::All)).expect("summarize");

    let text = render_text(&report);

    assert!(text.starts_with("Palace gain - all time (all projects)\n"));
    assert!(!text.contains("MemPalace"));
}

#[test]
fn rendered_gain_history_uses_palace_branding() {
    let conn = palace::db::open_in_memory().expect("open test db");

    let empty = render_history(&[]);
    assert_eq!(empty, "No Palace gain history yet.\n");
    assert!(!empty.contains("MemPalace"));

    insert_event(
        &conn,
        &event(Utc::now().to_rfc3339(), "alpha", "palace_search", "hit"),
    )
    .expect("insert event");
    let events = history(&conn, &options(None, SinceWindow::All), 20).expect("history");
    let text = render_history(&events);

    assert!(text.starts_with("Palace gain history\n"));
    assert!(!text.contains("MemPalace"));
}

#[test]
fn aggregates_value_metrics_by_project() {
    let conn = palace::db::open_in_memory().expect("open test db");
    let now = Utc::now().to_rfc3339();

    let mut search = event(now.clone(), "alpha", "palace_search", "hit");
    search.result_count = 3;
    search.est_tokens_saved = 1200;
    insert_event(&conn, &search).expect("insert search");

    let mut kg = event(now.clone(), "alpha", "palace_kg_query", "kg_fact");
    kg.result_count = 2;
    kg.est_tokens_saved = 200;
    insert_event(&conn, &kg).expect("insert kg");

    let duplicate = event(now.clone(), "alpha", "palace_add_drawer", "duplicate_skip");
    insert_event(&conn, &duplicate).expect("insert duplicate");

    let mut diary = event(now, "beta", "palace_diary_read", "diary_recall");
    diary.result_count = 4;
    diary.est_tokens_saved = 400;
    insert_event(&conn, &diary).expect("insert diary");

    let report = summarize(&conn, &options(None, SinceWindow::All)).expect("summarize");

    assert_eq!(report.tool_calls, 4);
    assert_eq!(report.search_calls, 1);
    assert_eq!(report.search_hits, 1);
    assert_eq!(report.hit_rate, 1.0);
    assert_eq!(report.tokens_saved_est, 1800);
    assert_eq!(report.duplicate_skips, 1);
    assert_eq!(report.kg_facts_recalled, 2);
    assert_eq!(report.diary_recalls, 4);
    assert_eq!(report.per_project.len(), 2);
    assert_eq!(report.per_project[0].project, "alpha");
    assert!(report
        .per_tool_latency
        .iter()
        .any(|item| item.tool == "palace_search"
            && item.calls == 1
            && item.p50_latency_ms == 10
            && item.p95_latency_ms == 10));
}

#[test]
fn summarizes_latency_by_tool() {
    let conn = palace::db::open_in_memory().expect("open test db");
    let now = Utc::now().to_rfc3339();

    let mut first = event(now.clone(), "alpha", "palace_search", "hit");
    first.duration_ms = 5;
    insert_event(&conn, &first).expect("insert first");

    let mut second = event(now.clone(), "alpha", "palace_search", "hit");
    second.duration_ms = 25;
    insert_event(&conn, &second).expect("insert second");

    let mut third = event(now, "alpha", "palace_verify", "hit");
    third.duration_ms = 2;
    insert_event(&conn, &third).expect("insert third");

    let report = summarize(&conn, &options(None, SinceWindow::All)).expect("summarize");

    let search = report
        .per_tool_latency
        .iter()
        .find(|item| item.tool == "palace_search")
        .expect("search latency");
    assert_eq!(search.calls, 2);
    assert_eq!(search.p50_latency_ms, 25);
    assert_eq!(search.p95_latency_ms, 25);

    let verify = report
        .per_tool_latency
        .iter()
        .find(|item| item.tool == "palace_verify")
        .expect("verify latency");
    assert_eq!(verify.calls, 1);
    assert_eq!(verify.p50_latency_ms, 2);
}

#[test]
fn repeated_query_hashes_count_as_repeat_questions_avoided() {
    let conn = palace::db::open_in_memory().expect("open test db");
    let now = Utc::now().to_rfc3339();

    let mut first = event(now.clone(), "alpha", "palace_search", "hit");
    first.query_hash = Some("same_hash".to_string());
    insert_event(&conn, &first).expect("insert first");

    let mut second = event(now, "alpha", "palace_search", "hit");
    second.query_hash = Some("same_hash".to_string());
    insert_event(&conn, &second).expect("insert second");

    let report = summarize(&conn, &options(None, SinceWindow::All)).expect("summarize");

    assert_eq!(report.repeat_questions_avoided, 1);
    assert_eq!(report.per_project[0].repeat_questions_avoided, 1);
}

#[test]
fn since_window_filters_old_events() {
    let conn = palace::db::open_in_memory().expect("open test db");
    let old = (Utc::now() - Duration::days(10)).to_rfc3339();
    let recent = Utc::now().to_rfc3339();

    insert_event(&conn, &event(old, "alpha", "palace_search", "hit")).expect("insert old");
    insert_event(&conn, &event(recent, "alpha", "palace_search", "hit")).expect("insert recent");

    let report = summarize(&conn, &options(None, SinceWindow::Days(7))).expect("summarize");

    assert_eq!(report.tool_calls, 1);
}

#[test]
fn recorder_classifies_search_hits_and_repeats() {
    let conn = palace::db::open_in_memory().expect("open test db");
    let session = UsageSession {
        session_id: "session_record".to_string(),
        project: "alpha".to_string(),
    };
    let args = json!({"query": "where is gain shown"});
    let result = json!({
        "results": [
            {
                "wing": "wing_alpha",
                "room": "room_alpha",
                "similarity": 0.91,
                "text": "A useful remembered answer."
            }
        ]
    });

    palace::usage::record_event(
        &conn,
        &session,
        "palace_search",
        &args,
        &result,
        StdDuration::from_millis(7),
    )
    .expect("record first");
    palace::usage::record_event(
        &conn,
        &session,
        "palace_search",
        &args,
        &result,
        StdDuration::from_millis(8),
    )
    .expect("record repeat");

    let report = summarize(&conn, &options(Some("alpha"), SinceWindow::All)).expect("summarize");

    assert_eq!(report.search_hits, 2);
    assert_eq!(report.repeat_questions_avoided, 1);
    assert!(report.tokens_saved_est > 0);
}

#[test]
fn explicit_feedback_records_precision_at_1() {
    let conn = palace::db::open_in_memory().expect("open test db");
    let now = Utc::now().to_rfc3339();

    let mut search = event(now, "alpha", "palace_search", "hit");
    search.meta = json!({
        "query_id": "query_1",
        "intent": "preference",
        "top_drawer_ids": ["drawer_good", "drawer_other"],
        "top_drawer_id": "drawer_good"
    });
    insert_event(&conn, &search).expect("insert search");
    record_feedback(
        &conn,
        &FeedbackRecord {
            query_id: "query_1".to_string(),
            drawer_id: "drawer_good".to_string(),
            verdict: "useful".to_string(),
            note: None,
        },
    )
    .expect("record feedback");

    let report = summarize(&conn, &options(Some("alpha"), SinceWindow::All)).expect("summarize");

    assert_eq!(report.precision_at_1, Some(1.0));
    assert_eq!(report.precision_at_5, Some(1.0));
    assert_eq!(report.per_intent_precision[0].intent, "preference");
    assert!(render_text(&report).contains("Precision@1"));
}

#[test]
fn diary_citation_infers_useful_feedback() {
    let conn = palace::db::open_in_memory().expect("open test db");
    let now = Utc::now().to_rfc3339();

    let mut search = event(now.clone(), "alpha", "palace_search", "hit");
    search.meta = json!({
        "query_id": "query_implicit",
        "intent": "decision",
        "top_drawer_ids": ["drawer_cited"],
        "top_drawer_id": "drawer_cited"
    });
    insert_event(&conn, &search).expect("insert search");

    let mut diary = event(now, "alpha", "palace_diary_write", "diary_write");
    diary.meta = json!({"referenced_drawer_ids": ["drawer_cited"]});
    insert_event(&conn, &diary).expect("insert diary");

    let report = summarize(&conn, &options(Some("alpha"), SinceWindow::All)).expect("summarize");

    assert_eq!(report.precision_at_1, Some(1.0));
}

#[test]
fn recorder_classifies_diary_warm_start_and_search_as_recall() {
    let conn = palace::db::open_in_memory().expect("open test db");
    let session = UsageSession {
        session_id: "session_diary".to_string(),
        project: "alpha".to_string(),
    };
    let result = json!({
        "agent": "codex",
        "entries": [
            {"text": "Prior decision", "topic": "planning"}
        ]
    });

    palace::usage::record_event(
        &conn,
        &session,
        "palace_session_context",
        &json!({"agent_name": "codex"}),
        &result,
        StdDuration::from_millis(3),
    )
    .expect("record session context");
    palace::usage::record_event(
        &conn,
        &session,
        "palace_diary_search",
        &json!({"agent_name": "codex", "query": "prior decision"}),
        &result,
        StdDuration::from_millis(4),
    )
    .expect("record diary search");

    let report = summarize(&conn, &options(Some("alpha"), SinceWindow::All)).expect("summarize");

    assert_eq!(report.diary_recalls, 2);
    assert!(report.tokens_saved_est > 0);
}

#[test]
fn recorder_classifies_kg_query_shapes_as_facts() {
    let conn = palace::db::open_in_memory().expect("open test db");
    let session = UsageSession {
        session_id: "session_kg".to_string(),
        project: "alpha".to_string(),
    };

    palace::usage::record_event(
        &conn,
        &session,
        "palace_kg_query",
        &json!({"entity": "palace"}),
        &json!({"relationships": [{"predicate": "supports", "object": "codex"}]}),
        StdDuration::from_millis(3),
    )
    .expect("record kg");

    let report = summarize(&conn, &options(Some("alpha"), SinceWindow::All)).expect("summarize");

    assert_eq!(report.kg_facts_recalled, 1);
}

#[test]
fn recorder_marks_repeated_diary_search_as_repeat_question() {
    let conn = palace::db::open_in_memory().expect("open test db");
    let session = UsageSession {
        session_id: "session_repeat_diary".to_string(),
        project: "alpha".to_string(),
    };
    let args = json!({"agent_name": "codex", "query": "what was the adoption plan"});
    let result = json!({"entries": [{"text": "Use all four agents."}]});

    palace::usage::record_event(
        &conn,
        &session,
        "palace_diary_search",
        &args,
        &result,
        StdDuration::from_millis(4),
    )
    .expect("record first");
    palace::usage::record_event(
        &conn,
        &session,
        "palace_diary_search",
        &args,
        &result,
        StdDuration::from_millis(5),
    )
    .expect("record repeat");

    let report = summarize(&conn, &options(Some("alpha"), SinceWindow::All)).expect("summarize");

    assert_eq!(report.diary_recalls, 2);
    assert_eq!(report.repeat_questions_avoided, 1);
}

#[test]
fn mcp_dispatch_records_usage_event() {
    let conn = palace::db::open_in_memory().expect("open test db");
    let config = palace::config::PalaceConfig::new();
    let session = UsageSession {
        session_id: "session_mcp".to_string(),
        project: "alpha".to_string(),
    };

    let result = palace::mcp_server::dispatch_tool_with_usage(
        &conn,
        &config,
        &session,
        "palace_status",
        &json!({}),
    );

    assert!(result.get("total_drawers").is_some());

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM usage_events", [], |row| row.get(0))
        .expect("count usage events");
    let outcome: String = conn
        .query_row("SELECT outcome FROM usage_events LIMIT 1", [], |row| {
            row.get(0)
        })
        .expect("read outcome");

    assert_eq!(count, 1);
    assert_eq!(outcome, "noop");
}
