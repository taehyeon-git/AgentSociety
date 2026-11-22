//! Experience retrieval and event projection contracts, independent of the GUI.

use agentsociety_core::*;
use agentsociety_memory::{MAX_RECALL_RESULTS, MemoryQuery, recall, record_events};
use agentsociety_world::World;

fn world() -> World {
    World::seeded(&SimulationConfig {
        agents: 3,
        ..Default::default()
    })
    .unwrap()
}

fn record(sequence: u64, tick: Tick, event: Event) -> EventRecord {
    EventRecord {
        sequence,
        tick,
        event,
    }
}

fn trade(sequence: u64, tick: Tick) -> EventRecord {
    record(
        sequence,
        tick,
        Event::TradeExecuted {
            buy_order_id: 0,
            sell_order_id: 1,
            buyer: AgentId(0),
            seller: AgentId(1),
            resource: Resource::Food,
            quantity: 2,
            price: 97,
        },
    )
}

#[test]
fn settled_trade_has_distinct_buyer_and_seller_perspectives_and_a_shared_source() {
    let mut world = world();
    let accounts: Vec<_> = world.agents().map(|a| a.account.clone()).collect();
    record_events(&mut world, &[trade(7, 4)], 64).unwrap();
    for (owner, other, side) in [(0, 1, OrderSide::Buy), (1, 0, OrderSide::Sell)] {
        let memories = &world.agent(AgentId(owner)).unwrap().memories;
        assert_eq!(memories.len(), 1);
        let memory = &memories[0];
        assert_eq!(memory.id, MemoryId(7));
        assert_eq!(memory.agent_id, AgentId(owner));
        assert_eq!(memory.tick, 4);
        assert_eq!(
            memory.content,
            MemoryContent::Trade {
                side,
                counterparty: AgentId(other),
                resource: Resource::Food,
                quantity: 2,
                price: 97,
            }
        );
    }
    assert!(world.agent(AgentId(2)).unwrap().memories.is_empty());
    assert_eq!(
        accounts,
        world
            .agents()
            .map(|a| a.account.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn unfilled_orders_and_proposed_decisions_are_not_remembered_as_successes() {
    let mut world = world();
    let events = [
        record(
            0,
            1,
            Event::DecisionRecorded {
                agent_id: AgentId(0),
                origin: DecisionOrigin::ExternalInput,
                decision: Decision {
                    action: Action::Wait {},
                    reason: DecisionReason::NoOpportunity,
                },
            },
        ),
        record(
            1,
            1,
            Event::OrderPlaced {
                order_id: 0,
                agent_id: AgentId(0),
                side: OrderSide::Buy,
                resource: Resource::Food,
                quantity: 2,
                limit_price: 1,
            },
        ),
        record(
            2,
            1,
            Event::OrderExpired {
                order_id: 0,
                agent_id: AgentId(0),
                remaining: 2,
            },
        ),
        record(
            3,
            1,
            Event::ActionRejected {
                agent_id: AgentId(0),
                reason: "not affordable".into(),
            },
        ),
    ];
    record_events(&mut world, &events, 64).unwrap();
    let agent = world.agent(AgentId(0)).unwrap();
    assert_eq!(agent.memories.len(), 1);
    assert_eq!(
        agent.memories[0].content,
        MemoryContent::ActionRejected {
            reason: "not affordable".into()
        }
    );
}

#[test]
fn all_four_categories_are_derived_from_facts_and_roundtrip() {
    let mut world = world();
    let events = [
        record(
            0,
            0,
            Event::AgentCreated {
                agent_id: AgentId(0),
                occupation: Occupation::Farmer,
                goal: Goal::SecureFood,
            },
        ),
        record(
            1,
            1,
            Event::ResourceProduced {
                agent_id: AgentId(0),
                resource: Resource::Food,
                quantity: 4,
            },
        ),
        trade(2, 1),
        record(
            3,
            1,
            Event::RelationshipUpdated {
                agent_id: AgentId(0),
                other: AgentId(1),
                relationship: Relationship {
                    trust: 3,
                    familiarity: 2,
                    interaction_count: 1,
                    last_interaction: 1,
                    ..Default::default()
                },
            },
        ),
    ];
    record_events(&mut world, &events, 64).unwrap();
    let agent = world.agent(AgentId(0)).unwrap();
    assert_eq!(
        agent
            .memories
            .iter()
            .map(|m| m.content.kind())
            .collect::<Vec<_>>(),
        [
            MemoryKind::Goal,
            MemoryKind::Episodic,
            MemoryKind::Economic,
            MemoryKind::Social
        ]
    );
    let encoded = serde_json::to_vec(&agent.memories).unwrap();
    let decoded: std::collections::VecDeque<MemoryRecord> =
        serde_json::from_slice(&encoded).unwrap();
    assert_eq!(agent.memories, decoded);
}

#[test]
fn retention_is_bounded_per_owner_and_can_be_disabled() {
    let mut world = world();
    for sequence in 0..20 {
        record_events(&mut world, &[trade(sequence, sequence)], 3).unwrap();
    }
    for agent in world.agents().take(2) {
        assert_eq!(
            agent.memories.iter().map(|m| m.id.0).collect::<Vec<_>>(),
            [17, 18, 19]
        );
        assert!(agent.history.is_empty());
    }
    let before = world.snapshot();
    record_events(&mut world, &[trade(20, 20)], 0).unwrap();
    assert_eq!(world.snapshot(), before);
}

#[test]
fn duplicate_source_is_rejected_instead_of_creating_ambiguous_ids() {
    let mut world = world();
    record_events(&mut world, &[trade(2, 1)], 4).unwrap();
    let before = world.snapshot();
    assert!(record_events(&mut world, &[trade(2, 1)], 4).is_err());
    assert_eq!(world.snapshot(), before);
}

fn push_memory(
    agent: &mut Agent,
    sequence: u64,
    tick: Tick,
    importance: u8,
    content: MemoryContent,
) {
    agent.memories.push_back(MemoryRecord {
        id: MemoryId(sequence),
        agent_id: agent.id,
        tick,
        importance,
        content,
    });
}

#[test]
fn relevant_resource_and_counterparty_can_outweigh_a_newer_unrelated_experience() {
    let world = world();
    let mut agent = world.agent(AgentId(0)).unwrap().clone();
    push_memory(
        &mut agent,
        0,
        1,
        65,
        MemoryContent::Trade {
            side: OrderSide::Buy,
            counterparty: AgentId(1),
            resource: Resource::Food,
            quantity: 2,
            price: 100,
        },
    );
    push_memory(
        &mut agent,
        1,
        100,
        80,
        MemoryContent::GoalAssigned {
            goal: Goal::BuildWealth,
        },
    );
    let mut query = MemoryQuery::new(100);
    assert_eq!(recall(&agent, &query)[0].memory.id, MemoryId(1));
    query.resource = Some(Resource::Food);
    assert_eq!(recall(&agent, &query)[0].memory.id, MemoryId(0));
    query.resource = None;
    query.related_agent = Some(AgentId(1));
    assert_eq!(recall(&agent, &query)[0].memory.id, MemoryId(0));
}

#[test]
fn importance_recency_and_stable_source_order_determine_ranking() {
    let world = world();
    let mut agent = world.agent(AgentId(0)).unwrap().clone();
    for (sequence, tick, importance) in [(0, 1, 10), (1, 1, 90), (2, 4, 90), (3, 4, 90)] {
        push_memory(
            &mut agent,
            sequence,
            tick,
            importance,
            MemoryContent::Produced {
                resource: Resource::Food,
                quantity: 1,
            },
        );
    }
    let before = agent.memories.clone();
    assert_eq!(
        recall(&agent, &MemoryQuery::new(5))
            .iter()
            .map(|e| e.memory.id.0)
            .collect::<Vec<_>>(),
        [3, 2, 1, 0]
    );
    assert_eq!(
        agent.memories, before,
        "retrieval must not reorder stored facts"
    );
}

#[test]
fn filters_exclude_future_foreign_old_and_unimportant_memories() {
    let world = world();
    let mut agent = world.agent(AgentId(0)).unwrap().clone();
    for (sequence, tick, importance) in
        [(0, 1, 90), (1, 9, 10), (2, 9, 90), (3, 11, 90), (4, 9, 90)]
    {
        push_memory(
            &mut agent,
            sequence,
            tick,
            importance,
            MemoryContent::Produced {
                resource: Resource::Food,
                quantity: 1,
            },
        );
    }
    agent.memories.back_mut().unwrap().agent_id = AgentId(1);
    let mut query = MemoryQuery::new(10);
    query.max_age = Some(2);
    query.min_importance = 20;
    assert_eq!(
        recall(&agent, &query)
            .iter()
            .map(|e| e.memory.id.0)
            .collect::<Vec<_>>(),
        [2]
    );
    query.kind = Some(MemoryKind::Economic);
    assert!(recall(&agent, &query).is_empty());
}

#[test]
fn recall_has_a_hard_context_limit_and_handles_maximum_tick_without_overflow() {
    let mut world = world();
    for sequence in 0..50 {
        record_events(&mut world, &[trade(sequence, 0)], 64).unwrap();
    }
    let agent = world.agent(AgentId(0)).unwrap();
    let mut query = MemoryQuery::new(u64::MAX);
    query.limit = usize::MAX;
    assert_eq!(recall(agent, &query).len(), MAX_RECALL_RESULTS);
    query.limit = 0;
    assert!(recall(agent, &query).is_empty());
}
