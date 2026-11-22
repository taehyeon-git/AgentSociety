//! Memories must follow actual simulation events without changing economic outcomes.

use agentsociety_core::*;
use agentsociety_simulation::Simulation;
use std::collections::BTreeMap;

#[test]
fn enabling_memory_preserves_economy_rng_and_event_determinism() {
    let mut enabled = Simulation::new(SimulationConfig {
        agents: 20,
        memory_capacity: 16,
        ..Default::default()
    })
    .unwrap();
    let mut disabled = Simulation::new(SimulationConfig {
        agents: 20,
        memory_capacity: 0,
        ..Default::default()
    })
    .unwrap();
    for _ in 0..150 {
        let a = enabled.step().unwrap();
        let b = disabled.step().unwrap();
        assert_eq!(a.events, b.events);
        assert_eq!(a.stats, b.stats);
    }
    let mut a = enabled.snapshot().unwrap();
    let b = disabled.snapshot().unwrap();
    assert!(
        a.world
            .agents
            .iter()
            .all(|agent| agent.memories.len() == 16)
    );
    assert!(b.world.agents.iter().all(|agent| agent.memories.is_empty()));
    a.config.memory_capacity = 0;
    for agent in &mut a.world.agents {
        agent.memories.clear();
    }
    assert_eq!(a, b);
}

#[test]
fn memory_sources_are_real_ordered_events_and_trades_use_filled_values() {
    let mut sim = Simulation::new(SimulationConfig {
        agents: 20,
        ..Default::default()
    })
    .unwrap();
    let mut events: BTreeMap<_, _> = sim
        .initial_events()
        .iter()
        .map(|r| (r.sequence, r.clone()))
        .collect();
    for _ in 0..50 {
        events.extend(
            sim.step()
                .unwrap()
                .events
                .into_iter()
                .map(|r| (r.sequence, r)),
        );
    }
    let mut economic_count = 0;
    for agent in sim.world().agents() {
        assert!(agent.memories.len() <= sim.config().memory_capacity);
        assert!(
            agent
                .memories
                .iter()
                .zip(agent.memories.iter().skip(1))
                .all(|(a, b)| a.id < b.id && a.tick <= b.tick)
        );
        for memory in &agent.memories {
            assert_eq!(memory.agent_id, agent.id);
            let source = &events[&memory.id.0];
            assert_eq!(memory.tick, source.tick);
            if let MemoryContent::Trade {
                side,
                counterparty,
                resource,
                quantity,
                price,
            } = memory.content
            {
                economic_count += 1;
                let Event::TradeExecuted {
                    buyer,
                    seller,
                    resource: r,
                    quantity: q,
                    price: p,
                    ..
                } = source.event
                else {
                    panic!("trade memory without settled trade");
                };
                assert_eq!((resource, quantity, price), (r, q, p));
                assert_eq!(
                    (agent.id, counterparty),
                    if side == OrderSide::Buy {
                        (buyer, seller)
                    } else {
                        (seller, buyer)
                    }
                );
            }
        }
    }
    assert!(economic_count > 0);
}

#[test]
fn initial_goal_and_rejected_actions_are_remembered_without_inventing_production() {
    let mut sim = Simulation::new(SimulationConfig {
        agents: 1,
        ..Default::default()
    })
    .unwrap();
    let initial = sim.world().agent(AgentId(0)).unwrap();
    assert_eq!(
        initial.memories[0].content,
        MemoryContent::GoalAssigned { goal: initial.goal }
    );
    let report = sim
        .step_with_overrides(&BTreeMap::from([(
            AgentId(0),
            Decision {
                action: Action::Produce {
                    resource: Resource::Metal,
                },
                reason: DecisionReason::ProduceOutput,
            },
        )]))
        .unwrap();
    assert_eq!(report.stats.flows.rejected_actions, 1);
    let memories = &sim.world().agent(AgentId(0)).unwrap().memories;
    assert!(
        memories
            .iter()
            .any(|m| matches!(m.content, MemoryContent::ActionRejected { .. }))
    );
    assert!(
        !memories
            .iter()
            .any(|m| matches!(m.content, MemoryContent::Produced { .. }))
    );
    sim.step().unwrap();
}

#[test]
fn memory_configuration_is_validated_before_world_creation() {
    assert!(
        Simulation::new(SimulationConfig {
            memory_capacity: 1025,
            ..Default::default()
        })
        .is_err()
    );
}
