use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct KnowledgeConfig {
    pub enabled: bool,
    /// "ollama" or "openai" (OpenAI-compatible: OpenRouter, DeepSeek, etc.)
    pub provider: String,
    /// Provider endpoint (e.g. "http://10.0.0.1:11434", "https://api.openai.com/v1")
    /// Falls back to global ollama.api if unset and provider == "ollama"
    pub api_base: Option<String>,
    /// API key (required for openai provider)
    pub api_key: Option<String>,
    pub chat_model: Option<String>,
    pub embedding_model: Option<String>,
    /// Max chars per digest chunk sent to LLM (default 20000 ≈ 16K tokens)
    pub max_chunk_chars: usize,
    pub match_threshold: f64,
    pub cluster_threshold: f64,
    pub pipeline_version: String,
    pub redact_before_llm: bool,
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "ollama".to_string(),
            api_base: None,
            api_key: None,
            chat_model: None,
            embedding_model: None,
            max_chunk_chars: 20000,
            match_threshold: 0.82,
            cluster_threshold: 0.76,
            pipeline_version: "v1".to_string(),
            redact_before_llm: true,
        }
    }
}
