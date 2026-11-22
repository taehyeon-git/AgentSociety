//! Acceptance tests exercise actual multi-system state transitions.

use agentsociety_core::*;
use agentsociety_simulation::{Simulation, SimulationSnapshot};
use std::collections::BTreeMap;

#[test]
fn same_seed_and_inputs_reproduce_state_and_every_event() {
    let config = SimulationConfig {
        agents: 100,
        seed: 42,
        ..Default::default()
    };
    let mut one = Simulation::new(config.clone()).unwrap();
    let mut two = Simulation::new(config).unwrap();
    assert_eq!(one.initial_events(), two.initial_events());
    for tick in 1_u64..=120 {
        let mut external = BTreeMap::new();
        if tick.is_multiple_of(7) {
            external.insert(
                AgentId(2),
                Decision {
                    action: Action::Rest {},
                    reason: DecisionReason::RecoverEnergy,
                },
            );
        }
        let a = one.step_with_overrides(&external).unwrap();
        let b = two.step_with_overrides(&external).unwrap();
        assert_eq!(a.events, b.events);
        assert_eq!(a.stats, b.stats);
    }
    assert_eq!(
        serde_json::to_vec(&one.snapshot().unwrap()).unwrap(),
        serde_json::to_vec(&two.snapshot().unwrap()).unwrap()
    );
}

#[test]
fn one_hundred_agents_run_one_thousand_ticks_with_real_flows() {
    let mut sim = Simulation::new(SimulationConfig::default()).unwrap();
    let initial = sim.snapshot().unwrap();
    for _ in 0..1000 {
        let report = sim.step().unwrap();
        assert_eq!(report.stats.total_money, initial.baseline.total_money);
        assert!(
            report
                .stats
                .prices
                .iter()
                .all(|&price| (1..=MAX_PRICE).contains(&price))
        );
        assert!(report.stats.gini.is_finite());
    }
    assert_eq!(sim.world().tick(), 1000);
    for resource in Resource::ALL {
        assert!(
            sim.cumulative().produced[resource.index()] > 0,
            "no {resource} produced"
        );
        assert!(
            sim.cumulative().traded[resource.index()] > 0,
            "no {resource} traded"
        );
    }
    assert!(sim.cumulative().household_consumed[0] > 0);
    assert!(sim.cumulative().household_consumed[3] > 0);
    assert!(sim.cumulative().production_inputs[1] > 0);
    assert!(sim.cumulative().production_inputs[2] > 0);
    assert!(sim.cumulative().trade_count > 1000);
    assert!(
        sim.world()
            .agents()
            .zip(&initial.world.agents)
            .any(|(end, start)| end.account.wallet() != start.account.wallet())
    );
    assert!(
        sim.world()
            .agents()
            .zip(&initial.world.agents)
            .any(|(end, start)| end.account.inventory() != start.account.inventory())
    );
    assert!(sim.world().agents().all(|a| a.history.len() == 16
        && a.needs.energy <= 100
        && a.needs.hunger <= 100
        && a.needs.comfort <= 100));
}

#[test]
fn zero_one_and_many_agents_are_supported() {
    for agents in [0, 1, 10, 1000] {
        let mut sim = Simulation::new(SimulationConfig {
            agents,
            ..Default::default()
        })
        .unwrap();
        for _ in 0..30 {
            sim.step().unwrap();
        }
        assert_eq!(sim.world().len(), agents);
        assert_eq!(sim.world().tick(), 30);
        assert!(sim.stats().average_wealth.is_finite());
        assert!(sim.stats().average_hunger.is_finite());
    }
}

#[test]
fn event_sequences_are_global_contiguous_and_tick_ordered() {
    let mut sim = Simulation::new(SimulationConfig {
        agents: 10,
        ..Default::default()
    })
    .unwrap();
    let mut next = 0;
    for record in sim.initial_events() {
        assert_eq!(record.sequence, next);
        assert_eq!(record.tick, 0);
        next += 1;
    }
    for tick in 1..=20 {
        for record in sim.step().unwrap().events {
            assert_eq!(record.sequence, next);
            assert_eq!(record.tick, tick);
            next += 1;
        }
    }
}

#[test]
fn invalid_external_intent_is_rejected_and_accounting_continues() {
    let mut sim = Simulation::new(SimulationConfig {
        agents: 10,
        ..Default::default()
    })
    .unwrap();
    let overrides = BTreeMap::from([(
        AgentId(0),
        Decision {
            action: Action::Buy {
                resource: Resource::Food,
                quantity: u64::MAX,
                limit_price: 1,
            },
            reason: DecisionReason::SecureFood,
        },
    )]);
    let report = sim.step_with_overrides(&overrides).unwrap();
    assert_eq!(report.stats.flows.rejected_actions, 1);
    assert!(report.events.iter().any(|e| matches!(
        e.event,
        Event::ActionRejected {
            agent_id: AgentId(0),
            ..
        }
    )));
    assert!(
        !sim.world()
            .agent(AgentId(0))
            .unwrap()
            .history
            .back()
            .unwrap()
            .accepted
    );
    sim.step().unwrap();
}

#[test]
fn unknown_external_agent_does_not_advance_any_state() {
    let mut sim = Simulation::new(SimulationConfig::default()).unwrap();
    let before = sim.snapshot().unwrap();
    let overrides = BTreeMap::from([(
        AgentId(u64::MAX),
        Decision {
            action: Action::Wait {},
            reason: DecisionReason::NoOpportunity,
        },
    )]);
    assert!(sim.step_with_overrides(&overrides).is_err());
    assert_eq!(sim.snapshot().unwrap(), before);
    sim.step().unwrap();
}

#[test]
fn exported_state_roundtrips_without_losing_integer_accounting() {
    let mut sim = Simulation::new(SimulationConfig {
        agents: 10,
        ..Default::default()
    })
    .unwrap();
    for _ in 0..20 {
        sim.step().unwrap();
    }
    let snapshot = sim.snapshot().unwrap();
    let json = serde_json::to_vec(&snapshot).unwrap();
    let decoded: SimulationSnapshot = serde_json::from_slice(&json).unwrap();
    assert_eq!(
        snapshot.stats, decoded.stats,
        "display metrics must round-trip exactly"
    );
    assert!(
        snapshot == decoded,
        "snapshot lost deterministic state during JSON round-trip"
    );
}

#[test]
fn varied_seeds_and_zero_money_do_not_break_conservation() {
    for seed in 0..6 {
        let mut sim = Simulation::new(SimulationConfig {
            agents: 20,
            seed,
            initial_cash: if seed == 0 { 0 } else { 10_000 },
            history_capacity: 0,
            ..Default::default()
        })
        .unwrap();
        for _ in 0..150 {
            sim.step().unwrap();
        }
        assert!(sim.world().agents().all(|a| a.history.is_empty()));
    }
}
