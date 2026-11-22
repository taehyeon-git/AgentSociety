use crate::{AgentId, Decision, Tick};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Explicit outcome at the external-decision boundary, including rule fallbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalOutcome {
    /// A schema-valid, feasible intent was accepted for engine revalidation.
    Accepted,
    /// Model output did not satisfy the strict response schema.
    InvalidResponse,
    /// A typed intent was not feasible in the observed world.
    InvalidAction,
    /// The entire decision batch reached its deadline.
    Timeout,
    /// HTTP, connection, authentication, or provider protocol failure.
    ProviderError,
    /// A run or per-tick call budget prevented a request.
    BudgetExhausted,
}

/// Auditable model result. Credentials and raw HTTP responses are never included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalDecision {
    /// Agent whose decision was requested.
    pub agent_id: AgentId,
    /// The only tick at which this result may be applied.
    pub target_tick: Tick,
    /// Provider protocol, not a credential-bearing endpoint.
    pub provider: String,
    /// Configured model name.
    pub model: String,
    /// Accepted result or the reason rules are used instead.
    pub outcome: ExternalOutcome,
    /// Accepted structured decision; absent for fallback outcomes.
    pub decision: Option<Decision>,
    /// Brief model summary or bounded, sanitized failure explanation.
    pub summary: String,
}

/// Completed external results in stable AgentId order, reusable without a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionBatch {
    /// Target tick; stale or future batches are rejected before mutation.
    pub target_tick: Tick,
    /// One external outcome per participating agent, sorted for replay.
    pub decisions: BTreeMap<AgentId, ExternalDecision>,
}

/// Per-run request accounting, kept outside deterministic engine state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmUsage {
    /// Scheduled attempts, including canceled requests waiting for concurrency slots.
    pub attempts: u64,
    /// Model intents accepted before the engine's final validation.
    pub accepted: u64,
    /// Failed or skipped external decisions that use offline rules instead.
    pub fallbacks: u64,
    /// Number of responses with reported token usage.
    pub usage_reports: u64,
    /// Provider-reported input tokens (unreported calls are excluded).
    pub input_tokens: u64,
    /// Provider-reported output tokens (unreported calls are excluded).
    pub output_tokens: u64,
}

/// Scheduler state persisted at completed tick boundaries, without live tasks or keys.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmState {
    /// Cumulative request and token accounting.
    pub usage: LlmUsage,
    /// Last attempted request tick for each agent, including failed requests.
    pub last_request: BTreeMap<AgentId, Tick>,
}
