use crate::{Account, GoalProgress, MemoryRecord, Money, OrderSide, Resource, Tick};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
};

/// Stable identifier independent of ECS entity allocation and iteration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(pub u64);

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Primary economic role; all roles may gather and grow subsistence food.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Occupation {
    /// Efficient food producer.
    Farmer,
    /// Turns wood and metal into goods.
    Producer,
    /// Buys and resells stock with a spread.
    Merchant,
    /// Funds household consumption by supplying raw materials.
    Consumer,
}

/// A goal changes reserve, spending, and quotation decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Goal {
    /// Prefer savings over comfort and inventory buffers.
    BuildWealth,
    /// Hold a larger food buffer.
    SecureFood,
    /// Maintain comfort and larger household stocks.
    StableLife,
    /// Prefer a larger trading margin and capital for resale.
    TradeProfit,
}

/// Bounded individual preferences (0 through 100).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Personality {
    /// Higher risk tolerance permits more aggressive bids.
    pub risk_tolerance: u8,
    /// Higher patience raises sale reservation prices and inventory buffers.
    pub patience: u8,
}

/// Job-specific productivity (0 through 100).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skills {
    /// Agricultural productivity.
    pub farming: u8,
    /// Factory productivity.
    pub manufacturing: u8,
    /// Forestry and mining productivity.
    pub gathering: u8,
    /// Merchant spread efficiency.
    pub trading: u8,
}

/// Bounded needs; high hunger is bad, high energy and comfort are good.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Needs {
    /// Hunger in 0..=100. The MVP has no mortality.
    pub hunger: u8,
    /// Energy in 0..=100.
    pub energy: u8,
    /// Household comfort in 0..=100.
    pub comfort: u8,
}

/// Integer city coordinates. The MVP uses a city-wide market.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// Horizontal coordinate.
    pub x: u16,
    /// Vertical coordinate.
    pub y: u16,
}

/// Implemented policy identifiers. Unsupported providers are not silently enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyKind {
    /// Deterministic, offline rule-based decisions.
    RuleBased,
    /// Bounded external decisions, with offline rules for skipped or failed calls.
    Llm,
    /// Offline rules for routine needs; external decisions for stalled strategies.
    Hybrid,
}

impl std::str::FromStr for PolicyKind {
    type Err = crate::DomainError;
    fn from_str(value: &str) -> crate::Result<Self> {
        match value {
            "rule-based" | "rule_based" => Ok(Self::RuleBased),
            "llm" => Ok(Self::Llm),
            "hybrid" => Ok(Self::Hybrid),
            _ => Err(crate::DomainError::InvalidConfig(
                "policy must be rule-based, llm, or hybrid".into(),
            )),
        }
    }
}

/// A declarative intent. The engine validates it against the current state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Manage shared property or recurring delivery obligations.
    Institution {
        /// Explicit cooperative or contract operation.
        operation: crate::InstitutionAction,
    },
    /// Produce one recipe batch (food or manufactured goods).
    Produce {
        /// Desired output resource.
        resource: Resource,
    },
    /// Extract one batch of wood or metal from regional capacity.
    Gather {
        /// Raw resource to extract.
        resource: Resource,
    },
    /// Consume additional food or goods, beyond automatic household use.
    Consume {
        /// Consumable resource.
        resource: Resource,
        /// Positive quantity.
        quantity: u64,
    },
    /// Submit a limit bid valid for the current tick.
    Buy {
        /// Resource to acquire.
        resource: Resource,
        /// Positive quantity.
        quantity: u64,
        /// Maximum unit price in minor units.
        limit_price: Money,
    },
    /// Submit a limit ask valid for the current tick.
    Sell {
        /// Resource to sell.
        resource: Resource,
        /// Positive quantity.
        quantity: u64,
        /// Minimum unit price in minor units.
        limit_price: Money,
    },
    /// Move within the city; travel costs one energy unit.
    Move {
        /// Destination inside city bounds.
        destination: Position,
    },
    /// Offer a directed trade, reserving the proposer's money or stock.
    Negotiate {
        /// Intended other participant.
        counterparty: AgentId,
        /// Whether this proposer buys or sells.
        side: OrderSide,
        /// Resource exchanged.
        resource: Resource,
        /// All-or-nothing quantity.
        quantity: u64,
        /// Proposed unit price.
        unit_price: Money,
    },
    /// Replace a received trade offer, releasing its old collateral and reserving yours.
    CounterOffer {
        /// Existing incoming trade proposal.
        proposal_id: u64,
        /// New all-or-nothing quantity.
        quantity: u64,
        /// New unit price; the original expiry is retained.
        unit_price: Money,
    },
    /// Accept a received trade or joint-production proposal atomically.
    AcceptProposal {
        /// Existing incoming proposal.
        proposal_id: u64,
    },
    /// Decline a received proposal, releasing the proposer's collateral.
    RejectProposal {
        /// Existing incoming proposal.
        proposal_id: u64,
    },
    /// Withdraw your currently outstanding proposal.
    CancelProposal {
        /// Existing outgoing proposal.
        proposal_id: u64,
    },
    /// A producer reserves metal and spends preparation labor for a joint batch.
    Cooperate {
        /// Agent invited to contribute wood and labor.
        partner: AgentId,
        /// Producer's share of four goods; must be one through three.
        producer_share: u64,
    },
    /// Recover energy.
    Rest {},
    /// Take no deliberate action.
    Wait {},
}

/// Machine-readable explanation for a policy decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionReason {
    /// Validated external model choice; its short summary is recorded separately.
    ExternalModel,
    /// Energy is too low for productive work.
    RecoverEnergy,
    /// Food buffer is below the goal-specific target.
    SecureFood,
    /// Hunger requires direct relief.
    RelieveHunger,
    /// Sell surplus to finance other activity.
    SellSurplus,
    /// Maintain desired comfort.
    ImproveComfort,
    /// Run the occupation's production process.
    ProduceOutput,
    /// Acquire factory inputs.
    AcquireInputs,
    /// Buy inventory for resale.
    TradeOpportunity,
    /// Extract raw materials for income or own production.
    EarnIncome,
    /// Grow a small food batch when the market is unaffordable.
    Subsistence,
    /// No feasible useful action is available.
    NoOpportunity,
    /// Negotiate a specific exchange with a known counterparty.
    NegotiateTerms,
    /// Respond to an incoming trade or production proposal.
    RespondToProposal,
    /// Share the inputs and output of a joint manufacturing batch.
    JointProduction,
    /// Join or manage a cooperative, including contribution and equal dividends.
    CollectiveOrganization,
    /// Propose or respond to funded recurring supply agreements.
    RecurringAgreement,
}

impl Action {
    /// Whether this intent needs the persistent proposal book in addition to the market.
    pub const fn is_interaction(&self) -> bool {
        matches!(
            self,
            Self::Negotiate { .. }
                | Self::CounterOffer { .. }
                | Self::AcceptProposal { .. }
                | Self::RejectProposal { .. }
                | Self::CancelProposal { .. }
                | Self::Cooperate { .. }
        )
    }
}

/// The policy's intent and bounded explanation; it has no mutation authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// Proposed action.
    pub action: Action,
    /// Reason suitable for logs and a future inspector.
    pub reason: DecisionReason,
}

/// Bounded recent action history, distinct from searchable experience memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRecord {
    /// Tick in which the intent was submitted.
    pub tick: Tick,
    /// Decision as originally proposed.
    pub decision: Decision,
    /// Whether the intent was accepted (an accepted order may remain unfilled).
    pub accepted: bool,
    /// Rejection reason, when applicable.
    pub rejection: Option<String>,
}

/// A directed, bounded relationship formed by actual interactions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relationship {
    /// Trust in -100..=100; successful exchange raises it.
    pub trust: i16,
    /// Familiarity in 0..=100.
    pub familiarity: u16,
    /// Cooperative behavior in -100..=100.
    pub cooperation_score: i16,
    /// Number of observed interactions (saturates at u64::MAX).
    pub interaction_count: u64,
    /// Most recent interaction tick, used for deterministic eviction.
    pub last_interaction: Tick,
}

/// Complete agent state, independent of a UI or LLM provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    /// Stable identity.
    pub id: AgentId,
    /// Human-readable name.
    pub name: String,
    /// Initial age in years (ticks do not represent years).
    pub age: u16,
    /// Primary productive role.
    pub occupation: Occupation,
    /// Individual preferences.
    pub personality: Personality,
    /// Current economic objective.
    pub goal: Goal,
    /// Evidence and review positions for optional adaptive objectives.
    pub goal_progress: GoalProgress,
    /// Household needs and labor capacity.
    pub needs: Needs,
    /// Wallet, stock, reservations, and cumulative receipts/expenditure.
    pub account: Account,
    /// Productive abilities.
    pub skills: Skills,
    /// Position in the city.
    pub position: Position,
    /// Selected policy implementation.
    pub policy: PolicyKind,
    /// Most recently selected action.
    pub current_action: Action,
    /// Last purchase unit price, used by merchant resale decisions.
    pub last_purchase_prices: [Money; 4],
    /// Recent decisions with a configurable bounded capacity.
    pub history: VecDeque<ActionRecord>,
    /// Directed social edges, keyed by stable counterparty identity.
    pub relationships: BTreeMap<AgentId, Relationship>,
    /// Bounded experiences derived from recorded facts, oldest first.
    pub memories: VecDeque<MemoryRecord>,
    /// Latest external model outcome, including explicit fallback reasons.
    pub last_external_decision: Option<crate::ExternalDecision>,
}

impl Agent {
    /// Goal-specific food reserve, adjusted by individual patience.
    pub fn food_reserve(&self) -> u64 {
        self.goal.food_reserve() + u64::from(self.personality.patience) / 30
    }
}
