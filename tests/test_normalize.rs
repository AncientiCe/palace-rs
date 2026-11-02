use palace::normalize::normalize_content;

#[test]
fn passthrough_when_already_has_markers() {
    let text = "> Hello\nWorld\n> Goodbye\nBye\n> Again\nYes";
    let result = normalize_content(text, "txt");
    assert_eq!(result, text);
}

#[test]
fn empty_content_passes_through() {
    let result = normalize_content("   ", "txt");
    assert_eq!(result, "   ");
}

#[test]
fn claude_ai_json_flat_list() {
    let json = serde_json::json!([
        {"role": "user", "content": "What is Rust?"},
        {"role": "assistant", "content": "Rust is a systems programming language."},
        {"role": "user", "content": "Is it fast?"},
        {"role": "assistant", "content": "Yes, very fast."},
    ])
    .to_string();
    let result = normalize_content(&json, "json");
    assert!(result.contains("> What is Rust?"), "result: {result}");
    assert!(
        result.contains("Rust is a systems programming language."),
        "result: {result}"
    );
}

#[test]
fn claude_code_jsonl() {
    let jsonl = r#"{"type":"human","message":{"content":"Hello claude"}}
{"type":"assistant","message":{"content":"Hi there!"}}
{"type":"human","message":{"content":"How are you?"}}
{"type":"assistant","message":{"content":"Doing well."}}"#;
    let result = normalize_content(jsonl, "jsonl");
    assert!(result.contains("> Hello claude"), "result: {result}");
    assert!(result.contains("Hi there!"), "result: {result}");
}

#[test]
fn codex_jsonl_session() {
    let jsonl = r#"{"type":"session_meta","session_id":"abc"}
{"type":"event_msg","payload":{"type":"user_message","message":"What is 2+2?"}}
{"type":"event_msg","payload":{"type":"agent_message","message":"It is 4."}}
{"type":"event_msg","payload":{"type":"user_message","message":"Thanks!"}}
{"type":"event_msg","payload":{"type":"agent_message","message":"No problem."}}"#;
    let result = normalize_content(jsonl, "jsonl");
    assert!(result.contains("> What is 2+2?"), "result: {result}");
    assert!(result.contains("It is 4."), "result: {result}");
}

#[test]
fn slack_json() {
    let json = serde_json::json!([
        {"type": "message", "user": "U001", "text": "Hey!"},
        {"type": "message", "user": "U002", "text": "Hey back!"},
        {"type": "message", "user": "U001", "text": "How are you?"},
        {"type": "message", "user": "U002", "text": "Great thanks."},
    ])
    .to_string();
    let result = normalize_content(&json, "json");
    assert!(result.contains("> Hey!"), "result: {result}");
}

#[test]
fn plain_text_passes_through() {
    let text = "Just a regular file with no special format.";
    let result = normalize_content(text, "txt");
    assert_eq!(result, text);
}

fn claude_code_jsonl_from(messages: &[(&str, &str)]) -> String {
    messages
        .iter()
        .map(|(kind, text)| {
            serde_json::json!({"type": kind, "message": {"content": text}}).to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn quote_in_assistant_reply_is_not_a_user_turn() {
    let jsonl = claude_code_jsonl_from(&[
        ("human", "How do I configure the cache?"),
        (
            "assistant",
            "Set the TTL in config.toml:\n> Note: values are in seconds, not minutes.\nThen restart the server so it picks up the change.",
        ),
        ("human", "Where do the cache files live?"),
        ("assistant", "Under the data directory, in a folder named cache."),
        ("human", "Can I clear it safely?"),
        ("assistant", "Yes, deleting the folder only costs a slower first request."),
    ]);

    let transcript = normalize_content(&jsonl, "jsonl");
    let chunks = palace::convo_miner::chunk_exchanges(&transcript);

    assert_eq!(chunks.len(), 3, "one chunk per real exchange: {chunks:#?}");
    let cache = &chunks[0].0;
    assert!(
        cache.starts_with("> How do I configure the cache?"),
        "{cache}"
    );
    assert!(cache.contains("values are in seconds"), "{cache}");
    assert!(cache.contains("Then restart the server"), "{cache}");
}

#[test]
fn quote_on_a_later_line_of_a_user_message_is_not_a_new_turn() {
    let jsonl = claude_code_jsonl_from(&[
        (
            "human",
            "The deploy failed with this log:\n> error: migration 42 timed out\nWhat went wrong?",
        ),
        (
            "assistant",
            "Migration 42 holds a table lock longer than the timeout.",
        ),
        ("human", "How do I raise the timeout?"),
        (
            "assistant",
            "Set lock_timeout in the migration settings file.",
        ),
        ("human", "Will that slow other queries?"),
        (
            "assistant",
            "Only queries waiting on that table during the migration.",
        ),
    ]);

    let transcript = normalize_content(&jsonl, "jsonl");
    let chunks = palace::convo_miner::chunk_exchanges(&transcript);

    assert_eq!(chunks.len(), 3, "one chunk per real exchange: {chunks:#?}");
    let deploy = &chunks[0].0;
    assert!(deploy.starts_with("> The deploy failed"), "{deploy}");
    assert!(deploy.contains("migration 42 timed out"), "{deploy}");
    assert!(deploy.contains("holds a table lock"), "{deploy}");
}
