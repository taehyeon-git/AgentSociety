//! New social behavior remains deterministic across checkpoints, replay and memory limits.
use agentsociety_core::*;
use agentsociety_simulation::{PolicyRuntime, RunCheckpoint, Simulation};
use std::collections::BTreeMap;

fn config(agents: usize) -> SimulationConfig {
    SimulationConfig {
        agents,
        interactions: InteractionSettings {
            negotiation: true,
            cooperation: true,
            goal_review_interval: 20,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn scripted(
    sim: &mut Simulation,
    actions: &[(u64, Action)],
) -> agentsociety_simulation::TickReport {
    let mut decisions: BTreeMap<_, _> = sim
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
    for (id, action) in actions {
        decisions.get_mut(&AgentId(*id)).unwrap().action = action.clone();
    }
    sim.step_with_overrides(&decisions).unwrap()
}

fn buy(counterparty: u64) -> Action {
    Action::Negotiate {
        counterparty: AgentId(counterparty),
        side: OrderSide::Buy,
        resource: Resource::Wood,
        quantity: 2,
        unit_price: 70,
    }
}

#[test]
fn pending_trade_and_joint_work_restore_replay_and_record_actual_facts() {
    let mut sim = Simulation::new(config(4)).unwrap();
    let mut replay = Simulation::new(config(4)).unwrap();
    let report = scripted(
        &mut sim,
        &[
            (0, buy(2)),
            (
                1,
                Action::Cooperate {
                    partner: AgentId(3),
                    producer_share: 2,
                },
            ),
        ],
    );
    replay.replay_tick(&report.events).unwrap();
    assert_eq!(sim.proposals().proposals().count(), 2);
    assert_eq!(
        sim.world()
            .agent(AgentId(0))
            .unwrap()
            .account
            .reserved_cash(),
        140
    );
    assert_eq!(
        sim.world()
            .agent(AgentId(1))
            .unwrap()
            .account
            .reserved_inventory()
            .get(Resource::Metal),
        2
    );
    let checkpoint = RunCheckpoint::capture(&sim, &PolicyRuntime::default()).unwrap();
    let bytes = serde_json::to_vec(&checkpoint).unwrap();
    let (mut restored, _) = serde_json::from_slice::<RunCheckpoint>(&bytes)
        .unwrap()
        .restore()
        .unwrap();
    let report = scripted(
        &mut sim,
        &[
            (2, Action::AcceptProposal { proposal_id: 0 }),
            (3, Action::AcceptProposal { proposal_id: 1 }),
        ],
    );
    restored.replay_tick(&report.events).unwrap();
    replay.replay_tick(&report.events).unwrap();
    assert_eq!(sim.snapshot().unwrap(), restored.snapshot().unwrap());
    assert_eq!(sim.snapshot().unwrap(), replay.snapshot().unwrap());
    assert_eq!(report.stats.flows.negotiated_trades, 1);
    assert_eq!(report.stats.flows.cooperations, 1);
    assert_eq!(report.stats.flows.trade_count, 1);
    assert_eq!(report.stats.flows.production_inputs, [0, 4, 2, 0]);
    assert_eq!(report.stats.flows.produced, [0, 0, 0, 4]);
    assert_eq!(report.stats.prices[Resource::Wood.index()], 70);
    for (id, other, trust, cooperation) in
        [(0, 2, 3, 0), (2, 0, 3, 0), (1, 3, 5, 10), (3, 1, 5, 10)]
    {
        let agent = sim.world().agent(AgentId(id)).unwrap();
        let edge = &agent.relationships[&AgentId(other)];
        assert_eq!(
            (edge.trust, edge.cooperation_score, edge.interaction_count),
            (trust, cooperation, 1)
        );
        assert!(agent.memories.iter().any(|m| matches!(
            m.content,
            MemoryContent::Proposal {
                stage: ProposalStage::Accepted,
                ..
            }
        )));
        assert!(agent.memories.iter().any(|m| matches!(
            m.content,
            MemoryContent::Trade { .. } | MemoryContent::Cooperated { .. }
        )));
    }
}

#[test]
fn unanswered_proposals_expire_after_restart_without_fictitious_trade_memories() {
    let mut sim = Simulation::new(config(4)).unwrap();
    scripted(&mut sim, &[(0, buy(2))]);
    let mut restored = Simulation::from_snapshot(sim.snapshot().unwrap()).unwrap();
    for tick in 2..=6 {
        let report = scripted(&mut sim, &[]);
        restored.replay_tick(&report.events).unwrap();
        assert_eq!(sim.snapshot().unwrap(), restored.snapshot().unwrap());
        assert_eq!(sim.proposals().proposals().count(), usize::from(tick <= 5));
    }
    assert_eq!(
        sim.world()
            .agent(AgentId(0))
            .unwrap()
            .account
            .reserved_cash(),
        0
    );
    assert_eq!(sim.cumulative().trade_count, 0);
    for agent in sim.world().agents() {
        assert!(!agent.memories.iter().any(|m| matches!(
            m.content,
            MemoryContent::Trade { .. } | MemoryContent::Cooperated { .. }
        )));
    }
    assert_eq!(
        sim.world().agent(AgentId(0)).unwrap().relationships[&AgentId(2)].trust,
        -1
    );
}

#[test]
fn a_thousand_ticks_of_all_features_conserve_assets_and_resume_identically() {
    let mut sim = Simulation::new(config(100)).unwrap();
    let mut repeated = Simulation::new(config(100)).unwrap();
    let money = sim.stats().total_money;
    for _ in 0..500 {
        assert_eq!(sim.step().unwrap().events, repeated.step().unwrap().events);
    }
    let mut resumed = Simulation::from_snapshot(sim.snapshot().unwrap()).unwrap();
    for _ in 0..500 {
        assert_eq!(sim.step().unwrap().events, resumed.step().unwrap().events);
    }
    assert_eq!(sim.snapshot().unwrap(), resumed.snapshot().unwrap());
    assert_eq!(sim.stats().total_money, money);
    assert!(sim.cumulative().negotiations > 0);
    assert!(sim.cumulative().negotiated_trades > 0);
    assert!(sim.cumulative().cooperations > 0);
    assert!(sim.cumulative().goal_changes > 0);
    sim.proposals().audit_collateral(sim.world()).unwrap();
}

#[test]
fn memory_retention_does_not_change_goal_evidence_or_interaction_results() {
    let mut with_memory = Simulation::new(config(20)).unwrap();
    let mut without_memory = Simulation::new(SimulationConfig {
        memory_capacity: 0,
        ..config(20)
    })
    .unwrap();
    for _ in 0..120 {
        assert_eq!(
            with_memory.step().unwrap().events,
            without_memory.step().unwrap().events
        );
    }
    let mut expected = with_memory.snapshot().unwrap();
    expected.config.memory_capacity = 0;
    for agent in &mut expected.world.agents {
        agent.memories.clear();
    }
    assert_eq!(expected, without_memory.snapshot().unwrap());
    Simulation::from_snapshot(expected).unwrap();
}

#[test]
fn starvation_changes_goals_only_on_review_boundaries_and_records_the_evidence() {
    let mut sim = Simulation::new(config(4)).unwrap();
    let initial: BTreeMap<_, _> = sim.world().agents().map(|a| (a.id, a.goal)).collect();
    let mut changes = 0;
    for tick in 1..=40 {
        let report = scripted(&mut sim, &[]);
        for record in report.events {
            if let Event::GoalChanged {
                agent_id,
                previous,
                goal,
                reason,
                assessment,
            } = record.event
            {
                changes += 1;
                assert_eq!(tick % 20, 0);
                assert_ne!(previous, goal);
                assert_eq!(
                    (goal, reason),
                    (Goal::SecureFood, GoalChangeReason::FoodInsecurity)
                );
                assert_eq!(assessment.elapsed_ticks, 20);
                assert!(
                    sim.world()
                        .agent(agent_id)
                        .unwrap()
                        .memories
                        .iter()
                        .any(|m| m.id.0 == record.sequence)
                );
            }
        }
        if tick < 20 {
            assert!(sim.world().agents().all(|a| a.goal == initial[&a.id]));
        }
        Simulation::from_snapshot(sim.snapshot().unwrap()).unwrap();
    }
    assert!(changes > 0);
    assert!(sim.world().agents().all(|a| a.goal == Goal::SecureFood));
}

#[test]
fn malformed_live_collateral_and_adaptive_progress_are_rejected() {
    let mut sim = Simulation::new(config(4)).unwrap();
    scripted(&mut sim, &[(0, buy(2))]);
    let source = serde_json::to_value(sim.snapshot().unwrap()).unwrap();
    for (path, value) in [
        ("/proposals/next_id", serde_json::json!(0)),
        ("/cumulative/negotiations", serde_json::json!(0)),
        ("/cumulative/negotiated_trades", serde_json::json!(1)),
        ("/cumulative/cooperations", serde_json::json!(1)),
        ("/cumulative/goal_changes", serde_json::json!(1)),
        ("/proposals/proposals/0/expires_at", serde_json::json!(99)),
        ("/proposals/proposals/0/recipient", serde_json::json!(0)),
        (
            "/world/agents/0/account/reserved_cash",
            serde_json::json!(139),
        ),
        (
            "/world/agents/0/goal_progress/last_review",
            serde_json::json!(1),
        ),
        (
            "/world/agents/0/goal_progress/hungry_ticks",
            serde_json::json!(2),
        ),
        (
            "/world/agents/0/goal_progress/reference_wealth",
            serde_json::json!(0),
        ),
    ] {
        let mut altered = source.clone();
        *altered.pointer_mut(path).unwrap() = value;
        assert!(
            Simulation::from_snapshot(serde_json::from_value(altered).unwrap()).is_err(),
            "accepted {path}"
        );
    }
}

#[test]
fn independent_features_short_deadlines_and_empty_or_penniless_worlds_restore() {
    for (negotiation, cooperation, goal_review_interval) in [
        (true, false, 0),
        (false, true, 0),
        (false, false, 1),
        (true, true, 7),
    ] {
        for (agents, initial_cash) in [(0, 0), (1, 0), (12, 0), (12, 10000)] {
            let mut sim = Simulation::new(SimulationConfig {
                agents,
                initial_cash,
                memory_capacity: 0,
                relationship_capacity: 0,
                interactions: InteractionSettings {
                    negotiation,
                    cooperation,
                    goal_review_interval,
                    offer_lifetime_ticks: 1,
                },
                ..Default::default()
            })
            .unwrap();
            for tick in 1..=60 {
                sim.step().unwrap();
                if tick % 13 == 0 {
                    sim = Simulation::from_snapshot(sim.snapshot().unwrap()).unwrap();
                }
            }
            if !negotiation {
                assert_eq!(sim.cumulative().negotiations, 0);
            }
            if !cooperation {
                assert_eq!(sim.cumulative().cooperations, 0);
            }
            if goal_review_interval == 0 {
                assert_eq!(sim.cumulative().goal_changes, 0);
            }
        }
    }
}
