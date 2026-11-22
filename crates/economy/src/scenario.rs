//! Recorded supply shocks and funded fiscal transfers at the authoritative tick boundary.
use crate::InstitutionBook;
use agentsociety_core::*;
use agentsociety_world::World;
use std::collections::BTreeMap;

/// Reconstruct the scheduler's expected position without replaying financial effects.
pub fn validate_scenario(config: &ScenarioConfig, state: &ScenarioState, tick: Tick) -> Result<()> {
    config.validate()?;
    let mut expected = ScenarioState::default();
    for scheduled in config.events.iter().take_while(|e| e.tick <= tick) {
        expected.next_event += 1;
        match &scheduled.intervention {
            Intervention::Supply {
                resource,
                regeneration_bps,
                duration,
            } => {
                let ends_at = scheduled.tick + duration;
                if ends_at > tick {
                    expected.supply.push(SupplyEffect {
                        resource: *resource,
                        regeneration_bps: *regeneration_bps,
                        ends_at,
                    });
                }
            }
            Intervention::Fiscal { policy } => expected.policy = policy.clone(),
            Intervention::InventoryLoss { .. } => {}
        }
    }
    expected.supply.sort_by_key(|s| s.resource);
    if state.next_event != expected.next_event
        || state.supply != expected.supply
        || state.policy != expected.policy
        || state.treasury.inventory() != &Inventory::default()
        || state.treasury.reserved_cash() != 0
        || state.treasury.reserved_inventory() != &Inventory::default()
        || state.treasury.earned() != 0
        || state.treasury.spent() != 0
    {
        return Err(DomainError::Invariant(
            "scenario state disagrees with completed schedule".into(),
        ));
    }
    state.treasury.validate_ledger(0)
}

fn parties(world: &World, institutions: &InstitutionBook) -> Vec<Party> {
    world
        .ids()
        .map(Party::Agent)
        .chain(
            institutions
                .organizations()
                .map(|g| Party::Organization(g.id)),
        )
        .collect()
}

/// Apply scheduled changes before replenishment and observations; reserved stock is protected.
pub fn begin_scenario(
    world: &mut World,
    institutions: &mut InstitutionBook,
    config: &ScenarioConfig,
    state: &mut ScenarioState,
    tick: Tick,
) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    state.supply.retain(|effect| {
        if effect.ends_at <= tick {
            events.push(Event::Scenario {
                event: ScenarioEvent::SupplyRestored {
                    resource: effect.resource,
                },
            });
            false
        } else {
            true
        }
    });
    while let Some(scheduled) = config
        .events
        .get(state.next_event)
        .filter(|e| e.tick == tick)
    {
        events.push(Event::Scenario {
            event: ScenarioEvent::Applied {
                index: state.next_event,
                scheduled: scheduled.clone(),
            },
        });
        match &scheduled.intervention {
            Intervention::Supply {
                resource,
                regeneration_bps,
                duration,
            } => state.supply.push(SupplyEffect {
                resource: *resource,
                regeneration_bps: *regeneration_bps,
                ends_at: tick + duration,
            }),
            Intervention::Fiscal { policy } => state.policy = policy.clone(),
            Intervention::InventoryLoss {
                resource,
                fraction_bps,
            } => {
                for owner in parties(world, institutions) {
                    let mut account = institutions.account(world, owner)?.clone();
                    let quantity = (u128::from(account.available(*resource))
                        * u128::from(*fraction_bps)
                        / 10000) as u64;
                    if quantity == 0 {
                        continue;
                    }
                    account.consume(*resource, quantity)?;
                    institutions.replace_account(world, owner, account)?;
                    events.push(Event::Scenario {
                        event: ScenarioEvent::InventoryLost {
                            owner,
                            participants: institutions.owners(owner)?,
                            resource: *resource,
                            quantity,
                        },
                    });
                }
            }
        }
        state.next_event += 1;
    }
    state.supply.sort_by_key(|s| s.resource);
    let mut factors = [10000; RESOURCE_COUNT];
    for effect in &state.supply {
        factors[effect.resource.index()] = effect.regeneration_bps;
    }
    world.regenerate_scaled(&factors)?;
    Ok(events)
}

/// Collect affordable taxes, then distribute existing treasury funds; no debt or issuance.
pub fn settle_fiscal(
    world: &mut World,
    institutions: &mut InstitutionBook,
    state: &mut ScenarioState,
    tick: Tick,
    activity: &[Event],
) -> Result<Vec<Event>> {
    let policy = &state.policy;
    let periodic = tick.is_multiple_of(policy.period);
    if policy.sales_tax_bps == 0
        && (!periodic || (policy.cash_levy_bps == 0 && policy.redistribution_bps == 0))
    {
        return Ok(Vec::new());
    }
    let mut sales: BTreeMap<Party, u128> = BTreeMap::new();
    for event in activity {
        let sale = match event {
            Event::TradeExecuted {
                seller,
                quantity,
                price,
                ..
            }
            | Event::NegotiatedTrade {
                seller,
                quantity,
                price,
                ..
            } => Some((Party::Agent(*seller), *quantity, *price)),
            Event::Contract {
                event:
                    ContractEvent::Delivered {
                        seller,
                        quantity,
                        price,
                        ..
                    },
            } => Some((*seller, *quantity, *price)),
            _ => None,
        };
        if let Some((owner, quantity, price)) = sale {
            let total = sales.entry(owner).or_default();
            *total = total
                .checked_add(u128::from(quantity) * u128::from(price))
                .ok_or(DomainError::Overflow("taxable sales"))?;
        }
    }
    let mut events = Vec::new();
    for payer in parties(world, institutions) {
        let mut account = institutions.account(world, payer)?.clone();
        let due =
            sales.get(&payer).copied().unwrap_or(0) * u128::from(policy.sales_tax_bps) / 10000;
        let sales = due.min(u128::from(account.available_cash())) as u64;
        let levy = if periodic {
            (u128::from(account.available_cash() - sales) * u128::from(policy.cash_levy_bps)
                / 10000) as u64
        } else {
            0
        };
        if sales + levy == 0 {
            continue;
        }
        transfer_assets(
            &mut account,
            &mut state.treasury,
            sales + levy,
            &Inventory::default(),
        )?;
        institutions.replace_account(world, payer, account)?;
        events.push(Event::Scenario {
            event: ScenarioEvent::TaxCollected {
                payer,
                participants: institutions.owners(payer)?,
                sales,
                levy,
            },
        });
    }
    if periodic && !world.is_empty() {
        let budget = (u128::from(state.treasury.available_cash())
            * u128::from(policy.redistribution_bps)
            / 10000) as u64;
        let count = world.len() as u64;
        let ids: Vec<_> = world.ids().collect();
        for (index, agent_id) in ids.into_iter().enumerate() {
            let amount = budget / count + u64::from((index as u64) < budget % count);
            if amount == 0 {
                continue;
            }
            world.with_agent_mut(agent_id, |a| {
                transfer_assets(
                    &mut state.treasury,
                    &mut a.account,
                    amount,
                    &Inventory::default(),
                )
            })?;
            events.push(Event::Scenario {
                event: ScenarioEvent::Redistribution { agent_id, amount },
            });
        }
    }
    Ok(events)
}
