//! Funded taxes, protected collateral and exact supply recovery boundaries.
use agentsociety_core::*;
use agentsociety_economy::{InstitutionBook, begin_scenario, settle_fiscal, validate_scenario};
use agentsociety_world::World;

#[test]
fn fiscal_payments_use_real_receipts_and_exact_funded_integer_shares() {
    let mut world = World::seeded(&SimulationConfig {
        agents: 2,
        ..Default::default()
    })
    .unwrap();
    world
        .with_agent_mut(AgentId(0), |a| {
            a.account = Account::new(100, Inventory::new([0, 10, 0, 0]));
            Ok(())
        })
        .unwrap();
    world
        .with_agent_mut(AgentId(1), |a| {
            a.account = Account::new(1000, Inventory::default());
            Ok(())
        })
        .unwrap();
    world
        .with_pair_mut(AgentId(1), AgentId(0), |buyer, seller| {
            buyer.account.reserve_buy(50, 2)?;
            seller.account.reserve_sell(Resource::Wood, 2)?;
            settle_trade(
                &mut buyer.account,
                &mut seller.account,
                Resource::Wood,
                2,
                50,
                50,
            )
        })
        .unwrap();
    let mut state = ScenarioState {
        policy: FiscalPolicy {
            sales_tax_bps: 1000,
            cash_levy_bps: 1000,
            redistribution_bps: 5000,
            period: 2,
        },
        ..Default::default()
    };
    let sale = Event::TradeExecuted {
        buy_order_id: 0,
        sell_order_id: 1,
        buyer: AgentId(1),
        seller: AgentId(0),
        resource: Resource::Wood,
        quantity: 2,
        price: 50,
    };
    let events = settle_fiscal(
        &mut world,
        &mut InstitutionBook::default(),
        &mut state,
        2,
        &[sale],
    )
    .unwrap();
    assert!(matches!(
        events[0],
        Event::Scenario {
            event: ScenarioEvent::TaxCollected {
                sales: 10,
                levy: 19,
                ..
            }
        }
    ));
    assert_eq!(state.treasury.wallet(), 60);
    assert_eq!(state.treasury.transfers_in(), 119);
    assert_eq!(state.treasury.transfers_out(), 59);
    assert_eq!(world.agent(AgentId(0)).unwrap().account.wallet(), 201);
    assert_eq!(world.agent(AgentId(1)).unwrap().account.wallet(), 839);
    world
        .agent(AgentId(0))
        .unwrap()
        .account
        .validate_ledger(100)
        .unwrap();
    world
        .agent(AgentId(1))
        .unwrap()
        .account
        .validate_ledger(1000)
        .unwrap();
    state.treasury.validate_ledger(0).unwrap();
}

#[test]
fn shocks_recover_on_schedule_and_losses_preserve_reserved_stock() {
    let mut world = World::seeded(&SimulationConfig {
        agents: 1,
        ..Default::default()
    })
    .unwrap();
    let mut book = InstitutionBook::default();
    let mut state = ScenarioState::default();
    let config = ScenarioConfig {
        name: "shock".into(),
        events: vec![
            ScheduledIntervention {
                tick: 1,
                label: "Food shock".into(),
                intervention: Intervention::Supply {
                    resource: Resource::Food,
                    regeneration_bps: 0,
                    duration: 2,
                },
            },
            ScheduledIntervention {
                tick: 1,
                label: "Food loss".into(),
                intervention: Intervention::InventoryLoss {
                    resource: Resource::Food,
                    fraction_bps: 5000,
                },
            },
        ],
    };
    world
        .with_agent_mut(AgentId(0), |a| a.account.reserve_sell(Resource::Food, 10))
        .unwrap();
    let initial_pool = world.resource_pool().clone();
    let events = begin_scenario(&mut world, &mut book, &config, &mut state, 1).unwrap();
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Scenario {
            event: ScenarioEvent::InventoryLost { quantity: 5, .. }
        }
    )));
    assert_eq!(
        world
            .agent(AgentId(0))
            .unwrap()
            .account
            .inventory()
            .get(Resource::Food),
        15
    );
    assert_eq!(
        world
            .agent(AgentId(0))
            .unwrap()
            .account
            .reserved_inventory()
            .get(Resource::Food),
        10
    );
    world.extract(AgentId(0), Resource::Food, 3).unwrap();
    begin_scenario(&mut world, &mut book, &config, &mut state, 2).unwrap();
    assert_eq!(
        world.resource_pool().available.get(Resource::Food),
        initial_pool.available.get(Resource::Food) - 3
    );
    validate_scenario(&config, &state, 2).unwrap();
    let events = begin_scenario(&mut world, &mut book, &config, &mut state, 3).unwrap();
    assert!(matches!(
        events.as_slice(),
        [Event::Scenario {
            event: ScenarioEvent::SupplyRestored {
                resource: Resource::Food
            }
        }]
    ));
    assert_eq!(
        world.resource_pool().available.get(Resource::Food),
        initial_pool.available.get(Resource::Food)
    );
    assert_eq!(
        world.resource_pool().regeneration,
        initial_pool.regeneration
    );
    validate_scenario(&config, &state, 3).unwrap();
}

#[test]
fn tax_cannot_spend_cash_pledged_to_an_agreement() {
    let mut world = World::seeded(&SimulationConfig {
        agents: 1,
        ..Default::default()
    })
    .unwrap();
    world
        .with_agent_mut(AgentId(0), |a| {
            a.account = Account::new(100, Inventory::default());
            a.account.reserve_buy(99, 1)
        })
        .unwrap();
    let mut state = ScenarioState {
        policy: FiscalPolicy {
            sales_tax_bps: 5000,
            ..Default::default()
        },
        ..Default::default()
    };
    let activity = [Event::NegotiatedTrade {
        proposal_id: 0,
        buyer: AgentId(1),
        seller: AgentId(0),
        resource: Resource::Wood,
        quantity: 1,
        price: 100,
    }];
    settle_fiscal(
        &mut world,
        &mut InstitutionBook::default(),
        &mut state,
        1,
        &activity,
    )
    .unwrap();
    assert_eq!(world.agent(AgentId(0)).unwrap().account.wallet(), 99);
    assert_eq!(world.agent(AgentId(0)).unwrap().account.reserved_cash(), 99);
    assert_eq!(state.treasury.wallet(), 1);
}

#[test]
fn overlapping_supply_windows_and_invalid_rates_are_rejected_before_running() {
    let mut config = ScenarioConfig {
        name: "bad".into(),
        events: vec![ScheduledIntervention {
            tick: 2,
            label: "shock".into(),
            intervention: Intervention::Supply {
                resource: Resource::Metal,
                regeneration_bps: 0,
                duration: 4,
            },
        }],
    };
    config.events.push(ScheduledIntervention {
        tick: 5,
        ..config.events[0].clone()
    });
    assert!(config.validate().is_err());
    config.events[1].tick = 6;
    config.validate().unwrap();
    config.events[1].intervention = Intervention::Fiscal {
        policy: FiscalPolicy {
            sales_tax_bps: 5001,
            ..Default::default()
        },
    };
    assert!(config.validate().is_err());
}
