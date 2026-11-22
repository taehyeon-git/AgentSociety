//! Portable provider settings, usable when replaying without the HTTP feature.
use agentsociety_core::{DomainError, MAX_RECALL_RESULTS, Result, Tick};
use serde::{Deserialize, Serialize};
use url::Url;

/// Supported structured chat protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// Ollama's non-streaming `/api/chat` endpoint.
    Ollama,
    /// The `/chat/completions` protocol used by OpenAI-compatible servers.
    OpenaiCompatible,
}

impl Provider {
    /// Stable protocol name for audit records.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::OpenaiCompatible => "openai_compatible",
        }
    }
}

/// Explicit opt-in connection and request limits; API keys never belong in this structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LlmSettings {
    /// HTTP chat protocol.
    pub provider: Provider,
    /// Provider base URL, without the chat endpoint suffix, credentials, or query.
    pub base_url: String,
    /// Model served by the chosen provider.
    pub model: String,
    /// Optional environment variable containing a bearer token. Only its name is stored.
    pub api_key_env: Option<String>,
    /// Whole-batch deadline, including time queued for concurrency slots.
    pub timeout_ms: u64,
    /// Maximum simultaneous HTTP requests.
    pub max_concurrency: usize,
    /// Maximum scheduled requests in a tick.
    pub max_calls_per_tick: usize,
    /// Maximum scheduled requests across a run. Zero keeps all decisions offline.
    pub max_total_calls: u64,
    /// Minimum ticks between requests for the same agent, including failures.
    pub min_interval_ticks: Tick,
    /// Generation token limit passed to the provider.
    pub max_output_tokens: u32,
    /// Maximum selected memories per prompt, also subject to a byte limit.
    pub memory_limit: usize,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            provider: Provider::Ollama,
            base_url: "http://127.0.0.1:11434".into(),
            model: "llama3.2:3b".into(),
            api_key_env: None,
            timeout_ms: 1500,
            max_concurrency: 2,
            max_calls_per_tick: 4,
            max_total_calls: 100,
            min_interval_ticks: 10,
            max_output_tokens: 256,
            memory_limit: 8,
        }
    }
}

impl LlmSettings {
    /// Validate settings before creating a client or reading credentials.
    pub fn validate(&self) -> Result<()> {
        let url = Url::parse(&self.base_url)
            .map_err(|_| DomainError::InvalidConfig("invalid LLM base_url".into()))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(DomainError::InvalidConfig(
                "LLM base_url must be HTTP(S), without credentials, query, or fragment".into(),
            ));
        }
        if self.model.trim().is_empty() || self.model.chars().count() > 128 {
            return Err(DomainError::InvalidConfig(
                "LLM model must contain 1..=128 characters".into(),
            ));
        }
        if !(1..=30_000).contains(&self.timeout_ms)
            || !(1..=32).contains(&self.max_concurrency)
            || !(1..=128).contains(&self.max_calls_per_tick)
            || self.max_total_calls > 1_000_000
            || self.min_interval_ticks == 0
            || !(1..=4096).contains(&self.max_output_tokens)
            || self.memory_limit > MAX_RECALL_RESULTS
        {
            return Err(DomainError::InvalidConfig(
                "invalid LLM timeout, concurrency, call, interval, token, or memory limit".into(),
            ));
        }
        if self.api_key_env.as_ref().is_some_and(|name| {
            name.is_empty() || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        }) {
            return Err(DomainError::InvalidConfig(
                "invalid API key environment variable name".into(),
            ));
        }
        Ok(())
    }
}
