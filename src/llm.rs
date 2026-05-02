//! Optional LLM-assisted init refinement.

use crate::origin::CorpusOrigin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmProvider {
    Ollama,
    OpenAiCompat,
    Anthropic,
}

pub fn detect_provider() -> Option<LlmProvider> {
    if std::env::var("MEMPALACE_OPENAI_COMPAT_BASE_URL").is_ok() {
        return Some(LlmProvider::OpenAiCompat);
    }
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        return Some(LlmProvider::Anthropic);
    }
    if std::env::var("OLLAMA_HOST").is_ok() {
        return Some(LlmProvider::Ollama);
    }
    None
}

pub fn refine_origin(origin: CorpusOrigin, _sample: &str) -> CorpusOrigin {
    // Missing providers are an expected offline path; keep heuristic output.
    if detect_provider().is_none() {
        return origin;
    }
    origin
}
