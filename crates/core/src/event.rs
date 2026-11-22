use crate::{
    AgentId, ContractEvent, Decision, Goal, GoalAssessment, GoalChangeReason, Money, Occupation,
    OrganizationEvent, Position, Prices, Proposal, ProposalOutcome, Relationship, Resource,
    ScenarioEvent, Tick,
};
use serde::{Deserialize, Serialize};

impl Event {
    /// Canonical affected individuals for query indexes, GUI filtering and personal memories.
    pub fn participants(&self) -> Vec<AgentId> {
        let mut ids = match self {
            Self::Organization { event } => event.participants(),
            Self::Contract { event } => event.participants().to_vec(),
            Self::Scenario { event } => event.participants(),
            Self::TradeExecuted { buyer, seller, .. }
            | Self::NegotiatedTrade { buyer, seller, .. } => vec![*buyer, *seller],
            Self::ProposalOpened { proposal }
            | Self::ProposalCountered { proposal }
            | Self::ProposalClosed { proposal, .. } => vec![proposal.proposer, proposal.recipient],
            Self::CooperationCompleted {
                producer, partner, ..
            } => vec![*producer, *partner],
            Self::RelationshipUpdated {
                agent_id, other, ..
            }
            | Self::RelationshipForgotten { agent_id, other } => vec![*agent_id, *other],
            Self::ExternalDecisionRecorded { result } => vec![result.agent_id],
            Self::AgentCreated { agent_id, .. }
            | Self::GoalChanged { agent_id, .. }
            | Self::DecisionRecorded { agent_id, .. }
            | Self::ActionRejected { agent_id, .. }
            | Self::ResourceProduced { agent_id, .. }
            | Self::ResourceConsumed { agent_id, .. }
            | Self::OrderPlaced { agent_id, .. }
            | Self::OrderExpired { agent_id, .. }
            | Self::AgentMoved { agent_id, .. } => vec![*agent_id],
            Self::TickCompleted { .. } => Vec::new(),
        };
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}
impl OrganizationEvent {
    /// Stable shared-property identity, including a group that later dissolved.
    pub const fn organization(&self) -> crate::OrganizationId {
        match self {
            Self::Founded { organization, .. }
            | Self::Joined { organization, .. }
            | Self::Contributed { organization, .. }
            | Self::Left { organization, .. }
            | Self::Produced { organization, .. }
            | Self::Distributed { organization, .. } => *organization,
        }
    }
    /// People directly involved in this ownership or shared-production fact.
    pub fn participants(&self) -> Vec<AgentId> {
        match self {
            Self::Founded { founder, .. } => vec![*founder],
            Self::Joined { agent_id, .. }
            | Self::Contributed { agent_id, .. }
            | Self::Left { agent_id, .. } => vec![*agent_id],
            Self::Produced { workers, .. } => workers.clone(),
            Self::Distributed { members, .. } => members.clone(),
        }
    }
}
impl ContractEvent {
    /// Stable agreement identity, including a closed agreement.
    pub const fn contract_id(&self) -> u64 {
        match self {
            Self::Offered { contract, .. }
            | Self::Accepted { contract, .. }
            | Self::Closed { contract, .. } => contract.id,
            Self::Delivered { contract_id, .. } => *contract_id,
        }
    }
    /// Legal owners affected by this event, independent of current membership.
    pub const fn parties(&self) -> [crate::Party; 2] {
        match self {
            Self::Offered { contract, .. }
            | Self::Accepted { contract, .. }
            | Self::Closed { contract, .. } => [contract.proposer, contract.recipient],
            Self::Delivered { buyer, seller, .. } => [*buyer, *seller],
        }
    }
    /// All owners involved, including members whose common assets were affected.
    pub fn participants(&self) -> &[AgentId] {
        match self {
            Self::Offered { participants, .. }
            | Self::Accepted { participants, .. }
            | Self::Delivered { participants, .. }
            | Self::Closed { participants, .. } => participants,
        }
    }
}
impl ScenarioEvent {
    /// Actual affected account owners; global announcements have no private participant.
    pub fn participants(&self) -> Vec<AgentId> {
        match self {
            Self::InventoryLost { participants, .. } | Self::TaxCollected { participants, .. } => {
                participants.clone()
            }
            Self::Redistribution { agent_id, .. } => vec![*agent_id],
            Self::Applied { .. } | Self::SupplyRestored { .. } => Vec::new(),
        }
    }
}

/// Purpose of resource destruction, separating factory inputs from household use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsumptionPurpose {
    /// Input to a manufacturing recipe.
    ProductionInput,
    /// Food or comfort consumption.
    Household,
}

/// A limit order's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderSide {
    /// Acquire stock for money.
    Buy,
    /// Supply stock for money.
    Sell,
}

/// Origin of a recorded decision; external inputs never bypass validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOrigin {
    /// Supplied by the policy passed to the engine.
    Policy,
    /// Supplied through the explicit external-decision boundary.
    ExternalInput,
}

/// An immutable auditable fact. It is not an instruction to mutate the world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Shared-property formation, ownership changes and actual collective production.
    Organization {
        /// Canonical collective fact.
        event: OrganizationEvent,
    },
    /// Recurring contract lifecycle and real installment settlements.
    Contract {
        /// Canonical agreement fact.
        event: ContractEvent,
    },
    /// Scheduled interventions, destruction, taxes and funded redistribution.
    Scenario {
        /// Canonical intervention or fiscal fact.
        event: ScenarioEvent,
    },
    /// External request result applied at a fixed tick, before action selection.
    ExternalDecisionRecorded {
        /// Validated decision or explicit rule fallback; no provider credentials.
        result: crate::ExternalDecision,
    },
    /// Initial agent creation.
    AgentCreated {
        /// New agent.
        agent_id: AgentId,
        /// Assigned occupation.
        occupation: Occupation,
        /// Initial objective, recorded so goal memories derive from facts.
        goal: Goal,
    },
    /// Proposed policy output before validation.
    DecisionRecorded {
        /// Acting agent.
        agent_id: AgentId,
        /// Structured policy result.
        decision: Decision,
        /// Whether the decision came from a policy or an explicit external input.
        origin: DecisionOrigin,
    },
    /// An intent was rejected without applying it.
    ActionRejected {
        /// Acting agent.
        agent_id: AgentId,
        /// Explicit validation failure.
        reason: String,
    },
    /// Stock entered an agent inventory through production or extraction.
    ResourceProduced {
        /// Producing agent.
        agent_id: AgentId,
        /// Produced resource.
        resource: Resource,
        /// Quantity added.
        quantity: u64,
    },
    /// Stock left an agent inventory through use, not trade.
    ResourceConsumed {
        /// Consuming agent.
        agent_id: AgentId,
        /// Consumed resource.
        resource: Resource,
        /// Quantity removed.
        quantity: u64,
        /// Why stock was removed.
        purpose: ConsumptionPurpose,
    },
    /// An order and its collateral were accepted.
    OrderPlaced {
        /// Monotonic order identifier, also the within-tick time priority.
        order_id: u64,
        /// Owner.
        agent_id: AgentId,
        /// Bid or ask.
        side: OrderSide,
        /// Market.
        resource: Resource,
        /// Original quantity.
        quantity: u64,
        /// Unit limit price.
        limit_price: Money,
    },
    /// One atomic bilateral settlement.
    TradeExecuted {
        /// Matched bid.
        buy_order_id: u64,
        /// Matched ask.
        sell_order_id: u64,
        /// Buyer.
        buyer: AgentId,
        /// Seller.
        seller: AgentId,
        /// Transferred resource.
        resource: Resource,
        /// Filled quantity.
        quantity: u64,
        /// Maker's unit limit price.
        price: Money,
    },
    /// A named counterparty received a collateralized offer; no trade has happened yet.
    ProposalOpened {
        /// Complete committed offer.
        proposal: Proposal,
    },
    /// Old collateral was released and the recipient committed to revised trade terms.
    ProposalCountered {
        /// Complete replacement, retaining the original deadline and identifier.
        proposal: Proposal,
    },
    /// Proposal left the live book and its collateral was consumed or released.
    ProposalClosed {
        /// Final terms and participants.
        proposal: Proposal,
        /// Reason the proposal ended.
        outcome: ProposalOutcome,
    },
    /// Atomic exchange following explicit acceptance, without synthetic market order IDs.
    NegotiatedTrade {
        /// Proposal whose current terms were accepted.
        proposal_id: u64,
        /// Receiving party.
        buyer: AgentId,
        /// Supplying party.
        seller: AgentId,
        /// Transferred resource.
        resource: Resource,
        /// Actual quantity transferred.
        quantity: u64,
        /// Agreed unit price.
        price: Money,
    },
    /// Joint work completed; ordinary production/input events account for all physical flows.
    CooperationCompleted {
        /// Accepted joint-production proposal.
        proposal_id: u64,
        /// Metal supplier and preparation worker.
        producer: AgentId,
        /// Wood supplier and completion worker.
        partner: AgentId,
        /// Goods actually credited to the producer.
        producer_share: u64,
        /// Goods actually credited to the partner.
        partner_share: u64,
    },
    /// Actual review outcomes changed an objective used by subsequent decisions.
    GoalChanged {
        /// Agent adapting its objective.
        agent_id: AgentId,
        /// Previous objective.
        previous: Goal,
        /// New objective.
        goal: Goal,
        /// Decision rationale.
        reason: GoalChangeReason,
        /// Evidence from the completed review window.
        assessment: GoalAssessment,
    },
    /// Unfilled stock/cash collateral was released at tick end.
    OrderExpired {
        /// Expired order.
        order_id: u64,
        /// Owner.
        agent_id: AgentId,
        /// Unfilled quantity.
        remaining: u64,
    },
    /// Position changed.
    AgentMoved {
        /// Moving agent.
        agent_id: AgentId,
        /// Previous location.
        from: Position,
        /// Destination.
        to: Position,
    },
    /// An interaction changed a directed social edge.
    RelationshipUpdated {
        /// Agent whose perception changed.
        agent_id: AgentId,
        /// Counterparty being evaluated.
        other: AgentId,
        /// Complete new edge state.
        relationship: Relationship,
    },
    /// Least recently used social edge evicted by the configured memory bound.
    RelationshipForgotten {
        /// Agent forgetting an edge.
        agent_id: AgentId,
        /// Forgotten counterparty.
        other: AgentId,
    },
    /// End of a completely settled and audited tick.
    TickCompleted {
        /// Current market reference prices.
        prices: Prices,
        /// Number of bilateral fills in this tick.
        trades: u64,
        /// Total cash after settlement.
        #[serde(deserialize_with = "deserialize_cash_total")]
        total_money: u128,
    },
}

// Internally tagged enum content cannot dispatch deserialize_u128. Valid cash
// supply is bounded by 100,000 agents * 1.2 billion minor units, below u64::MAX.
// Deserialize through that exact integer representation, never through a float.
fn deserialize_cash_total<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<u128, D::Error> {
    u64::deserialize(deserializer).map(u128::from)
}

/// Stable event order, with no wall-clock timestamps in deterministic data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    /// Global monotonic sequence beginning at zero.
    pub sequence: u64,
    /// Tick zero is initialization; the first executed tick is one.
    pub tick: Tick,
    /// Event payload.
    pub event: Event,
}
