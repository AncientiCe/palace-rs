//! Convert a BEAM parquet shard (Mohammadta/BEAM on Hugging Face) into the
//! canonical JSONL shape consumed by `palace-bench prepare-beam` /
//! `palace-bench beam`.
//!
//! Each parquet row holds a single conversation (a list of sessions of turns)
//! plus a `probing_questions` blob keyed by ability category. We emit one
//! JSONL line per probing question, mapping its `source_chat_ids` (turn ids)
//! to the synthetic `session-N` ids that contain those turns.
//!
//! Usage:
//!   beam-convert <input.parquet> <output.jsonl>

use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::{Field, Row};
use serde::Serialize;

mod py_literal;

#[derive(Serialize)]
struct CanonicalSession {
    session_id: String,
    date: Option<String>,
    content: String,
}

#[derive(Serialize)]
struct CanonicalInstance {
    question_id: String,
    question_type: String,
    question: String,
    question_date: Option<String>,
    conversation_id: String,
    sessions: Vec<CanonicalSession>,
    answer_session_ids: Vec<String>,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!(
            "usage: {} <input.parquet> <output.jsonl>",
            args.first().map(String::as_str).unwrap_or("beam-convert")
        );
        return ExitCode::from(2);
    }
    let input = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[2]);
    match run(&input, &output) {
        Ok(written) => {
            println!("wrote {written} probing-question rows to {}", output.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(input: &PathBuf, output: &PathBuf) -> Result<usize> {
    let file = File::open(input).with_context(|| format!("opening {}", input.display()))?;
    let reader = SerializedFileReader::new(file)
        .with_context(|| format!("reading parquet metadata for {}", input.display()))?;

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating output dir {}", parent.display()))?;
    }
    let out_file =
        File::create(output).with_context(|| format!("creating {}", output.display()))?;
    let mut writer = BufWriter::new(out_file);

    let mut written = 0;
    for (row_index, row_result) in reader.get_row_iter(None)?.enumerate() {
        let row = row_result.with_context(|| format!("decoding parquet row {row_index}"))?;
        written += emit_row(&row, row_index, &mut writer)?;
    }
    writer.flush()?;
    Ok(written)
}

fn emit_row<W: Write>(row: &Row, row_index: usize, writer: &mut W) -> Result<usize> {
    let conversation_id = field_string(row, "conversation_id")
        .unwrap_or_else(|| format!("conversation-{}", row_index + 1));
    let chat = field(row, "chat")
        .ok_or_else(|| anyhow!("row {row_index}: missing `chat`"))?;
    let probing_blob = field_string(row, "probing_questions")
        .ok_or_else(|| anyhow!("row {row_index}: missing `probing_questions`"))?;

    let mut chat_to_session: HashMap<i64, String> = HashMap::new();
    let mut sessions: Vec<CanonicalSession> = Vec::new();
    for (session_index, session_field) in iter_list(chat).enumerate() {
        let session_id = format!("session-{session_index}");
        let mut content_lines: Vec<String> = Vec::new();
        for turn in iter_list(session_field) {
            let turn_group = match turn {
                Field::Group(row) => row,
                _ => continue,
            };
            let role = field_string(turn_group, "role").unwrap_or_else(|| "message".to_string());
            let content = field_string(turn_group, "content").unwrap_or_default();
            if let Some(id) = field_int(turn_group, "id") {
                chat_to_session.insert(id, session_id.clone());
            }
            if !content.is_empty() {
                content_lines.push(format!("{role}: {content}"));
            }
        }
        sessions.push(CanonicalSession {
            session_id,
            date: None,
            content: content_lines.join("\n"),
        });
    }

    let probing = py_literal::parse(&probing_blob)
        .with_context(|| format!("row {row_index}: parsing probing_questions"))?;
    let categories = probing
        .as_object()
        .ok_or_else(|| anyhow!("row {row_index}: probing_questions is not a dict"))?;

    let mut written = 0;
    for (category, items) in categories {
        let Some(items) = items.as_array() else {
            continue;
        };
        for (item_index, item) in items.iter().enumerate() {
            let Some(item_obj) = item.as_object() else {
                continue;
            };
            let Some(question) = item_obj.get("question").and_then(|v| v.as_str()) else {
                continue;
            };
            let chat_ids = collect_chat_ids(item_obj.get("source_chat_ids"));
            let mut answer_session_ids: Vec<String> = Vec::new();
            for chat_id in chat_ids {
                if let Some(sid) = chat_to_session.get(&chat_id) {
                    if !answer_session_ids.iter().any(|existing| existing == sid) {
                        answer_session_ids.push(sid.clone());
                    }
                }
            }
            let record = CanonicalInstance {
                question_id: format!("{conversation_id}_{category}_{item_index}"),
                question_type: category.clone(),
                question: question.to_string(),
                question_date: None,
                conversation_id: conversation_id.clone(),
                sessions: sessions.iter().map(clone_session).collect(),
                answer_session_ids,
            };
            serde_json::to_writer(&mut *writer, &record)?;
            writer.write_all(b"\n")?;
            written += 1;
        }
    }
    Ok(written)
}

fn clone_session(session: &CanonicalSession) -> CanonicalSession {
    CanonicalSession {
        session_id: session.session_id.clone(),
        date: session.date.clone(),
        content: session.content.clone(),
    }
}

fn collect_chat_ids(value: Option<&serde_json::Value>) -> Vec<i64> {
    let mut out = Vec::new();
    if let Some(value) = value {
        walk_chat_ids(value, &mut out);
    }
    out
}

fn walk_chat_ids(value: &serde_json::Value, out: &mut Vec<i64>) {
    match value {
        serde_json::Value::Number(number) => {
            if let Some(n) = number.as_i64() {
                out.push(n);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                walk_chat_ids(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values() {
                walk_chat_ids(v, out);
            }
        }
        _ => {}
    }
}

fn field<'a>(row: &'a Row, name: &str) -> Option<&'a Field> {
    row.get_column_iter()
        .find(|(column_name, _)| column_name.as_str() == name)
        .map(|(_, field)| field)
}

fn field_string(row: &Row, name: &str) -> Option<String> {
    match field(row, name)? {
        Field::Str(s) => Some(s.clone()),
        _ => None,
    }
}

fn field_int(row: &Row, name: &str) -> Option<i64> {
    match field(row, name)? {
        Field::Int(value) => Some(i64::from(*value)),
        Field::Long(value) => Some(*value),
        Field::Short(value) => Some(i64::from(*value)),
        Field::Byte(value) => Some(i64::from(*value)),
        Field::UInt(value) => Some(i64::from(*value)),
        Field::ULong(value) => i64::try_from(*value).ok(),
        _ => None,
    }
}

fn iter_list(field: &Field) -> impl Iterator<Item = &Field> {
    match field {
        Field::ListInternal(items) => items.elements().iter(),
        _ => EMPTY_FIELDS.iter(),
    }
}

static EMPTY_FIELDS: [Field; 0] = [];

#[cfg(test)]
mod tests {
    use super::collect_chat_ids;
    use serde_json::json;

    #[test]
    fn collects_from_list_dict_and_scalar() {
        assert_eq!(collect_chat_ids(Some(&json!([1, 2, 3]))), vec![1, 2, 3]);
        assert_eq!(
            collect_chat_ids(Some(&json!({"first": [4], "second": [5, 6]}))),
            vec![4, 5, 6]
        );
        assert_eq!(collect_chat_ids(Some(&json!(7))), vec![7]);
        assert!(collect_chat_ids(Some(&serde_json::Value::Null)).is_empty());
        assert!(collect_chat_ids(None).is_empty());
    }

}
