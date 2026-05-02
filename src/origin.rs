//! Corpus-origin detection for project and conversation mining.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginPlatform {
    ProjectFiles,
    AiDialogue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusOrigin {
    pub schema_version: u32,
    pub platform: OriginPlatform,
    pub user_name: Option<String>,
    pub agent_persona_names: Vec<String>,
}

pub fn detect_origin(sample: &str) -> CorpusOrigin {
    let lower = sample.to_lowercase();
    let has_turn_markers = lower.contains("user:")
        || lower.contains("assistant:")
        || lower.contains("human:")
        || lower.contains("claude:");
    let known_agents = ["Claude", "Gemini", "Codex", "Assistant"];
    let agent_persona_names: Vec<String> = known_agents
        .iter()
        .filter(|agent| lower.contains(&agent.to_lowercase()))
        .map(|agent| (*agent).to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    CorpusOrigin {
        schema_version: 1,
        platform: if has_turn_markers || !agent_persona_names.is_empty() {
            OriginPlatform::AiDialogue
        } else {
            OriginPlatform::ProjectFiles
        },
        user_name: None,
        agent_persona_names,
    }
}

pub fn write_origin(path: &Path, origin: &CorpusOrigin) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(origin)?)?;
    Ok(())
}
