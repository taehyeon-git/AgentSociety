//! Shared ownership, shocks, persistent contracts and their identical restored futures.
use agentsociety_core::*;
use agentsociety_simulation::Simulation;
use std::collections::BTreeMap;

fn config() -> SimulationConfig {
    SimulationConfig {
        agents: 6,
        organizations: OrganizationSettings {
            enabled: true,
            ..Default::default()
        },
        interactions: InteractionSettings {
            negotiation: true,
            cooperation: true,
            goal_review_interval: 5,
            ..Default::default()
        },
        scenario: ScenarioConfig {
            name: "scheduled".into(),
            events: vec![
                ScheduledIntervention {
                    tick: 1,
                    label: "Tax".into(),
                    intervention: Intervention::Fiscal {
                        policy: FiscalPolicy {
                            cash_levy_bps: 100,
                            redistribution_bps: 5000,
                            period: 2,
                            ..Default::default()
                        },
                    },
                },
                ScheduledIntervention {
                    tick: 3,
                    label: "Supply shock".into(),
                    intervention: Intervention::Supply {
                        resource: Resource::Food,
                        regeneration_bps: 0,
                        duration: 6,
                    },
                },
                ScheduledIntervention {
                    tick: 7,
                    label: "Stock loss".into(),
                    intervention: Intervention::InventoryLoss {
                        resource: Resource::Wood,
                        fraction_bps: 5000,
                    },
                },
            ],
        },
        ..Default::default()
    }
}
fn step(
    sim: &mut Simulation,
    actions: &[(u64, InstitutionAction)],
) -> agentsociety_simulation::TickReport {
    let mut overrides: BTreeMap<_, _> = sim
        .world()
        .ids()
        .map(|id| {
            (
                id,
                Decision {
                    action: Action::Wait {},
                    reason: DecisionReason::NoOpportunity,
                },
            )
        })
        .collect();
    for (id, operation) in actions {
        overrides.get_mut(&AgentId(*id)).unwrap().action = Action::Institution {
            operation: operation.clone(),
        };
    }
    sim.step_with_overrides(&overrides).unwrap()
}
fn active() -> Simulation {
    let mut sim = Simulation::new(config()).unwrap();
    step(&mut sim, &[(1, InstitutionAction::Found {})]);
    step(
        &mut sim,
        &[
            (
                2,
                InstitutionAction::Join {
                    organization: OrganizationId(0),
                },
            ),
            (
                3,
                InstitutionAction::Join {
                    organization: OrganizationId(0),
                },
            ),
        ],
    );
    step(
        &mut sim,
        &[
            (
                2,
                InstitutionAction::Contribute {
                    organization: OrganizationId(0),
                    cash: 0,
                    inventory: Inventory::new([0, 4, 2, 0]),
                },
            ),
            (
                1,
                InstitutionAction::OfferContract {
                    organization: Some(OrganizationId(0)),
                    counterparty: Party::Agent(AgentId(5)),
                    side: OrderSide::Buy,
                    resource: Resource::Wood,
                    quantity: 1,
                    unit_price: 10,
                    deliveries: 3,
                    interval_ticks: 2,
                },
            ),
        ],
    );
    step(
        &mut sim,
        &[(5, InstitutionAction::AcceptContract { contract_id: 0 })],
    );
    assert_eq!(sim.institutions().contracts().len(), 1);
    assert_eq!(sim.cumulative().group_productions, 1);
    sim
}

#[test]
fn live_contract_and_supply_shock_survive_json_restore_and_event_replay() {
    let mut continuous = active();
    let snapshot = continuous.snapshot().unwrap();
    assert_eq!(snapshot.scenario.supply.len(), 1);
    let bytes = serde_json::to_vec(&snapshot).unwrap();
    let mut resumed = Simulation::from_snapshot(serde_json::from_slice(&bytes).unwrap()).unwrap();
    let mut replayed = Simulation::from_snapshot(snapshot).unwrap();
    for _ in 0..12 {
        let report = step(&mut continuous, &[]);
        let records: Vec<EventRecord> =
            serde_json::from_slice(&serde_json::to_vec(&report.events).unwrap()).unwrap();
        assert_eq!(step(&mut resumed, &[]).events, records);
        replayed.replay_tick(&records).unwrap();
    }
    assert_eq!(continuous.snapshot().unwrap(), resumed.snapshot().unwrap());
    assert_eq!(continuous.snapshot().unwrap(), replayed.snapshot().unwrap());
    assert_eq!(continuous.cumulative().contract_deliveries, 3);
    assert!(continuous.cumulative().destroyed[Resource::Wood.index()] > 0);
    assert!(continuous.cumulative().taxes > continuous.cumulative().redistributed);
    assert!(continuous.scenario().supply.is_empty());
}

#[test]
fn branch_preserves_current_state_and_rejects_past_or_overlapping_changes() {
    let original = active();
    let before = original.snapshot().unwrap();
    let mut future = ScenarioConfig {
        name: "alternate".into(),
        events: vec![ScheduledIntervention {
            tick: 5,
            label: "New fiscal policy".into(),
            intervention: Intervention::Fiscal {
                policy: FiscalPolicy {
                    cash_levy_bps: 500,
                    redistribution_bps: 10000,
                    period: 1,
                    ..Default::default()
                },
            },
        }],
    };
    let mut branched = original.branch_scenario(future.clone()).unwrap();
    let branch = branched.snapshot().unwrap();
    assert_eq!(branch.world, before.world);
    assert_eq!(branch.institutions, before.institutions);
    assert_eq!(branch.random, before.random);
    assert_eq!(branch.scenario, before.scenario);
    assert_eq!(branch.config.scenario.events.len(), 3);
    let mut continued = Simulation::from_snapshot(before.clone()).unwrap();
    step(&mut branched, &[]);
    step(&mut continued, &[]);
    assert_ne!(branched.scenario().policy, continued.scenario().policy);
    assert_eq!(branched.stats().total_money, continued.stats().total_money);
    future.events[0].tick = 4;
    assert!(original.branch_scenario(future.clone()).is_err());
    future.events[0].tick = 5;
    future.events[0].intervention = Intervention::Supply {
        resource: Resource::Food,
        regeneration_bps: 20000,
        duration: 1,
    };
    assert!(original.branch_scenario(future).is_err());
    assert_eq!(original.snapshot().unwrap(), before);
}

#[test]
fn inconsistent_shared_accounts_contract_clocks_and_policy_cursor_fail_restore() {
    let original = active().snapshot().unwrap();
    let source = serde_json::to_value(&original).unwrap();
    for (pointer, value) in [
        ("/institutions/organizations/0/leader", serde_json::json!(5)),
        (
            "/institutions/organizations/0/account/reserved_cash",
            serde_json::json!(0),
        ),
        (
            "/institutions/contracts/0/next_delivery",
            serde_json::json!(5),
        ),
        ("/institutions/contracts/0/bond", serde_json::json!(0)),
        ("/scenario/next_event", serde_json::json!(0)),
        ("/scenario/supply/0/ends_at", serde_json::json!(10)),
        ("/scenario/treasury/transfers_in", serde_json::json!(0)),
        ("/cumulative/group_productions", serde_json::json!(0)),
    ] {
        let mut changed = source.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        let snapshot = serde_json::from_slice(&serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(
            Simulation::from_snapshot(snapshot).is_err(),
            "accepted {pointer}"
        );
    }
}

#[test]
fn autonomous_complete_example_forms_groups_and_executes_real_agreements() {
    let config: SimulationConfig =
        serde_json::from_str(include_str!("../../../examples/complete.json")).unwrap();
    let mut sim = Simulation::new(config).unwrap();
    for _ in 0..220 {
        sim.step().unwrap();
    }
    assert!(sim.cumulative().organizations_formed > 0);
    assert!(sim.cumulative().group_productions > 0);
    assert!(sim.cumulative().contract_deliveries > 0);
    assert_eq!(sim.cumulative().scenario_events, 5);
    let restored = Simulation::from_snapshot(sim.snapshot().unwrap()).unwrap();
    assert_eq!(restored.snapshot().unwrap(), sim.snapshot().unwrap());
}
