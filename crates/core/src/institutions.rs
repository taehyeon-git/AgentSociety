//! Public charter, shared-property and recurring-contract domain records.
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Maximum concurrent incoming/outgoing agreements per legal owner.
pub const MAX_CONTRACTS: usize = 4;
/// Explicit charter and agreement operations, still limited to one main action per tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InstitutionAction {
    /// Create a cooperative and pay the fixed membership stake.
    Found {},
    /// Accept open membership, equal ownership and recurring labor obligations.
    Join {
        /// Cooperative to join.
        organization: OrganizationId,
    },
    /// Return the departing member's equal share; all group contracts must be closed.
    Leave {
        /// Current cooperative.
        organization: OrganizationId,
    },
    /// Transfer free assets to common property with no privileged withdrawal claim.
    Contribute {
        /// Current cooperative.
        organization: OrganizationId,
        /// Voluntary cash contribution.
        cash: Money,
        /// Voluntary resource contribution.
        inventory: Inventory,
    },
    /// Leader pays the same dividend to every owner, using uncommitted property only.
    Distribute {
        /// Managed cooperative.
        organization: OrganizationId,
        /// Amount paid to each member.
        cash_per_member: Money,
        /// Goods paid to each member.
        goods_per_member: u64,
    },
    /// Promise a recurring fixed-price exchange with explicit funded obligations.
    OfferContract {
        /// Group represented by its leader, or None for the individual.
        organization: Option<OrganizationId>,
        /// Other legal owner.
        counterparty: Party,
        /// Proposer perspective.
        side: OrderSide,
        /// Recurring shipment resource.
        resource: Resource,
        /// Units per installment.
        quantity: u64,
        /// Agreed unit price.
        unit_price: Money,
        /// Number of installments, 2..=32.
        deliveries: u8,
        /// Ticks between installments, 1..=64.
        interval_ticks: Tick,
    },
    /// Recipient commits funding or the seller bond.
    AcceptContract {
        /// Pending agreement.
        contract_id: u64,
    },
    /// Recipient declines a pending agreement.
    RejectContract {
        /// Pending agreement.
        contract_id: u64,
    },
    /// Proposer withdraws a pending offer; active commitments cannot be cancelled unilaterally.
    CancelContract {
        /// Pending agreement.
        contract_id: u64,
    },
}
/// Stable cooperative identifier, separate from individual identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OrganizationId(pub u64);

/// An individual or a member-owned organization may own a contract's assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Party {
    /// Individual account.
    Agent(AgentId),
    /// Cooperative treasury and inventory.
    Organization(OrganizationId),
}

/// Optional cooperative economy. Joining accepts equal ownership and recurring labor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OrganizationSettings {
    /// Enable formation, membership, contributions, dividends and contracts.
    pub enabled: bool,
    /// Maximum members per cooperative; at least three are needed for production.
    pub max_members: usize,
    /// Cash paid into the shared account by each founding or joining member.
    pub membership_stake: Money,
    /// Period between automatic collective production opportunities.
    pub production_interval: Tick,
}
impl Default for OrganizationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            max_members: 8,
            membership_stake: 200,
            production_interval: 4,
        }
    }
}
impl OrganizationSettings {
    /// Validate charter bounds before creating a run.
    pub fn validate(&self) -> Result<()> {
        if !(3..=16).contains(&self.max_members)
            || self.membership_stake == 0
            || self.membership_stake > 1_000_000
            || !(1..=64).contains(&self.production_interval)
        {
            return Err(DomainError::InvalidConfig("organization members 3..=16, stake 1..=1000000, production interval 1..=64 required".into()));
        }
        Ok(())
    }
}

/// Equal-share property, managed by one member under a fixed open charter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Organization {
    /// Monotonic identity, never reused.
    pub id: OrganizationId,
    /// Member authorized to propose or accept agreements for the group.
    pub leader: AgentId,
    /// Equal owners in canonical order; each person belongs to at most one group.
    pub members: BTreeSet<AgentId>,
    /// Shared cash, inventory and persistent contract collateral.
    pub account: Account,
    /// Completed tick in which the group was founded.
    pub founded_at: Tick,
    /// Last successful collective production tick, or zero.
    pub last_production: Tick,
}

/// A fixed-price recurring shipment, funded by the buyer and bonded by the seller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryContract {
    /// Monotonic agreement identifier.
    pub id: u64,
    /// Party making the offer.
    pub proposer: Party,
    /// Party authorized to accept or reject.
    pub recipient: Party,
    /// Direction from the proposer's perspective.
    pub side: OrderSide,
    /// Resource delivered in each installment.
    pub resource: Resource,
    /// Quantity per shipment.
    pub quantity: u64,
    /// Fixed price per unit.
    pub unit_price: Money,
    /// Total installments, in 2..=32.
    pub deliveries: u8,
    /// Shipments still owed; unchanged while the offer is pending.
    pub remaining: u8,
    /// Gap between shipments, in 1..=64 ticks.
    pub interval_ticks: Tick,
    /// Original offer tick.
    pub created_at: Tick,
    /// Last acceptable response tick, inclusive.
    pub expires_at: Tick,
    /// Acceptance tick; absent until explicitly accepted.
    pub accepted_at: Option<Tick>,
    /// First unfulfilled deadline; absent for pending offers.
    pub next_delivery: Option<Tick>,
    /// Seller's fixed cash bond, returned on completion or paid to the buyer on default.
    pub bond: Money,
}
impl DeliveryContract {
    /// Customer account.
    pub const fn buyer(&self) -> Party {
        match self.side {
            OrderSide::Buy => self.proposer,
            OrderSide::Sell => self.recipient,
        }
    }
    /// Supplier account.
    pub const fn seller(&self) -> Party {
        match self.side {
            OrderSide::Buy => self.recipient,
            OrderSide::Sell => self.proposer,
        }
    }
    /// Unpaid total of the remaining funded shipments.
    pub fn outstanding(&self) -> Result<Money> {
        notional(self.unit_price, self.quantity)?
            .checked_mul(u64::from(self.remaining))
            .ok_or(DomainError::Overflow("contract commitment"))
    }
}

/// Facts about shared ownership; asset movements are not production or market turnover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrganizationEvent {
    /// A founder put the mandatory stake into a new cooperative.
    Founded {
        /// New organization.
        organization: OrganizationId,
        /// Founding member.
        founder: AgentId,
        /// Actual paid-in capital.
        stake: Money,
    },
    /// Another person accepted the charter and funded membership.
    Joined {
        /// Organization.
        organization: OrganizationId,
        /// New owner.
        agent_id: AgentId,
        /// Paid-in capital.
        stake: Money,
    },
    /// A member transferred owned resources or money into common property.
    Contributed {
        /// Organization.
        organization: OrganizationId,
        /// Contributor.
        agent_id: AgentId,
        /// Actual cash movement.
        cash: Money,
        /// Actual stock movement.
        inventory: Inventory,
    },
    /// Departure returns a proportional share after all agreements have closed.
    Left {
        /// Organization.
        organization: OrganizationId,
        /// Departing person.
        agent_id: AgentId,
        /// Returned cash.
        cash: Money,
        /// Returned stock.
        inventory: Inventory,
        /// Successor; absent when dissolved.
        leader: Option<AgentId>,
    },
    /// Three members completed a paid-in-kind batch using common inputs.
    Produced {
        /// Organization.
        organization: OrganizationId,
        /// Workers, including a producer.
        workers: Vec<AgentId>,
        /// Actual inputs consumed from shared stock.
        inputs: Inventory,
        /// Goods credited to shared stock.
        goods: u64,
    },
    /// Equal distributions moved common cash and goods to the owners.
    Distributed {
        /// Organization.
        organization: OrganizationId,
        /// Recipients at the time of payment.
        members: Vec<AgentId>,
        /// Cash to each recipient.
        cash_per_member: Money,
        /// Goods to each recipient.
        goods_per_member: u64,
    },
}

/// A recurring agreement closes for an explicit, auditable reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractOutcome {
    /// All installments were delivered.
    Completed,
    /// The seller missed a due shipment and forfeited the bond.
    Defaulted,
    /// Recipient declined the pending offer.
    Rejected,
    /// Proposer withdrew the pending offer.
    Cancelled,
    /// No acceptance arrived before the deadline.
    Expired,
}

/// Historical contract facts preserve the affected people even after group membership changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContractEvent {
    /// Proposal posted with the proposer's side of cash collateral.
    Offered {
        /// Pending offer.
        contract: DeliveryContract,
        /// All represented individuals.
        participants: Vec<AgentId>,
    },
    /// Both sides committed and the shipment schedule began.
    Accepted {
        /// Accepted state.
        contract: DeliveryContract,
        /// All represented individuals.
        participants: Vec<AgentId>,
    },
    /// A real atomic cash-for-stock exchange at a scheduled deadline.
    Delivered {
        /// Agreement.
        contract_id: u64,
        /// Customer.
        buyer: Party,
        /// Supplier.
        seller: Party,
        /// Customer/supplier representatives for social feedback.
        representatives: [AgentId; 2],
        /// All current owners affected by the exchange.
        participants: Vec<AgentId>,
        /// Resource.
        resource: Resource,
        /// Filled units.
        quantity: u64,
        /// Actual unit price.
        price: Money,
    },
    /// Residual buyer funding and seller bond were released or paid as compensation.
    Closed {
        /// Final contract state.
        contract: DeliveryContract,
        /// End reason.
        outcome: ContractOutcome,
        /// Actual bond paid to the buyer, zero except default.
        compensation: Money,
        /// Customer/supplier representatives.
        representatives: [AgentId; 2],
        /// Affected owners.
        participants: Vec<AgentId>,
    },
}

/// Public cooperative view; private member memory is never shared.
#[derive(Debug, Clone, Serialize)]
pub struct OrganizationView {
    /// Identity.
    pub id: OrganizationId,
    /// Authorized representative.
    pub leader: AgentId,
    /// Equal owners.
    pub members: Vec<AgentId>,
    /// Cash free of commitments.
    pub available_cash: Money,
    /// Unreserved shared stock.
    pub available_inventory: Inventory,
    /// Number of live agreements, including pending offers.
    pub contracts: usize,
}

/// Bounded context used by both local and external decision policies.
#[derive(Debug, Clone, Serialize)]
pub struct InstitutionObservation {
    /// Current charter settings.
    pub settings: OrganizationSettings,
    /// Actor's membership, if any.
    pub own: Option<OrganizationView>,
    /// At most eight public organizations.
    pub organizations: Vec<OrganizationView>,
    /// At most eight public individual counterparties.
    pub counterparties: Vec<ContractCounterpartyView>,
    /// Actor's and, for leaders, their organization's agreements (at most eight).
    pub contracts: Vec<DeliveryContract>,
}

/// Public stock and commitment capacity of a potential individual contract partner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractCounterpartyView {
    /// Individual identity.
    pub id: AgentId,
    /// Current productive specialization.
    pub occupation: Occupation,
    /// Unpledged stock available at the observation boundary.
    pub available_inventory: Inventory,
    /// Cash available after all reservations.
    pub available_cash: Money,
    /// Public ability to contribute labor.
    pub energy: u8,
    /// Public household buffer preference.
    pub food_reserve: u64,
    /// Pending and accepted recurring agreements, not bilateral spot proposals.
    pub open_contracts: usize,
}
