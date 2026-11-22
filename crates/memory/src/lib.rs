//! Bounded event-derived experiences and deterministic, selective retrieval.

use agentsociety_core::*;
use agentsociety_world::World;
use std::cmp::Reverse;

pub use agentsociety_core::MAX_RECALL_RESULTS;

/// Structured retrieval request. Category/age/importance are filters; tags add relevance.
#[derive(Debug, Clone)]
pub struct MemoryQuery {
    /// Current tick; future records are never returned.
    pub now: Tick,
    /// Restrict results to one category, if supplied.
    pub kind: Option<MemoryKind>,
    /// Prefer experiences involving this resource.
    pub resource: Option<Resource>,
    /// Prefer experiences involving this counterparty.
    pub related_agent: Option<AgentId>,
    /// Ignore memories below this importance.
    pub min_importance: u8,
    /// Ignore memories older than this many ticks, if supplied.
    pub max_age: Option<Tick>,
    /// Requested count, capped at [`MAX_RECALL_RESULTS`]. Zero returns nothing.
    pub limit: usize,
}

impl MemoryQuery {
    /// Recall up to eight contextually relevant experiences at a given tick.
    pub const fn new(now: Tick) -> Self {
        Self {
            now,
            kind: None,
            resource: None,
            related_agent: None,
            min_importance: 0,
            max_age: None,
            limit: 8,
        }
    }
}

/// A borrowed result with an integer score, suitable for inspection and context building.
#[derive(Debug)]
pub struct RecalledMemory<'a> {
    /// Selected experience; retrieval never changes stored memories.
    pub memory: &'a MemoryRecord,
    /// Importance + recency + relevant resource/counterparty contributions.
    pub score: u32,
}

/// Rank experiences with stable tie-breaking by most recent tick and source sequence.
pub fn recall<'a>(agent: &'a Agent, query: &MemoryQuery) -> Vec<RecalledMemory<'a>> {
    if query.limit == 0 {
        return Vec::new();
    }
    let mut matches: Vec<_> = agent
        .memories
        .iter()
        .filter_map(|memory| {
            let age = query.now.checked_sub(memory.tick)?;
            if memory.agent_id != agent.id
                || query.kind.is_some_and(|kind| memory.content.kind() != kind)
                || memory.importance < query.min_importance
                || query.max_age.is_some_and(|max| age > max)
            {
                return None;
            }
            let recency = 100 / (age.min(100) as u32 + 1);
            let resource_match =
                query.resource.is_some() && query.resource == memory.content.resource();
            let agent_match = query.related_agent.is_some()
                && query.related_agent == memory.content.related_agent();
            Some(RecalledMemory {
                memory,
                score: u32::from(memory.importance) * 2
                    + recency
                    + u32::from(resource_match) * 200
                    + u32::from(agent_match) * 250,
            })
        })
        .collect();
    matches.sort_by_key(|entry| Reverse((entry.score, entry.memory.tick, entry.memory.id)));
    matches.truncate(query.limit.min(MAX_RECALL_RESULTS));
    matches
}

/// Record experiences only from committed facts, in the journal's canonical order.
///
/// Each owner receives at most one memory per source event. Ordinary proposed
/// decisions and unfilled orders are not confused with successful outcomes.
/// Retention is FIFO per agent, independent from action and relationship histories.
pub fn record_events(world: &mut World, records: &[EventRecord], capacity: usize) -> Result<()> {
    if capacity == 0 {
        return Ok(());
    }
    for record in records {
        let mut remember = |id, importance, content| {
            world.with_agent_mut(id, |agent| {
                if agent
                    .memories
                    .back()
                    .is_some_and(|last| last.id.0 >= record.sequence)
                {
                    return Err(DomainError::Invariant(
                        "memory source events must be recorded once in sequence order".into(),
                    ));
                }
                while agent.memories.len() >= capacity {
                    agent.memories.pop_front();
                }
                agent.memories.push_back(MemoryRecord {
                    id: MemoryId(record.sequence),
                    agent_id: id,
                    tick: record.tick,
                    importance,
                    content,
                });
                Ok(())
            })
        };
        match &record.event {
            Event::Organization { event } => {
                for owner in record.event.participants() {
                    remember(
                        owner,
                        65,
                        MemoryContent::Organization {
                            event: event.clone(),
                        },
                    )?;
                }
            }
            Event::Contract { event } => {
                for owner in record.event.participants() {
                    remember(
                        owner,
                        75,
                        MemoryContent::Contract {
                            event: event.clone(),
                        },
                    )?;
                }
            }
            Event::Scenario { event } => {
                for owner in record.event.participants() {
                    remember(
                        owner,
                        60,
                        MemoryContent::Scenario {
                            event: event.clone(),
                        },
                    )?;
                }
            }
            Event::AgentCreated { agent_id, goal, .. } => {
                remember(*agent_id, 80, MemoryContent::GoalAssigned { goal: *goal })?
            }
            Event::GoalChanged {
                agent_id,
                previous,
                goal,
                reason,
                ..
            } => {
                remember(
                    *agent_id,
                    95,
                    MemoryContent::GoalChanged {
                        previous: *previous,
                        goal: *goal,
                        reason: *reason,
                    },
                )?;
            }
            Event::ProposalOpened { proposal }
            | Event::ProposalCountered { proposal }
            | Event::ProposalClosed { proposal, .. } => {
                let stage = match record.event {
                    Event::ProposalOpened { .. } => ProposalStage::Opened,
                    Event::ProposalCountered { .. } => ProposalStage::Countered,
                    Event::ProposalClosed { outcome, .. } => outcome.into(),
                    _ => unreachable!(),
                };
                for (owner, other) in [
                    (proposal.proposer, proposal.recipient),
                    (proposal.recipient, proposal.proposer),
                ] {
                    remember(
                        owner,
                        55,
                        MemoryContent::Proposal {
                            proposal_id: proposal.id,
                            other,
                            terms: proposal.terms.clone(),
                            stage,
                        },
                    )?;
                }
            }
            Event::CooperationCompleted {
                proposal_id,
                producer,
                partner,
                producer_share,
                partner_share,
            } => {
                for (owner, other, received_goods) in [
                    (*producer, *partner, *producer_share),
                    (*partner, *producer, *partner_share),
                ] {
                    remember(
                        owner,
                        80,
                        MemoryContent::Cooperated {
                            proposal_id: *proposal_id,
                            other,
                            received_goods,
                        },
                    )?;
                }
            }
            Event::ResourceProduced {
                agent_id,
                resource,
                quantity,
            } => remember(
                *agent_id,
                30,
                MemoryContent::Produced {
                    resource: *resource,
                    quantity: *quantity,
                },
            )?,
            Event::ResourceConsumed {
                agent_id,
                resource,
                quantity,
                purpose,
            } => remember(
                *agent_id,
                10,
                MemoryContent::Consumed {
                    resource: *resource,
                    quantity: *quantity,
                    purpose: *purpose,
                },
            )?,
            Event::AgentMoved { agent_id, from, to } => remember(
                *agent_id,
                20,
                MemoryContent::Moved {
                    from: *from,
                    to: *to,
                },
            )?,
            Event::ActionRejected { agent_id, reason } => remember(
                *agent_id,
                75,
                MemoryContent::ActionRejected {
                    reason: reason.clone(),
                },
            )?,
            Event::TradeExecuted {
                buyer,
                seller,
                resource,
                quantity,
                price,
                ..
            }
            | Event::NegotiatedTrade {
                buyer,
                seller,
                resource,
                quantity,
                price,
                ..
            } => {
                for (owner, counterparty, side) in [
                    (*buyer, *seller, OrderSide::Buy),
                    (*seller, *buyer, OrderSide::Sell),
                ] {
                    remember(
                        owner,
                        65,
                        MemoryContent::Trade {
                            side,
                            counterparty,
                            resource: *resource,
                            quantity: *quantity,
                            price: *price,
                        },
                    )?;
                }
            }
            Event::RelationshipUpdated {
                agent_id,
                other,
                relationship,
            } => remember(
                *agent_id,
                50,
                MemoryContent::RelationshipChanged {
                    other: *other,
                    relationship: relationship.clone(),
                },
            )?,
            Event::RelationshipForgotten { agent_id, other } => remember(
                *agent_id,
                25,
                MemoryContent::RelationshipForgotten { other: *other },
            )?,
            Event::ExternalDecisionRecorded { .. }
            | Event::DecisionRecorded { .. }
            | Event::OrderPlaced { .. }
            | Event::OrderExpired { .. }
            | Event::TickCompleted { .. } => {}
        }
    }
    Ok(())
}
