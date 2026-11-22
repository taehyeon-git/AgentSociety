//! Time-stamped environmental and fiscal interventions with no unbacked currency issuance.
use crate::*;
use serde::{Deserialize, Serialize};

/// Fiscal rules; sales are charged each tick, cash levies and redistribution periodically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FiscalPolicy {
    /// Sales receipts tax in basis points (10000 = 100%, maximum 5000).
    pub sales_tax_bps: u16,
    /// Periodic levy on unreserved cash, maximum 1000 basis points.
    pub cash_levy_bps: u16,
    /// Share of the government treasury paid equally to citizens each period.
    pub redistribution_bps: u16,
    /// Positive period, 1..=1000000 ticks.
    pub period: Tick,
}
impl Default for FiscalPolicy {
    fn default() -> Self {
        Self {
            sales_tax_bps: 0,
            cash_levy_bps: 0,
            redistribution_bps: 0,
            period: 10,
        }
    }
}
impl FiscalPolicy {
    /// Reject invalid percentages or a zero collection period.
    pub fn validate(&self) -> Result<()> {
        if self.sales_tax_bps > 5000
            || self.cash_levy_bps > 1000
            || self.redistribution_bps > 10000
            || !(1..=1_000_000).contains(&self.period)
        {
            return Err(DomainError::InvalidConfig(
                "invalid fiscal rates or period".into(),
            ));
        }
        Ok(())
    }
}

/// A real intervention affects supply, unpledged inventories or fiscal transfers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Intervention {
    /// Temporarily scale environmental replenishment; prices still emerge from actual trades.
    Supply {
        /// Renewable production opportunity.
        resource: Resource,
        /// Regeneration multiplier, 0..=20000 basis points.
        regeneration_bps: u16,
        /// Positive duration in ticks.
        duration: Tick,
    },
    /// Destroy a fraction of unreserved private and common stock, with explicit loss accounting.
    InventoryLoss {
        /// Damaged stock.
        resource: Resource,
        /// Fraction destroyed in basis points.
        fraction_bps: u16,
    },
    /// Replace fiscal rules until another recorded policy change.
    Fiscal {
        /// New policy.
        policy: FiscalPolicy,
    },
}

/// Stable order within the schedule is the intervention identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledIntervention {
    /// First affected tick; initialization remains tick zero.
    pub tick: Tick,
    /// Human-readable bounded description.
    pub label: String,
    /// Effect to apply exactly once.
    pub intervention: Intervention,
}

/// Configuration carried by checkpoints and compared as an experiment axis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScenarioConfig {
    /// Safe report identifier, up to 64 ASCII letters/digits/dashes/underscores.
    pub name: String,
    /// At most 256 interventions, nondecreasing tick order and at most four per tick.
    pub events: Vec<ScheduledIntervention>,
}
impl Default for ScenarioConfig {
    fn default() -> Self {
        Self {
            name: "baseline".into(),
            events: Vec::new(),
        }
    }
}
impl ScenarioConfig {
    /// Validate ordering, limits and nonoverlapping supply changes for each resource.
    pub fn validate(&self) -> Result<()> {
        let invalid = || {
            DomainError::InvalidConfig(
                "invalid scenario name, schedule, bounds or overlapping supply shocks".into(),
            )
        };
        if self.name.is_empty()
            || self.name.len() > 64
            || !self
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || self.events.len() > 256
        {
            return Err(invalid());
        }
        let mut previous_tick = 0;
        let mut same_tick = 0;
        let mut supply_until = [0; 4];
        for event in &self.events {
            if event.tick == 0
                || event.tick < previous_tick
                || event.label.is_empty()
                || event.label.chars().count() > 128
            {
                return Err(invalid());
            }
            same_tick = if previous_tick == event.tick {
                same_tick + 1
            } else {
                1
            };
            if same_tick > 4 {
                return Err(invalid());
            }
            previous_tick = event.tick;
            match &event.intervention {
                Intervention::Supply {
                    resource,
                    regeneration_bps,
                    duration,
                } => {
                    if *resource == Resource::Goods
                        || *regeneration_bps > 20000
                        || !(1..=1_000_000).contains(duration)
                        || supply_until[resource.index()] > event.tick
                    {
                        return Err(invalid());
                    }
                    supply_until[resource.index()] =
                        event.tick.checked_add(*duration).ok_or_else(invalid)?;
                }
                Intervention::InventoryLoss { fraction_bps, .. }
                    if *fraction_bps == 0 || *fraction_bps > 10000 =>
                {
                    return Err(invalid());
                }
                Intervention::Fiscal { policy } => policy.validate()?,
                _ => {}
            }
        }
        Ok(())
    }
}

/// A temporary supply factor at a completed tick boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupplyEffect {
    /// Resource affected.
    pub resource: Resource,
    /// Replacement regeneration factor.
    pub regeneration_bps: u16,
    /// First tick on which normal replenishment resumes.
    pub ends_at: Tick,
}

/// Saved government and scheduler state; the treasury is part of conserved currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioState {
    /// Index of the next unapplied scheduled event.
    pub next_event: usize,
    /// Live supply overrides, in resource order.
    pub supply: Vec<SupplyEffect>,
    /// Current fiscal policy.
    pub policy: FiscalPolicy,
    /// Tax receipts and redistribution history; no minting.
    pub treasury: Account,
}
impl Default for ScenarioState {
    fn default() -> Self {
        Self {
            next_event: 0,
            supply: Vec::new(),
            policy: FiscalPolicy::default(),
            treasury: Account::new(0, Inventory::default()),
        }
    }
}

/// Scenario facts are included in the same deterministic event stream as agent actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScenarioEvent {
    /// A scheduled event started exactly once.
    Applied {
        /// Configuration index.
        index: usize,
        /// Complete original schedule entry.
        scheduled: ScheduledIntervention,
    },
    /// Temporary supply reduction or expansion ended.
    SupplyRestored {
        /// Resource returning to its base rate.
        resource: Resource,
    },
    /// Owned stock actually destroyed, excluding reserved contractual collateral.
    InventoryLost {
        /// Affected account.
        owner: Party,
        /// Affected individuals.
        participants: Vec<AgentId>,
        /// Stock kind.
        resource: Resource,
        /// Actual loss.
        quantity: u64,
    },
    /// Cash paid into the government treasury, bounded by available money.
    TaxCollected {
        /// Taxpayer account.
        payer: Party,
        /// Affected individuals.
        participants: Vec<AgentId>,
        /// Actual sales tax paid.
        sales: Money,
        /// Actual periodic cash levy paid.
        levy: Money,
    },
    /// Funded government payment; never newly created money.
    Redistribution {
        /// Recipient.
        agent_id: AgentId,
        /// Actual amount.
        amount: Money,
    },
}
