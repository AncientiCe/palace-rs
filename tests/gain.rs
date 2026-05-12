use chrono::{Duration, Utc};
use palace::gain::{summarize, GainOptions, SinceWindow};
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
