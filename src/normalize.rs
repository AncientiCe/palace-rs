//! Conversation format normalizer.
//!
//! Converts any supported chat export format to >-style transcript text.
//! Supported: Claude.ai JSON, ChatGPT JSON, Claude Code JSONL, Codex CLI JSONL,
//! Slack JSON, plain text (passthrough).
//!
//! Port of normalize.py.

use anyhow::Result;
use std::path::Path;

/// Load a file and normalize its content to transcript format.
/// Plain text files pass through unchanged.
pub fn normalize_file(filepath: &Path) -> Result<String> {
    let content = std::fs::read_to_string(filepath).unwrap_or_else(|_| String::new());
    Ok(normalize_content(
        &content,
        filepath.extension().and_then(|e| e.to_str()).unwrap_or(""),
    ))
}

/// Normalize an already-loaded string.
pub fn normalize_content(content: &str, ext: &str) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return content.to_string();
    }

    // Already has > markers — pass through
    let quote_count = content
        .lines()
        .filter(|l| l.trim().starts_with('>'))
        .count();
    if quote_count >= 3 {
        return content.to_string();
    }

    let is_json_ext = matches!(ext.to_lowercase().as_str(), "json" | "jsonl");
    let is_json_content = trimmed.starts_with('{') || trimmed.starts_with('[');

    if is_json_ext || is_json_content {
        if let Some(normalized) = try_normalize_json(content) {
            return normalized;
        }
    }

    content.to_string()
}

fn try_normalize_json(content: &str) -> Option<String> {
    // Try JSONL parsers first (line-delimited)
    if let Some(v) = try_claude_code_jsonl(content) {
        return Some(v);
    }
    if let Some(v) = try_codex_jsonl(content) {
        return Some(v);
    }

    // Try single-object JSON parsers
    let data: serde_json::Value = serde_json::from_str(content).ok()?;

    for parser in [
        try_claude_ai_json as fn(&serde_json::Value) -> Option<String>,
        try_chatgpt_json,
        try_slack_json,
    ] {
        if let Some(v) = parser(&data) {
            return Some(v);
        }
    }
    None
}

fn try_claude_code_jsonl(content: &str) -> Option<String> {
    let mut messages: Vec<(String, String)> = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry: serde_json::Value = serde_json::from_str(trimmed).ok()?;
        let msg_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let message = entry.get("message").cloned().unwrap_or_default();
        match msg_type {
            "human" | "user" => {
                let text =
                    extract_content(message.get("content").unwrap_or(&serde_json::Value::Null));
                if !text.is_empty() {
                    messages.push(("user".into(), text));
                }
            }
            "assistant" => {
                let text =
                    extract_content(message.get("content").unwrap_or(&serde_json::Value::Null));
                if !text.is_empty() {
                    messages.push(("assistant".into(), text));
                }
            }
            _ => {}
        }
    }
    if messages.len() >= 2 {
        Some(messages_to_transcript(&messages))
    } else {
        None
    }
}

fn try_codex_jsonl(content: &str) -> Option<String> {
    let mut messages: Vec<(String, String)> = Vec::new();
    let mut has_session_meta = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry: serde_json::Value = serde_json::from_str(trimmed).ok()?;
        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if entry_type == "session_meta" {
            has_session_meta = true;
            continue;
        }
        if entry_type != "event_msg" {
            continue;
        }
        let payload = entry.get("payload")?;
        let payload_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let msg = payload.get("message").and_then(|v| v.as_str())?;
        let text = msg.trim().to_string();
        if text.is_empty() {
            continue;
        }
        match payload_type {
            "user_message" => messages.push(("user".into(), text)),
            "agent_message" => messages.push(("assistant".into(), text)),
            _ => {}
        }
    }

    if messages.len() >= 2 && has_session_meta {
        Some(messages_to_transcript(&messages))
    } else {
        None
    }
}

fn try_claude_ai_json(data: &serde_json::Value) -> Option<String> {
    let msgs_val = match data {
        serde_json::Value::Object(o) => o
            .get("messages")
            .or_else(|| o.get("chat_messages"))
            .cloned()
            .unwrap_or_else(|| data.clone()),
        _ => data.clone(),
    };

    let list = msgs_val.as_array()?;

    // Privacy export: array of conversation objects with chat_messages
    if list.first()?.get("chat_messages").is_some() {
        let mut all: Vec<(String, String)> = Vec::new();
        for convo in list {
            let chat_msgs = convo.get("chat_messages")?.as_array()?;
            for item in chat_msgs {
                let role = item.get("role").and_then(|v| v.as_str()).unwrap_or("");
                let text = extract_content(item.get("content").unwrap_or(&serde_json::Value::Null));
                match role {
                    "user" | "human" if !text.is_empty() => all.push(("user".into(), text)),
                    "assistant" | "ai" if !text.is_empty() => all.push(("assistant".into(), text)),
                    _ => {}
                }
            }
        }
        return if all.len() >= 2 {
            Some(messages_to_transcript(&all))
        } else {
            None
        };
    }

    // Flat messages list
    let mut messages: Vec<(String, String)> = Vec::new();
    for item in list {
        let role = item.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let text = extract_content(item.get("content").unwrap_or(&serde_json::Value::Null));
        match role {
            "user" | "human" if !text.is_empty() => messages.push(("user".into(), text)),
            "assistant" | "ai" if !text.is_empty() => messages.push(("assistant".into(), text)),
            _ => {}
        }
    }
    if messages.len() >= 2 {
        Some(messages_to_transcript(&messages))
    } else {
        None
    }
}

fn try_chatgpt_json(data: &serde_json::Value) -> Option<String> {
    let mapping = data.get("mapping")?.as_object()?;
    let mut messages: Vec<(String, String)> = Vec::new();

    // Find root node (parent == null, no message)
    let root_id = mapping
        .iter()
        .find(|(_, node)| {
            node.get("parent").is_some_and(|p| p.is_null()) && node.get("message").is_none()
        })
        .or_else(|| {
            mapping
                .iter()
                .find(|(_, node)| node.get("parent").is_some_and(|p| p.is_null()))
        })
        .map(|(id, _)| id.clone())?;

    let mut current_id: Option<String> = Some(root_id);
    let mut visited = std::collections::HashSet::new();

    while let Some(id) = current_id {
        if visited.contains(&id) {
            break;
        }
        visited.insert(id.clone());
        let node = mapping.get(&id)?;
        if let Some(msg) = node.get("message") {
            let role = msg
                .get("author")
                .and_then(|a| a.get("role"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let content = msg.get("content").cloned().unwrap_or_default();
            let parts = content
                .get("parts")
                .and_then(|p| p.as_array())
                .map(|p| {
                    p.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim()
                        .to_string()
                })
                .unwrap_or_default();
            match role {
                "user" if !parts.is_empty() => messages.push(("user".into(), parts)),
                "assistant" if !parts.is_empty() => messages.push(("assistant".into(), parts)),
                _ => {}
            }
        }
        current_id = node
            .get("children")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
            .and_then(|v| v.as_str())
            .map(String::from);
    }

    if messages.len() >= 2 {
        Some(messages_to_transcript(&messages))
    } else {
        None
    }
}

fn try_slack_json(data: &serde_json::Value) -> Option<String> {
    let list = data.as_array()?;
    let mut messages: Vec<(String, String)> = Vec::new();
    let mut seen_users: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut last_role = String::new();

    for item in list {
        if item.get("type").and_then(|v| v.as_str()) != Some("message") {
            continue;
        }
        let user_id = item
            .get("user")
            .or_else(|| item.get("username"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let text = item
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() || user_id.is_empty() {
            continue;
        }
        let role = seen_users.entry(user_id).or_insert_with(|| {
            if last_role.is_empty() || last_role == "assistant" {
                "user".to_string()
            } else {
                "assistant".to_string()
            }
        });
        last_role = role.clone();
        messages.push((role.clone(), text));
    }

    if messages.len() >= 2 {
        Some(messages_to_transcript(&messages))
    } else {
        None
    }
}

fn extract_content(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|item| match item {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Object(o) => {
                    if o.get("type").and_then(|v| v.as_str()) == Some("text") {
                        o.get("text").and_then(|v| v.as_str()).map(String::from)
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string(),
        serde_json::Value::Object(o) => o
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

/// Escape a leading `>` on each line, as markdown does with `\>`, so text
/// quoted inside a message isn't read as a new user turn when chunked.
fn escape_turn_markers(text: &str) -> String {
    text.lines()
        .map(|line| {
            let body = line.trim_start();
            if body.starts_with('>') {
                let indent = &line[..line.len() - body.len()];
                format!("{indent}\\{body}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn messages_to_transcript(messages: &[(String, String)]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let (role, text) = &messages[i];
        if role == "user" {
            // Only the first line of a user message marks the turn.
            let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
            lines.push(format!("> {first}"));
            if !rest.is_empty() {
                lines.push(escape_turn_markers(rest));
            }
            if i + 1 < messages.len() && messages[i + 1].0 == "assistant" {
                lines.push(escape_turn_markers(&messages[i + 1].1));
                i += 2;
            } else {
                i += 1;
            }
        } else {
            lines.push(escape_turn_markers(text));
            i += 1;
        }
        lines.push(String::new());
    }
    lines.join("\n")
}
