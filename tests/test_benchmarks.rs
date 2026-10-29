#![cfg(feature = "benchmarks")]

use palace::benchmarks::{
    benchmark_source_file, evaluate_hits, parse_beam_jsonl, parse_longmemeval, run_benchmark,
    BenchmarkSummary, Chunking, RunOptions,
};
use std::fs;
use std::io::Write;

#[test]
fn longmemeval_parser_skips_abstention_and_flattens_sessions() {
    let raw = r#"
    [
      {
        "question_id": "q1",
        "question_type": "single-session-user",
        "question": "What drink does the user prefer?",
        "question_date": "2025-01-02",
        "haystack_session_ids": ["s1", "s2"],
        "haystack_dates": ["2025-01-01", "2025-01-02"],
        "haystack_sessions": [
          [
            {"role": "user", "content": "I prefer green tea."},
            {"role": "assistant", "content": "I will remember that."}
          ],
          [
            {"role": "user", "content": "The weather is clear."}
          ]
        ],
        "answer_session_ids": ["s1"]
      },
      {
        "question_id": "q2_abs",
        "question_type": "single-session-user",
        "question": "What never happened?",
        "haystack_session_ids": ["s3"],
        "haystack_sessions": [[{"role": "user", "content": "Nothing relevant."}]],
        "answer_session_ids": []
      }
    ]
    "#;

    let parsed = parse_longmemeval(raw).expect("fixture should parse");

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].question_id, "q1");
    assert_eq!(parsed[0].sessions.len(), 2);
    assert_eq!(parsed[0].sessions[0].session_id, "s1");
    assert!(parsed[0].sessions[0]
        .content
        .contains("user: I prefer green tea."));
    assert_eq!(parsed[0].answer_session_ids, vec!["s1"]);
}

#[test]
fn beam_parser_accepts_canonical_jsonl_and_evidence_ids() {
    let raw = r#"
{"question_id":"b1","question_type":"information_extraction","question":"Where did Maya move?","conversation_id":"conv-a","sessions":[{"session_id":"turn-1","date":"2025-02-01","content":"Maya moved to Lisbon."}],"answer_session_ids":["turn-1"]}
{"question_id":"b2","question_type":"summarization","question":"Summarize the project","conversation_id":"conv-a","sessions":[{"session_id":"turn-2","turns":[{"role":"user","content":"We planned the release."}]}]}
    "#;

    let parsed = parse_beam_jsonl(raw).expect("fixture should parse");

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].question_id, "b1");
    assert_eq!(parsed[0].answer_session_ids, vec!["turn-1"]);
    assert!(parsed[1].answer_session_ids.is_empty());
    assert!(parsed[1].sessions[0]
        .content
        .contains("user: We planned the release."));
}

#[test]
fn benchmark_source_file_is_stable_and_separator_free() {
    let source = benchmark_source_file("LongMemEval S", "question/one", "session two");

    assert_eq!(source, "longmemeval_s__question_one__session_two");
}

#[test]
fn hit_evaluation_reports_recall_at_k() {
    let ranked = vec!["s3".to_string(), "s2".to_string(), "s1".to_string()];
    let answers = vec!["s1".to_string()];

    let hits = evaluate_hits(&ranked, &answers);

    assert!(!hits.hit_at_1);
    assert!(hits.hit_at_5);
    assert!(hits.hit_at_10);
}

#[test]
fn summary_aggregates_overall_and_per_type_recall() {
    let mut summary = BenchmarkSummary::new("longmemeval");

    summary.record(
        "single-session-user",
        &evaluate_hits(&["s1".into()], &["s1".into()]),
    );
    summary.record(
        "single-session-user",
        &evaluate_hits(&["s2".into(), "s3".into()], &["s1".into()]),
    );
    summary.record(
        "temporal-reasoning",
        &evaluate_hits(&["s4".into()], &["s4".into()]),
    );

    assert_eq!(summary.evaluated_questions, 3);
    assert_eq!(summary.recall_at_1, 2.0 / 3.0);
    assert_eq!(
        summary.per_question_type["single-session-user"].recall_at_1,
        0.5
    );
}

#[test]
fn run_benchmark_processes_multiple_questions_in_bm25_only_mode() {
    let raw = serde_json::json!([
        {
            "question_id": "q1",
            "question_type": "single-session-user",
            "question": "Where did Maya move?",
            "haystack_session_ids": ["s1", "s2", "s3"],
            "haystack_dates": ["2025-01-01", "2025-01-02", "2025-01-03"],
            "haystack_sessions": [
                [{"role": "user", "content": "Maya moved to Lisbon last spring."}],
                [{"role": "user", "content": "We discussed Rust async runtimes."}],
                [{"role": "user", "content": "The weather forecast called for rain."}]
            ],
            "answer_session_ids": ["s1"]
        },
        {
            "question_id": "q2",
            "question_type": "knowledge-update",
            "question": "What database did we switch to?",
            "haystack_session_ids": ["a1", "a2"],
            "haystack_dates": ["2025-02-01", "2025-02-02"],
            "haystack_sessions": [
                [{"role": "user", "content": "We are evaluating PostgreSQL."}],
                [{"role": "user", "content": "We finally migrated everything to SQLite."}]
            ],
            "answer_session_ids": ["a2"]
        }
    ])
    .to_string();

    let tmp = tempfile::tempdir().expect("creating tempdir");
    let input_path = tmp.path().join("longmemeval_fixture.json");
    let output_dir = tmp.path().join("out");
    let mut input_file = fs::File::create(&input_path).expect("creating input file");
    input_file
        .write_all(raw.as_bytes())
        .expect("writing fixture");

    let summary = run_benchmark(&RunOptions {
        benchmark: "longmemeval".to_string(),
        input_path,
        output_dir: output_dir.clone(),
        limit: None,
        bm25_only: true,
        model: palace::embedding_models::default_model(),
        chunking: Chunking::Session,
        dump_chunks: 0,
    })
    .expect("benchmark should run end-to-end in bm25-only mode");

    assert_eq!(summary.benchmark, "longmemeval");
    assert_eq!(summary.model, "bm25-only");
    assert_eq!(summary.evaluated_questions, 2);
    assert_eq!(summary.failed_questions, 0);
    assert_eq!(summary.unscored_questions, 0);
    assert_eq!(summary.drawers_indexed, 5);
    assert_eq!(summary.hits_at_5, 2);

    let cases_path = output_dir.join("longmemeval_cases.jsonl");
    let cases = fs::read_to_string(&cases_path).expect("reading cases output");
    let lines: Vec<&str> = cases.lines().collect();
    assert_eq!(lines.len(), 2, "one JSONL row per question");

    let summary_path = output_dir.join("longmemeval_summary.json");
    let summary_on_disk = fs::read_to_string(&summary_path).expect("reading summary output");
    assert!(summary_on_disk.contains("\"evaluated_questions\": 2"));
}

#[test]
fn chunked_mode_ranks_each_session_once_by_its_best_chunk() {
    // Every chunk of both sessions mentions Maya, so many chunks match; only
    // the last chunk of s1 also says where she will move.
    let filler = "Maya talked about the weather and nothing else of note.\n\n".repeat(40);
    let raw = serde_json::json!([{
        "question_id": "q1",
        "question_type": "single-session-user",
        "question": "Where will Maya move?",
        "haystack_session_ids": ["s1", "s2"],
        "haystack_dates": ["2025-01-01", "2025-01-02"],
        "haystack_sessions": [
            [{"role": "user", "content": format!("{filler}Maya will move to Lisbon next spring.")}],
            [{"role": "user", "content": filler}]
        ],
        "answer_session_ids": ["s1"]
    }])
    .to_string();

    let tmp = tempfile::tempdir().expect("creating tempdir");
    let input_path = tmp.path().join("longmemeval_fixture.json");
    let output_dir = tmp.path().join("out");
    fs::write(&input_path, raw).expect("writing fixture");

    let summary = run_benchmark(&RunOptions {
        benchmark: "longmemeval".to_string(),
        input_path,
        output_dir: output_dir.clone(),
        limit: None,
        bm25_only: true,
        model: palace::embedding_models::default_model(),
        chunking: Chunking::Text,
        dump_chunks: 0,
    })
    .expect("chunked benchmark should run");

    assert!(
        summary.drawers_indexed > 2,
        "long sessions should be split into several chunk drawers"
    );
    assert_eq!(summary.hits_at_1, 1, "the late answer chunk should win");

    let cases = fs::read_to_string(output_dir.join("longmemeval_cases.jsonl"))
        .expect("reading cases output");
    let case: serde_json::Value =
        serde_json::from_str(cases.lines().next().expect("one case")).expect("case json");
    let ranked: Vec<&str> = case["ranked_session_ids"]
        .as_array()
        .expect("ranked ids")
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert_eq!(ranked, ["s1", "s2"], "each session appears once");
}

#[test]
fn exchange_mode_chunks_sessions_the_way_the_conversation_miner_does() {
    let turn = |role: &str, content: &str| serde_json::json!({"role": role, "content": content});
    let raw = serde_json::json!([{
        "question_id": "q1",
        "question_type": "single-session-user",
        "question": "Where will Maya move?",
        "haystack_session_ids": ["s1", "s2"],
        "haystack_dates": ["2025-01-01", "2025-01-02"],
        "haystack_sessions": [
            [
                turn("user", "Maya asked about the weather this week."),
                turn("assistant", "Sunny with light wind most days."),
                turn("user", "Maya also wanted a good book."),
                turn("assistant", "Try a mystery novel for the train."),
                turn("user", "Maya says she will move to Lisbon next spring."),
                turn("assistant", "Lisbon is lovely in spring; plan the move early.")
            ],
            [
                turn("user", "Maya asked about lunch ideas today."),
                turn("assistant", "A salad or a warm soup would work."),
                turn("user", "Maya also asked about dinner."),
                turn("assistant", "Pasta with fresh tomatoes is quick.")
            ]
        ],
        "answer_session_ids": ["s1"]
    }])
    .to_string();

    let tmp = tempfile::tempdir().expect("creating tempdir");
    let input_path = tmp.path().join("longmemeval_fixture.json");
    fs::write(&input_path, raw).expect("writing fixture");

    let summary = run_benchmark(&RunOptions {
        benchmark: "longmemeval".to_string(),
        input_path,
        output_dir: tmp.path().join("out"),
        limit: None,
        bm25_only: true,
        model: palace::embedding_models::default_model(),
        chunking: Chunking::Exchange,
        dump_chunks: 0,
    })
    .expect("exchange benchmark should run");

    assert_eq!(
        summary.drawers_indexed, 5,
        "one drawer per user turn and its reply: 3 + 2"
    );
    assert_eq!(summary.hits_at_1, 1);
}
