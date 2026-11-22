use crate::{
    DomainError, InteractionSettings, Money, OrganizationSettings, PolicyKind, Prices, Result,
    ScenarioConfig,
};
use serde::{Deserialize, Serialize};

/// Maximum accepted limit price and published market price.
pub const MAX_PRICE: Money = 1_000_000;
/// Maximum quantity on a single external intent.
pub const MAX_ORDER_QUANTITY: u64 = 1_000_000;
/// Version of simulation rules, separate from the serialization format.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Version of exported state and event records.
pub const FORMAT_VERSION: u32 = 6;
/// Conservative bound for a tick including bounded proposal expiry and social feedback.
pub const MAX_EVENTS_PER_AGENT: usize = 160;

/// Reproducible initial conditions; unknown JSON fields are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SimulationConfig {
    /// Number of agents (zero is supported).
    pub agents: usize,
    /// ChaCha8 seed; used for all initial variation and policy tie breakers.
    pub seed: u64,
    /// Starting wallet per agent, before seeded +/-20% variation.
    pub initial_cash: Money,
    /// Initial market prices and agents' long-run reservation anchors.
    pub initial_prices: Prices,
    /// City width in integer cells.
    pub map_width: u16,
    /// City height in integer cells.
    pub map_height: u16,
    /// Number of recent decisions retained per agent (0 disables retention).
    pub history_capacity: usize,
    /// Maximum directed social relationships retained per agent.
    pub relationship_capacity: usize,
    /// Maximum structured memories retained per agent (0 disables retention).
    pub memory_capacity: usize,
    /// Policy assigned to the first `llm_agents`; all remaining agents use rules.
    pub policy: PolicyKind,
    /// Maximum number of agents allowed to use the selected external policy.
    pub llm_agents: usize,
    /// Optional negotiation, joint production, and periodic objective adaptation.
    pub interactions: InteractionSettings,
    /// Optional member-owned cooperatives and recurring supply agreements.
    pub organizations: OrganizationSettings,
    /// Recorded environmental and fiscal interventions.
    pub scenario: ScenarioConfig,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            agents: 100,
            seed: 42,
            initial_cash: 10_000,
            initial_prices: [100, 70, 120, 400],
            map_width: 100,
            map_height: 100,
            history_capacity: 16,
            relationship_capacity: 64,
            memory_capacity: 64,
            policy: PolicyKind::RuleBased,
            llm_agents: 4,
            interactions: InteractionSettings::default(),
            organizations: OrganizationSettings::default(),
            scenario: ScenarioConfig::default(),
        }
    }
}

impl SimulationConfig {
    /// Validate bounds before allocation or world mutation.
    pub fn validate(&self) -> Result<()> {
        self.organizations.validate()?;
        self.scenario.validate()?;
        if !(1..=64).contains(&self.interactions.offer_lifetime_ticks)
            || self.interactions.goal_review_interval > 1_000_000
        {
            return Err(DomainError::InvalidConfig(
                "offer_lifetime_ticks must be 1..=64; goal_review_interval must be 0..=1000000"
                    .into(),
            ));
        }
        if self.agents > 100_000 {
            return Err(DomainError::InvalidConfig(
                "agents must be <= 100000".into(),
            ));
        }
        if self.initial_cash > 1_000_000_000 {
            return Err(DomainError::InvalidConfig(
                "initial_cash must be <= 1000000000".into(),
            ));
        }
        if self.initial_prices.iter().any(|&p| p == 0 || p > MAX_PRICE) {
            return Err(DomainError::InvalidConfig(format!(
                "prices must be in 1..={MAX_PRICE}"
            )));
        }
        if self.map_width == 0 || self.map_height == 0 {
            return Err(DomainError::InvalidConfig(
                "city dimensions must be positive".into(),
            ));
        }
        if self.history_capacity > 1024 {
            return Err(DomainError::InvalidConfig(
                "history_capacity must be <= 1024".into(),
            ));
        }
        if self.relationship_capacity > 1024 {
            return Err(DomainError::InvalidConfig(
                "relationship_capacity must be <= 1024".into(),
            ));
        }
        if self.memory_capacity > 1024 {
            return Err(DomainError::InvalidConfig(
                "memory_capacity must be <= 1024".into(),
            ));
        }
        if self.llm_agents > 100_000 {
            return Err(DomainError::InvalidConfig(
                "llm_agents must be <= 100000".into(),
            ));
        }
        Ok(())
    }
}
