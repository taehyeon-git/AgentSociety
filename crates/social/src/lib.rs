//! Deterministic social feedback and graph analysis derived from canonical agents.

use agentsociety_core::*;
use agentsociety_world::World;
use petgraph::{algo::connected_components, graphmap::UnGraphMap};
use serde::{Deserialize, Serialize};

/// Summary of the current social graph; no separate mutable graph is persisted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SocialStats {
    /// Number of directed social relationships.
    pub directed_edges: usize,
    /// Mean trust, zero when there are no edges.
    pub mean_trust: f64,
    /// Connected groups joined by at least one trust score >= 20, including isolates.
    pub communities: usize,
}

/// Build a petgraph projection for community analysis or rendering.
pub fn trusted_graph<'a>(agents: impl IntoIterator<Item = &'a Agent>) -> UnGraphMap<AgentId, i16> {
    let agents: Vec<_> = agents.into_iter().collect();
    let mut graph = UnGraphMap::new();
    for agent in &agents {
        graph.add_node(agent.id);
    }
    for agent in agents {
        for (&other, edge) in &agent.relationships {
            if edge.trust >= 20 && graph.contains_node(other) {
                let weight = graph
                    .edge_weight(agent.id, other)
                    .copied()
                    .unwrap_or(0)
                    .max(edge.trust);
                graph.add_edge(agent.id, other, weight);
            }
        }
    }
    graph
}

/// Calculate network indicators from actual retained social relationships.
pub fn analyze<'a>(agents: impl IntoIterator<Item = &'a Agent>) -> SocialStats {
    let agents: Vec<_> = agents.into_iter().collect();
    let graph = trusted_graph(agents.iter().copied());
    let edges: Vec<_> = agents
        .iter()
        .flat_map(|a| a.relationships.values())
        .collect();
    SocialStats {
        directed_edges: edges.len(),
        mean_trust: if edges.is_empty() {
            0.0
        } else {
            edges.iter().map(|e| f64::from(e.trust)).sum::<f64>() / edges.len() as f64
        },
        communities: connected_components(&graph),
    }
}

/// Update a directed edge with bounded scores and deterministic oldest-edge eviction.
pub fn interact(
    agent: &mut Agent,
    other: AgentId,
    tick: Tick,
    trust_delta: i16,
    cooperation_delta: i16,
    capacity: usize,
) -> Vec<Event> {
    if capacity == 0 || agent.id == other {
        return Vec::new();
    }
    let mut events = Vec::new();
    if !agent.relationships.contains_key(&other) && agent.relationships.len() >= capacity {
        let evicted = agent
            .relationships
            .iter()
            .min_by_key(|(id, edge)| (edge.last_interaction, **id))
            .map(|(&id, _)| id)
            .expect("positive capacity and full relationships");
        agent.relationships.remove(&evicted);
        events.push(Event::RelationshipForgotten {
            agent_id: agent.id,
            other: evicted,
        });
    }
    let edge = agent.relationships.entry(other).or_default();
    edge.trust = edge.trust.saturating_add(trust_delta).clamp(-100, 100);
    edge.familiarity = edge.familiarity.saturating_add(2).min(100);
    edge.cooperation_score = edge
        .cooperation_score
        .saturating_add(cooperation_delta)
        .clamp(-100, 100);
    edge.interaction_count = edge.interaction_count.saturating_add(1);
    edge.last_interaction = tick;
    events.push(Event::RelationshipUpdated {
        agent_id: agent.id,
        other,
        relationship: edge.clone(),
    });
    events
}

/// Apply feedback from settled trades, completed cooperation and explicit proposal outcomes.
pub fn process(
    world: &mut World,
    events: &[Event],
    tick: Tick,
    capacity: usize,
) -> Result<Vec<Event>> {
    let mut updates = Vec::new();
    for event in events {
        if let Event::Organization {
            event: OrganizationEvent::Produced { workers, .. },
        } = event
        {
            for (index, &first) in workers.iter().enumerate() {
                for &second in &workers[index + 1..] {
                    world.with_pair_mut(first, second, |one, two| {
                        updates.extend(interact(one, two.id, tick, 4, 6, capacity));
                        updates.extend(interact(two, one.id, tick, 4, 6, capacity));
                        Ok(())
                    })?;
                }
            }
        }
        let interaction = match event {
            Event::Contract {
                event:
                    ContractEvent::Delivered {
                        representatives, ..
                    },
            } => Some((representatives[0], representatives[1], 4, 1, true)),
            Event::Contract {
                event:
                    ContractEvent::Closed {
                        representatives,
                        outcome: ContractOutcome::Defaulted,
                        ..
                    },
            } => Some((representatives[0], representatives[1], -12, -5, false)),
            Event::TradeExecuted { buyer, seller, .. }
            | Event::NegotiatedTrade { buyer, seller, .. } => Some((*buyer, *seller, 3, 0, true)),
            Event::CooperationCompleted {
                producer, partner, ..
            } => Some((*producer, *partner, 5, 10, true)),
            Event::ProposalClosed {
                proposal,
                outcome: ProposalOutcome::Rejected | ProposalOutcome::Expired,
            } => Some((proposal.proposer, proposal.recipient, -1, 0, false)),
            Event::ProposalClosed {
                proposal,
                outcome: ProposalOutcome::Cancelled,
            } => Some((proposal.recipient, proposal.proposer, -1, 0, false)),
            _ => None,
        };
        if let Some((first, second, trust, cooperation, reciprocal)) = interaction {
            world.with_pair_mut(first, second, |one, two| {
                updates.extend(interact(one, two.id, tick, trust, cooperation, capacity));
                if reciprocal {
                    updates.extend(interact(two, one.id, tick, trust, cooperation, capacity));
                }
                Ok(())
            })?;
        }
    }
    Ok(updates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_trade_builds_trust_and_a_connected_group() {
        let config = SimulationConfig {
            agents: 3,
            ..Default::default()
        };
        let mut world = World::seeded(&config).unwrap();
        let trade = Event::TradeExecuted {
            buy_order_id: 0,
            sell_order_id: 1,
            buyer: AgentId(0),
            seller: AgentId(1),
            resource: Resource::Food,
            quantity: 1,
            price: 100,
        };
        for tick in 1..=40 {
            process(&mut world, std::slice::from_ref(&trade), tick, 64).unwrap();
        }
        let edge = &world.agent(AgentId(0)).unwrap().relationships[&AgentId(1)];
        assert_eq!(edge.trust, 100);
        assert_eq!(edge.interaction_count, 40);
        assert_eq!(analyze(world.agents()).communities, 2);
    }

    #[test]
    fn edges_are_bounded_and_evicted_in_stable_order() {
        let mut agent = World::seeded(&SimulationConfig::default())
            .unwrap()
            .agent(AgentId(0))
            .unwrap()
            .clone();
        interact(&mut agent, AgentId(1), 1, 3, 0, 2);
        interact(&mut agent, AgentId(2), 1, 3, 0, 2);
        let events = interact(&mut agent, AgentId(3), 2, -5, 0, 2);
        assert!(matches!(
            events[0],
            Event::RelationshipForgotten {
                other: AgentId(1),
                ..
            }
        ));
        assert_eq!(agent.relationships.len(), 2);
        assert_eq!(agent.relationships[&AgentId(3)].trust, -5);
    }
}
