use crate::{
    AgentId, ConsumptionPurpose, ContractEvent, Goal, GoalChangeReason, Money, OrderSide,
    OrganizationEvent, Position, ProposalStage, ProposalTerms, Relationship, Resource,
    ScenarioEvent, Tick,
};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Maximum memories selected for a bounded policy observation.
pub const MAX_RECALL_RESULTS: usize = 16;

/// Source event sequence. Together with the owner this uniquely identifies a memory.
///
/// A trade creates one memory for each participant, both referring to the same fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryId(pub u64);

/// Structured experience categories used for selective retrieval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Personal actions, experiences, and failures.
    Episodic,
    /// Changes to perceptions of other agents.
    Social,
    /// Settled transactions with actual prices and quantities.
    Economic,
    /// Established and adaptively revised economic objectives.
    Goal,
}

/// Typed facts instead of unconstrained prose or copies of an entire world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryContent {
    /// An actual shared-property event involving this owner.
    Organization {
        /// Structured charter, transfer or production fact.
        event: OrganizationEvent,
    },
    /// A recurring agreement lifecycle or settlement involving this owner.
    Contract {
        /// Complete bounded event, distinct from an intended contract action.
        event: ContractEvent,
    },
    /// An actual crisis loss or fiscal payment affecting this owner.
    Scenario {
        /// Structured realized intervention effect.
        event: ScenarioEvent,
    },
    /// Initial objective assigned at creation.
    GoalAssigned {
        /// Objective that guides subsequent decisions.
        goal: Goal,
    },
    /// An actual adaptive objective change, not a proposed model action.
    GoalChanged {
        /// Previous objective.
        previous: Goal,
        /// New objective.
        goal: Goal,
        /// Outcome-based rationale.
        reason: GoalChangeReason,
    },
    /// A proposal lifecycle fact, distinguished from a completed trade.
    Proposal {
        /// Source agreement identifier.
        proposal_id: u64,
        /// Other participant.
        other: AgentId,
        /// Terms from the current proposer's perspective.
        terms: ProposalTerms,
        /// Actual lifecycle transition.
        stage: ProposalStage,
    },
    /// Completed joint work and the owner's real share of the output.
    Cooperated {
        /// Accepted production proposal.
        proposal_id: u64,
        /// Other participant.
        other: AgentId,
        /// Goods credited to this owner.
        received_goods: u64,
    },
    /// Production or extraction that actually credited inventory.
    Produced {
        /// Output resource.
        resource: Resource,
        /// Credited quantity.
        quantity: u64,
    },
    /// Actual resource use, distinguishing household use from factory input.
    Consumed {
        /// Resource used.
        resource: Resource,
        /// Quantity removed.
        quantity: u64,
        /// Cause of consumption.
        purpose: ConsumptionPurpose,
    },
    /// An executed move.
    Moved {
        /// Previous position.
        from: Position,
        /// New position.
        to: Position,
    },
    /// A rejected intent; it must never be remembered as a successful action.
    ActionRejected {
        /// Explicit engine validation failure.
        reason: String,
    },
    /// A completed exchange from this owner's perspective.
    Trade {
        /// Whether this owner bought or sold.
        side: OrderSide,
        /// Other participant.
        counterparty: AgentId,
        /// Traded resource.
        resource: Resource,
        /// Filled quantity, not the original order size.
        quantity: u64,
        /// Actual settlement price in minor units.
        price: Money,
    },
    /// Updated perception after an actual interaction.
    RelationshipChanged {
        /// Other agent being evaluated.
        other: AgentId,
        /// New bounded relationship state.
        relationship: Relationship,
    },
    /// A relationship evicted by its retention bound.
    RelationshipForgotten {
        /// Formerly tracked counterparty.
        other: AgentId,
    },
}

impl MemoryContent {
    /// Derive category from the payload, preventing inconsistent duplicate tags.
    pub const fn kind(&self) -> MemoryKind {
        match self {
            Self::GoalAssigned { .. } | Self::GoalChanged { .. } => MemoryKind::Goal,
            Self::Trade { .. } | Self::Contract { .. } | Self::Scenario { .. } => {
                MemoryKind::Economic
            }
            Self::RelationshipChanged { .. }
            | Self::RelationshipForgotten { .. }
            | Self::Proposal { .. }
            | Self::Cooperated { .. }
            | Self::Organization { .. } => MemoryKind::Social,
            _ => MemoryKind::Episodic,
        }
    }

    /// Resource tag used for contextual relevance.
    pub const fn resource(&self) -> Option<Resource> {
        match self {
            Self::Contract {
                event:
                    ContractEvent::Offered { contract, .. }
                    | ContractEvent::Accepted { contract, .. }
                    | ContractEvent::Closed { contract, .. },
            } => Some(contract.resource),
            Self::Contract {
                event: ContractEvent::Delivered { resource, .. },
            }
            | Self::Scenario {
                event: ScenarioEvent::InventoryLost { resource, .. },
            } => Some(*resource),
            Self::Organization {
                event: OrganizationEvent::Produced { .. } | OrganizationEvent::Distributed { .. },
            } => Some(Resource::Goods),
            Self::Produced { resource, .. }
            | Self::Consumed { resource, .. }
            | Self::Trade { resource, .. } => Some(*resource),
            Self::Proposal { terms, .. } => Some(terms.resource()),
            Self::Cooperated { .. } => Some(Resource::Goods),
            _ => None,
        }
    }

    /// Related agent, if any; no free-text parsing is needed to find social context.
    pub const fn related_agent(&self) -> Option<AgentId> {
        match self {
            Self::Trade { counterparty, .. } => Some(*counterparty),
            Self::RelationshipChanged { other, .. }
            | Self::RelationshipForgotten { other }
            | Self::Proposal { other, .. }
            | Self::Cooperated { other, .. } => Some(*other),
            _ => None,
        }
    }
}

impl fmt::Display for MemoryContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Organization { event } => write!(f, "Cooperative: {event:?}"),
            Self::Contract { event } => write!(f, "Agreement: {event:?}"),
            Self::Scenario { event } => write!(f, "Intervention: {event:?}"),
            Self::GoalAssigned { goal } => write!(f, "Goal assigned: {goal:?}"),
            Self::GoalChanged {
                previous,
                goal,
                reason,
            } => {
                write!(f, "Goal {previous:?} -> {goal:?}: {reason:?}")
            }
            Self::Proposal {
                proposal_id,
                other,
                terms,
                stage,
            } => {
                write!(
                    f,
                    "Proposal {proposal_id} {stage:?} with Agent {other}: {terms:?}"
                )
            }
            Self::Cooperated {
                proposal_id,
                other,
                received_goods,
            } => {
                write!(
                    f,
                    "Joint batch {proposal_id} with Agent {other}: received {received_goods} goods"
                )
            }
            Self::Produced { resource, quantity } => write!(f, "Produced {quantity} {resource}"),
            Self::Consumed {
                resource,
                quantity,
                purpose,
            } => write!(f, "Used {quantity} {resource} ({purpose:?})"),
            Self::Moved { from, to } => {
                write!(f, "Moved ({}, {}) -> ({}, {})", from.x, from.y, to.x, to.y)
            }
            Self::ActionRejected { reason } => write!(f, "Action rejected: {reason}"),
            Self::Trade {
                side,
                counterparty,
                resource,
                quantity,
                price,
            } => write!(
                f,
                "{side:?} {quantity} {resource} @ {price} with Agent {counterparty} (value {})",
                u128::from(*quantity) * u128::from(*price)
            ),
            Self::RelationshipChanged {
                other,
                relationship,
            } => write!(
                f,
                "Agent {other}: trust {}, familiarity {}, interactions {}",
                relationship.trust, relationship.familiarity, relationship.interaction_count
            ),
            Self::RelationshipForgotten { other } => write!(f, "Stopped tracking Agent {other}"),
        }
    }
}

/// One bounded, traceable experience owned by an individual agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecord {
    /// Event sequence that produced this memory.
    pub id: MemoryId,
    /// Owner; `(agent_id, id)` is the unique memory key.
    pub agent_id: AgentId,
    /// Simulation tick, never wall-clock time.
    pub tick: Tick,
    /// Retrieval importance in 0..=100.
    pub importance: u8,
    /// Typed content with category, resource, and social tags.
    pub content: MemoryContent,
}
