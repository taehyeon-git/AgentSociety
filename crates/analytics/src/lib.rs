//! Event-derived flows, mark-to-market wealth, and per-tick conservation audits.

use agentsociety_core::*;
use agentsociety_world::World;
use serde::{Deserialize, Serialize};

/// Resource and money flows over either one tick or an entire run.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowTotals {
    /// New resources credited by farming, extraction, and manufacturing.
    pub produced: ResourceTotals,
    /// Household food and goods use.
    pub household_consumed: ResourceTotals,
    /// Factory inputs, separately accounted from household consumption.
    pub production_inputs: ResourceTotals,
    /// Transferred quantities (not production or consumption).
    pub traded: ResourceTotals,
    /// Submitted bid quantities, including unfilled quantities.
    pub buy_demand: ResourceTotals,
    /// Submitted ask quantities, including unfilled quantities.
    pub sell_supply: ResourceTotals,
    /// Number of bilateral fills.
    pub trade_count: u64,
    /// Total money transferred by trades, in minor units.
    pub trade_value: u128,
    /// Number of rejected intents.
    pub rejected_actions: u64,
    /// Newly opened bilateral price negotiations, excluding counteroffers.
    pub negotiations: u64,
    /// Completed negotiated exchanges, also included in trade_count and trade_value.
    pub negotiated_trades: u64,
    /// Completed joint manufacturing batches.
    pub cooperations: u64,
    /// Actual adaptive objective changes.
    pub goal_changes: u64,
    /// Stock destroyed by recorded crises, never counted as consumption.
    pub destroyed: ResourceTotals,
    /// Newly chartered cooperatives.
    pub organizations_formed: u64,
    /// Shared manufacturing batches completed by at least three members.
    pub group_productions: u64,
    /// Recurring delivery agreements proposed.
    pub contracts_offered: u64,
    /// Fulfilled installments, included in total trade volume and value.
    pub contract_deliveries: u64,
    /// Accepted contracts closed with a funded default penalty.
    pub contract_defaults: u64,
    /// Scheduled interventions started.
    pub scenario_events: u64,
    /// Actual payments to the treasury.
    pub taxes: u128,
    /// Actual treasury payments to citizens.
    pub redistributed: u128,
    /// Default compensation, separate from sales revenue.
    pub compensation: u128,
}

impl FlowTotals {
    /// Derive flows from actual accepted events, never from intended actions.
    pub fn from_events(events: &[Event]) -> Result<Self> {
        let mut totals = Self::default();
        for event in events {
            match event {
                Event::Organization {
                    event: OrganizationEvent::Founded { .. },
                } => increment(&mut totals.organizations_formed)?,
                Event::Organization {
                    event: OrganizationEvent::Produced { inputs, goods, .. },
                } => {
                    increment(&mut totals.group_productions)?;
                    for r in Resource::ALL {
                        add(
                            &mut totals.production_inputs[r.index()],
                            inputs.get(r).into(),
                        )?;
                    }
                    add(
                        &mut totals.produced[Resource::Goods.index()],
                        (*goods).into(),
                    )?;
                }
                Event::Contract {
                    event: ContractEvent::Offered { .. },
                } => increment(&mut totals.contracts_offered)?,
                Event::Contract {
                    event:
                        ContractEvent::Delivered {
                            resource,
                            quantity,
                            price,
                            ..
                        },
                } => {
                    increment(&mut totals.contract_deliveries)?;
                    increment(&mut totals.trade_count)?;
                    add(&mut totals.traded[resource.index()], (*quantity).into())?;
                    add(
                        &mut totals.trade_value,
                        u128::from(*quantity) * u128::from(*price),
                    )?;
                }
                Event::Contract {
                    event:
                        ContractEvent::Closed {
                            outcome: ContractOutcome::Defaulted,
                            compensation,
                            ..
                        },
                } => {
                    increment(&mut totals.contract_defaults)?;
                    add(&mut totals.compensation, (*compensation).into())?;
                }
                Event::Scenario {
                    event: ScenarioEvent::Applied { .. },
                } => increment(&mut totals.scenario_events)?,
                Event::Scenario {
                    event:
                        ScenarioEvent::InventoryLost {
                            resource, quantity, ..
                        },
                } => add(&mut totals.destroyed[resource.index()], (*quantity).into())?,
                Event::Scenario {
                    event: ScenarioEvent::TaxCollected { sales, levy, .. },
                } => add(&mut totals.taxes, u128::from(*sales) + u128::from(*levy))?,
                Event::Scenario {
                    event: ScenarioEvent::Redistribution { amount, .. },
                } => add(&mut totals.redistributed, (*amount).into())?,
                _ => {}
            }
            match *event {
                Event::ResourceProduced {
                    resource, quantity, ..
                } => add(&mut totals.produced[resource.index()], quantity.into())?,
                Event::ResourceConsumed {
                    resource,
                    quantity,
                    purpose,
                    ..
                } => {
                    let counts = match purpose {
                        ConsumptionPurpose::Household => &mut totals.household_consumed,
                        ConsumptionPurpose::ProductionInput => &mut totals.production_inputs,
                    };
                    add(&mut counts[resource.index()], quantity.into())?;
                }
                Event::TradeExecuted {
                    resource,
                    quantity,
                    price,
                    ..
                }
                | Event::NegotiatedTrade {
                    resource,
                    quantity,
                    price,
                    ..
                } => {
                    add(&mut totals.traded[resource.index()], quantity.into())?;
                    add(
                        &mut totals.trade_value,
                        u128::from(quantity) * u128::from(price),
                    )?;
                    totals.trade_count = totals
                        .trade_count
                        .checked_add(1)
                        .ok_or(DomainError::Overflow("trade count"))?;
                    if matches!(event, Event::NegotiatedTrade { .. }) {
                        increment(&mut totals.negotiated_trades)?;
                    }
                }
                Event::ProposalOpened { ref proposal }
                    if matches!(proposal.terms, ProposalTerms::Trade { .. }) =>
                {
                    increment(&mut totals.negotiations)?
                }
                Event::CooperationCompleted { .. } => increment(&mut totals.cooperations)?,
                Event::GoalChanged { .. } => increment(&mut totals.goal_changes)?,
                Event::OrderPlaced {
                    resource,
                    quantity,
                    side,
                    ..
                } => {
                    let counts = match side {
                        OrderSide::Buy => &mut totals.buy_demand,
                        OrderSide::Sell => &mut totals.sell_supply,
                    };
                    add(&mut counts[resource.index()], quantity.into())?;
                }
                Event::ActionRejected { .. } => {
                    totals.rejected_actions = totals
                        .rejected_actions
                        .checked_add(1)
                        .ok_or(DomainError::Overflow("rejection count"))?;
                }
                _ => {}
            }
        }
        Ok(totals)
    }

    /// Merge a tick's flows, with explicit overflow reporting.
    pub fn accumulate(&mut self, tick: &Self) -> Result<()> {
        let mut next = self.clone();
        for i in 0..RESOURCE_COUNT {
            add(&mut next.produced[i], tick.produced[i])?;
            add(&mut next.household_consumed[i], tick.household_consumed[i])?;
            add(&mut next.production_inputs[i], tick.production_inputs[i])?;
            add(&mut next.traded[i], tick.traded[i])?;
            add(&mut next.buy_demand[i], tick.buy_demand[i])?;
            add(&mut next.sell_supply[i], tick.sell_supply[i])?;
            add(&mut next.destroyed[i], tick.destroyed[i])?;
        }
        add(&mut next.trade_value, tick.trade_value)?;
        add(&mut next.taxes, tick.taxes)?;
        add(&mut next.redistributed, tick.redistributed)?;
        add(&mut next.compensation, tick.compensation)?;
        next.trade_count = next
            .trade_count
            .checked_add(tick.trade_count)
            .ok_or(DomainError::Overflow("trade count"))?;
        next.rejected_actions = next
            .rejected_actions
            .checked_add(tick.rejected_actions)
            .ok_or(DomainError::Overflow("rejection count"))?;
        for (total, value) in [
            (&mut next.negotiations, tick.negotiations),
            (&mut next.negotiated_trades, tick.negotiated_trades),
            (&mut next.cooperations, tick.cooperations),
            (&mut next.goal_changes, tick.goal_changes),
            (&mut next.organizations_formed, tick.organizations_formed),
            (&mut next.group_productions, tick.group_productions),
            (&mut next.contracts_offered, tick.contracts_offered),
            (&mut next.contract_deliveries, tick.contract_deliveries),
            (&mut next.contract_defaults, tick.contract_defaults),
            (&mut next.scenario_events, tick.scenario_events),
        ] {
            *total = total
                .checked_add(value)
                .ok_or(DomainError::Overflow("interaction count"))?;
        }
        *self = next;
        Ok(())
    }
}

fn increment(total: &mut u64) -> Result<()> {
    *total = total
        .checked_add(1)
        .ok_or(DomainError::Overflow("interaction count"))?;
    Ok(())
}

fn add(total: &mut u128, amount: u128) -> Result<()> {
    *total = total
        .checked_add(amount)
        .ok_or(DomainError::Overflow("aggregate statistic"))?;
    Ok(())
}

/// Statistics at a tick boundary. Floating point is confined to display metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EconomyStats {
    /// Number of completed ticks.
    pub tick: Tick,
    /// Population.
    pub agent_count: usize,
    /// Currency held by people, cooperatives and the government, counted once.
    pub total_money: u128,
    /// Cash plus stock valued at the latest traded prices.
    pub total_wealth: u128,
    /// Mean wealth in minor currency units (zero for no agents).
    pub average_wealth: f64,
    /// Gini coefficient in [0, 1]; zero for an empty or zero-wealth population.
    pub gini: f64,
    /// Current resource prices, in minor currency units.
    pub prices: Prices,
    /// Owned physical resources; environmental capacity is excluded.
    pub inventories: ResourceTotals,
    /// Number of agents with nonzero hunger.
    pub hungry_agents: usize,
    /// Mean hunger (zero for an empty population).
    pub average_hunger: f64,
    /// Number of active cooperatives.
    pub organizations: usize,
    /// Shared cash, already included in total_money.
    pub organization_cash: u128,
    /// Treasury cash, already included in total_money.
    pub treasury_money: u128,
    /// Pending or accepted recurring agreements.
    pub open_contracts: usize,
    /// Actual flows for this tick only.
    pub flows: FlowTotals,
}

/// Initial conservation reference, independent of market valuations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConservationBaseline {
    /// Fixed total monetary supply.
    pub total_money: u128,
    /// Initial owned resources in canonical resource order.
    pub inventories: ResourceTotals,
}

impl From<&EconomyStats> for ConservationBaseline {
    fn from(stats: &EconomyStats) -> Self {
        Self {
            total_money: stats.total_money,
            inventories: stats.inventories,
        }
    }
}

/// Calculate a distribution statistic with defined empty and zero-income behavior.
pub fn gini(values: &[u128]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let sum: f64 = sorted.iter().map(|&v| v as f64).sum();
    if sum == 0.0 {
        return 0.0;
    }
    let weighted: f64 = sorted
        .iter()
        .enumerate()
        .map(|(i, &v)| (i + 1) as f64 * v as f64)
        .sum();
    let n = sorted.len() as f64;
    ((2.0 * weighted) / (n * sum) - (n + 1.0) / n).clamp(0.0, 1.0)
}

/// Compute population statistics without changing the world.
pub fn collect(world: &World, prices: Prices, flows: FlowTotals) -> Result<EconomyStats> {
    collect_with_ownership(
        world,
        prices,
        flows,
        &[],
        &Account::new(0, Inventory::default()),
        0,
    )
}

/// Include common assets and government cash; private wealth assigns each member an equal share.
pub fn collect_with_ownership(
    world: &World,
    prices: Prices,
    flows: FlowTotals,
    organizations: &[&Organization],
    treasury: &Account,
    open_contracts: usize,
) -> Result<EconomyStats> {
    let mut shares = std::collections::BTreeMap::<AgentId, u128>::new();
    let mut organization_cash = 0;
    let mut inventories = [0; RESOURCE_COUNT];
    for group in organizations {
        group.account.validate_ledger(0)?;
        let count = group.members.len() as u128;
        if count == 0 {
            return Err(DomainError::Invariant("empty ownership group".into()));
        }
        let wealth = group.account.wealth(&prices);
        for (index, &id) in group.members.iter().enumerate() {
            world.agent(id)?;
            add(
                shares.entry(id).or_default(),
                wealth / count + u128::from((index as u128) < wealth % count),
            )?;
        }
        add(&mut organization_cash, group.account.wallet().into())?;
        for r in Resource::ALL {
            add(
                &mut inventories[r.index()],
                group.account.inventory().get(r).into(),
            )?;
        }
    }
    treasury.validate_ledger(0)?;
    let mut total_money = 0;
    let mut total_wealth = 0;
    let mut wealths = Vec::with_capacity(world.len());
    let mut hungry_agents = 0;
    let mut hunger = 0_u64;
    for agent in world.agents() {
        agent.account.validate()?;
        add(&mut total_money, agent.account.wallet().into())?;
        let wealth = agent
            .account
            .wealth(&prices)
            .checked_add(shares.get(&agent.id).copied().unwrap_or(0))
            .ok_or(DomainError::Overflow("private wealth"))?;
        add(&mut total_wealth, wealth)?;
        wealths.push(wealth);
        for resource in Resource::ALL {
            add(
                &mut inventories[resource.index()],
                agent.account.inventory().get(resource).into(),
            )?;
        }
        hungry_agents += usize::from(agent.needs.hunger > 0);
        hunger += u64::from(agent.needs.hunger);
    }
    let denominator = world.len().max(1) as f64;
    let average_wealth = total_wealth as f64 / denominator;
    add(&mut total_money, organization_cash)?;
    add(&mut total_money, treasury.wallet().into())?;
    add(&mut total_wealth, treasury.wealth(&prices))?;
    Ok(EconomyStats {
        tick: world.tick(),
        agent_count: world.len(),
        total_money,
        total_wealth,
        average_wealth,
        gini: gini(&wealths),
        prices,
        inventories,
        hungry_agents,
        average_hunger: hunger as f64 / denominator,
        organizations: organizations.len(),
        organization_cash,
        treasury_money: treasury.wallet().into(),
        open_contracts,
        flows,
    })
}

/// Check money conservation, resource flow equations, and reservation bounds.
/// The simulation additionally attributes every live reservation to its proposal book.
pub fn audit_balances(
    world: &World,
    stats: &EconomyStats,
    baseline: &ConservationBaseline,
    cumulative: &FlowTotals,
) -> Result<()> {
    if stats.total_money != baseline.total_money {
        return Err(DomainError::Invariant(format!(
            "money supply changed: {} -> {}",
            baseline.total_money, stats.total_money
        )));
    }
    for resource in Resource::ALL {
        let i = resource.index();
        let mut created = baseline.inventories[i];
        add(&mut created, cumulative.produced[i])?;
        let mut accounted = stats.inventories[i];
        add(&mut accounted, cumulative.household_consumed[i])?;
        add(&mut accounted, cumulative.production_inputs[i])?;
        add(&mut accounted, cumulative.destroyed[i])?;
        if created != accounted {
            return Err(DomainError::Invariant(format!(
                "{resource} balance: created={created}, accounted={accounted}"
            )));
        }
    }
    for agent in world.agents() {
        agent.account.validate()?;
    }
    Ok(())
}

/// Audit a market-only world in which all collateral must be released at tick end.
pub fn audit(
    world: &World,
    stats: &EconomyStats,
    baseline: &ConservationBaseline,
    cumulative: &FlowTotals,
) -> Result<()> {
    audit_balances(world, stats, baseline, cumulative)?;
    for agent in world.agents() {
        if agent.account.reserved_cash() != 0
            || agent.account.reserved_inventory().amounts() != [0; 4]
        {
            return Err(DomainError::Invariant(format!(
                "agent {} retains collateral at tick end",
                agent.id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gini_handles_empty_equal_and_concentrated_wealth() {
        assert_eq!(gini(&[]), 0.0);
        assert_eq!(gini(&[0, 0]), 0.0);
        assert_eq!(gini(&[50, 50, 50]), 0.0);
        assert!((gini(&[0, 100]) - 0.5).abs() < 1e-12);
        assert!((gini(&[0, 0, 100]) - 2.0 / 3.0).abs() < 1e-12);
        assert!(gini(&[u128::MAX, 0]).is_finite());
    }

    #[test]
    fn empty_world_statistics_are_finite() {
        let config = SimulationConfig {
            agents: 0,
            ..Default::default()
        };
        let world = World::seeded(&config).unwrap();
        let stats = collect(&world, config.initial_prices, FlowTotals::default()).unwrap();
        assert_eq!(stats.average_wealth, 0.0);
        assert_eq!(stats.average_hunger, 0.0);
        audit(&world, &stats, &(&stats).into(), &FlowTotals::default()).unwrap();
    }
}
