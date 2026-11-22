//! Bounded bilateral proposals and evidence for adaptive economic objectives.
use crate::{AgentId, Goal, Inventory, Money, Occupation, OrderSide, Resource, Tick};
use serde::{Deserialize, Serialize};

/// Active proposals involving one agent, counting both senders and recipients.
pub const MAX_OPEN_PROPOSALS: usize = 4;
/// A negotiation cannot be prolonged indefinitely by counteroffers.
pub const MAX_COUNTEROFFERS: u8 = 3;
/// Maximum public counterparties in one policy observation.
pub const MAX_OBSERVED_PEERS: usize = 8;
/// A joint batch consumes four wood from the partner.
pub const COOPERATIVE_WOOD: u64 = 4;
/// A joint batch consumes two metal from the producer.
pub const COOPERATIVE_METAL: u64 = 2;
/// Joint work produces four goods, divided by the agreed shares.
pub const COOPERATIVE_GOODS: u64 = 4;
/// Each participant contributes five energy; proposal preparation is paid upfront.
pub const COOPERATIVE_LABOR: u8 = 5;

/// Optional economic interactions; defaults retain the basic market experiment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InteractionSettings {
    /// Enable directed price negotiation and reserved bilateral offers.
    pub negotiation: bool,
    /// Enable joint manufacturing proposals and shared output.
    pub cooperation: bool,
    /// An offer remains actionable through creation tick plus this lifetime.
    pub offer_lifetime_ticks: Tick,
    /// Periodic goal review interval; zero disables automatic adaptation.
    pub goal_review_interval: Tick,
}

impl Default for InteractionSettings {
    fn default() -> Self {
        Self {
            negotiation: false,
            cooperation: false,
            offer_lifetime_ticks: 4,
            goal_review_interval: 0,
        }
    }
}

/// A binding offer from the current proposer; the recipient has not yet agreed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProposalTerms {
    /// Seller stock or buyer cash is reserved by the current proposer.
    Trade {
        /// Side of the current proposer, reversed by a counteroffer.
        side: OrderSide,
        /// Resource exchanged.
        resource: Resource,
        /// All-or-nothing quantity.
        quantity: u64,
        /// Agreed unit price if accepted.
        unit_price: Money,
    },
    /// Producer commits metal and preparation labor; the partner supplies wood and labor.
    Cooperation {
        /// Producer's goods, from one through three; the partner receives the rest.
        producer_share: u64,
    },
}

impl ProposalTerms {
    /// Resource used for display and memory retrieval.
    pub const fn resource(&self) -> Resource {
        match self {
            Self::Trade { resource, .. } => *resource,
            Self::Cooperation { .. } => Resource::Goods,
        }
    }
}

/// A live proposal, serialized independently of ECS identity and indexes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    /// Monotonic identifier shared by trade and cooperation proposals.
    pub id: u64,
    /// Party currently committed to the terms.
    pub proposer: AgentId,
    /// Only this agent may accept, reject, or counter the current terms.
    pub recipient: AgentId,
    /// Current terms, including whose side of a trade is reserved.
    pub terms: ProposalTerms,
    /// Original proposal tick, unchanged by counteroffers.
    pub created_at: Tick,
    /// Most recent proposal or counteroffer tick.
    pub updated_at: Tick,
    /// Inclusive last tick at which a response can execute.
    pub expires_at: Tick,
    /// Number of counteroffers already made.
    pub counteroffers: u8,
}

/// Why a proposal left the live book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalOutcome {
    /// Recipient agreed and settlement or joint production completed.
    Accepted,
    /// Recipient explicitly declined.
    Rejected,
    /// Current proposer withdrew the offer.
    Cancelled,
    /// Inclusive response deadline passed.
    Expired,
}

/// Lifecycle fact stored in an agent's bounded memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStage {
    /// New offer, without a completed exchange.
    Opened,
    /// Recipient replaced the terms, without a completed exchange.
    Countered,
    /// Completed agreement.
    Accepted,
    /// Explicitly declined.
    Rejected,
    /// Withdrawn by its proposer.
    Cancelled,
    /// Expired without agreement.
    Expired,
}

impl From<ProposalOutcome> for ProposalStage {
    fn from(outcome: ProposalOutcome) -> Self {
        match outcome {
            ProposalOutcome::Accepted => Self::Accepted,
            ProposalOutcome::Rejected => Self::Rejected,
            ProposalOutcome::Cancelled => Self::Cancelled,
            ProposalOutcome::Expired => Self::Expired,
        }
    }
}

/// Small public economic view; policies do not receive another agent's full memory.
#[derive(Debug, Clone, Serialize)]
pub struct CounterpartyView {
    /// Stable identity.
    pub id: AgentId,
    /// Productive role.
    pub occupation: Occupation,
    /// Unreserved stocks only.
    pub available_inventory: Inventory,
    /// Unreserved wallet only.
    pub available_cash: Money,
    /// Energy available for cooperative labor.
    pub energy: u8,
    /// Goal-derived food reserve.
    pub food_reserve: u64,
    /// Number of active incoming and outgoing proposals.
    pub open_proposals: usize,
}

/// Bounded information shared by rule and model policies at the same tick boundary.
#[derive(Debug, Clone, Serialize)]
pub struct InteractionObservation {
    /// Enabled capabilities and deadlines.
    pub settings: InteractionSettings,
    /// At most four currently actionable proposals involving the actor.
    pub proposals: Vec<Proposal>,
    /// At most eight counterparties, ordered deterministically.
    pub counterparties: Vec<CounterpartyView>,
}

/// Real outcomes collected since the last objective review, independent of memory retention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalProgress {
    /// Most recent periodic review, or zero before the first review.
    pub last_review: Tick,
    /// Most recent actual objective change, or zero for the original objective.
    pub last_change: Tick,
    /// Wealth at the previous review, always valued using the initial reference prices.
    pub reference_wealth: u128,
    /// Completed ticks with nonzero hunger in this review window.
    pub hungry_ticks: u64,
    /// Rejected main actions in this review window.
    pub rejected_actions: u64,
    /// Completed exchanges involving this agent in this review window.
    pub successful_trades: u64,
    /// Completed joint batches involving this agent in this review window.
    pub cooperations: u64,
}

impl GoalProgress {
    /// Initialize performance tracking without adding random draws.
    pub const fn new(reference_wealth: u128) -> Self {
        Self {
            last_review: 0,
            last_change: 0,
            reference_wealth,
            hungry_ticks: 0,
            rejected_actions: 0,
            successful_trades: 0,
            cooperations: 0,
        }
    }
}

/// Bounded, auditable evidence attached to a goal-change event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalAssessment {
    /// Number of completed ticks considered.
    pub elapsed_ticks: Tick,
    /// Ticks with hunger.
    pub hungry_ticks: u64,
    /// Rejected main intents.
    pub rejected_actions: u64,
    /// Actual completed exchanges.
    pub successful_trades: u64,
    /// Actual completed joint batches.
    pub cooperations: u64,
    /// Reference-price wealth declined by more than fifteen percent.
    pub wealth_declined: bool,
}

/// Machine-readable motivation for an adaptive economic objective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalChangeReason {
    /// Repeated hunger or an unsafe food buffer.
    FoodInsecurity,
    /// A material decline in reference-price wealth.
    WealthDecline,
    /// Repeated trading success supports the merchant strategy.
    TradingSuccess,
    /// Sustained food security permits attention to comfort and cooperation.
    StableHousehold,
    /// Survival has recovered sufficiently to rebuild reserves.
    Recovery,
}

impl Goal {
    /// Base food buffer before personality adjustment.
    pub const fn food_reserve(self) -> u64 {
        match self {
            Self::BuildWealth => 6,
            Self::SecureFood => 14,
            Self::StableLife => 10,
            Self::TradeProfit => 8,
        }
    }
}
