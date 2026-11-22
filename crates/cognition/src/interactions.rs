use super::{Observation, buy, decision, sell};
use agentsociety_core::*;

/// Review an objective from actual outcomes, with food-buffer hysteresis.
/// Wealth is compared at constant initial prices by the simulation, not from price inflation.
pub fn review_goal(agent: &Agent, evidence: &GoalAssessment) -> Option<(Goal, GoalChangeReason)> {
    if evidence.elapsed_ticks == 0 {
        return None;
    }
    let food = agent.account.available(Resource::Food);
    let choice = if food <= 2
        || agent.needs.hunger >= 40
        || evidence.hungry_ticks >= evidence.elapsed_ticks.div_ceil(4)
    {
        (Goal::SecureFood, GoalChangeReason::FoodInsecurity)
    } else if agent.goal == Goal::SecureFood && food < Goal::SecureFood.food_reserve() {
        return None;
    } else if evidence.wealth_declined {
        (Goal::BuildWealth, GoalChangeReason::WealthDecline)
    } else if agent.occupation == Occupation::Merchant && evidence.successful_trades >= 2 {
        (Goal::TradeProfit, GoalChangeReason::TradingSuccess)
    } else if food >= Goal::StableLife.food_reserve()
        && (agent.needs.comfort < 50 || evidence.cooperations > 0)
    {
        (Goal::StableLife, GoalChangeReason::StableHousehold)
    } else if agent.goal == Goal::SecureFood {
        (Goal::BuildWealth, GoalChangeReason::Recovery)
    } else {
        return None;
    };
    (choice.0 != agent.goal).then_some(choice)
}

pub(super) fn choose(obs: &Observation<'_>, baseline: &Decision) -> Option<Decision> {
    let context = obs.interactions?;
    let actor = obs.agent;
    let food = actor.account.available(Resource::Food);
    let urgent = food <= 3 || actor.needs.hunger >= 30;
    for proposal in &context.proposals {
        if proposal.proposer == actor.id
            && food < 2
            && matches!(
                proposal.terms,
                ProposalTerms::Trade {
                    side: OrderSide::Sell,
                    resource: Resource::Food,
                    ..
                }
            )
        {
            return Some(decision(
                Action::CancelProposal {
                    proposal_id: proposal.id,
                },
                DecisionReason::SecureFood,
            ));
        }
        if proposal.recipient != actor.id {
            continue;
        }
        let useful_food = matches!(
            proposal.terms,
            ProposalTerms::Trade {
                side: OrderSide::Sell,
                resource: Resource::Food,
                ..
            }
        );
        if urgent && !useful_food {
            continue;
        }
        let action = response(obs, proposal);
        return Some(decision(action, DecisionReason::RespondToProposal));
    }
    if urgent || actor.needs.energy < COOPERATIVE_LABOR || !context.proposals.is_empty() {
        return None;
    }
    let mut peers: Vec<_> = context
        .counterparties
        .iter()
        .filter(|peer| peer.open_proposals < MAX_OPEN_PROPOSALS)
        .filter(|peer| {
            actor
                .relationships
                .get(&peer.id)
                .is_none_or(|edge| edge.trust >= 0)
        })
        .collect();
    peers.sort_by_key(|peer| {
        (
            std::cmp::Reverse(
                actor
                    .relationships
                    .get(&peer.id)
                    .map_or(0, |edge| edge.trust),
            ),
            peer.id,
        )
    });
    if context.settings.cooperation
        && actor.occupation == Occupation::Producer
        && actor.account.available(Resource::Metal) >= COOPERATIVE_METAL
        && actor.account.available(Resource::Goods) <= 3
        && obs.tick.wrapping_add(actor.id.0).is_multiple_of(12)
        && let Some(peer) = peers.iter().find(|peer| {
            peer.energy >= COOPERATIVE_LABOR
                && peer.available_inventory.get(Resource::Wood) >= COOPERATIVE_WOOD
                && peer.available_inventory.get(Resource::Food) > 3
        })
    {
        return Some(decision(
            Action::Cooperate {
                partner: peer.id,
                producer_share: 2,
            },
            DecisionReason::JointProduction,
        ));
    }
    if !context.settings.negotiation || !obs.tick.wrapping_add(actor.id.0).is_multiple_of(8) {
        return None;
    }
    match baseline.action {
        Action::Buy {
            resource,
            quantity,
            limit_price,
        } => {
            let peer = peers.iter().find(|peer| surplus(peer, resource) > 0)?;
            let unit_price = (limit_price * 90 / 100).clamp(1, MAX_PRICE);
            let quantity = quantity.min(surplus(peer, resource));
            Some(decision(
                Action::Negotiate {
                    counterparty: peer.id,
                    side: OrderSide::Buy,
                    resource,
                    quantity,
                    unit_price,
                },
                DecisionReason::NegotiateTerms,
            ))
        }
        Action::Sell {
            resource,
            quantity,
            limit_price,
        } => {
            let unit_price = (limit_price * 110 / 100).clamp(1, MAX_PRICE);
            let peer = peers.iter().find(|peer| {
                demand(
                    peer.occupation,
                    peer.food_reserve,
                    &peer.available_inventory,
                    resource,
                ) > 0
                    && peer.available_cash >= unit_price
            })?;
            let quantity = quantity
                .min(demand(
                    peer.occupation,
                    peer.food_reserve,
                    &peer.available_inventory,
                    resource,
                ))
                .min(peer.available_cash / unit_price);
            Some(decision(
                Action::Negotiate {
                    counterparty: peer.id,
                    side: OrderSide::Sell,
                    resource,
                    quantity,
                    unit_price,
                },
                DecisionReason::NegotiateTerms,
            ))
        }
        _ => None,
    }
}

fn response(obs: &Observation<'_>, proposal: &Proposal) -> Action {
    let reject = || Action::RejectProposal {
        proposal_id: proposal.id,
    };
    let accept = || Action::AcceptProposal {
        proposal_id: proposal.id,
    };
    let actor = obs.agent;
    let (quantity, price, can_accept) = match proposal.terms {
        ProposalTerms::Cooperation { producer_share } => {
            let share = COOPERATIVE_GOODS - producer_share;
            return if actor.needs.energy >= COOPERATIVE_LABOR
                && actor.account.available(Resource::Wood) >= COOPERATIVE_WOOD
                && share * obs.prices[Resource::Goods.index()]
                    >= COOPERATIVE_WOOD * obs.prices[Resource::Wood.index()]
            {
                accept()
            } else {
                reject()
            };
        }
        ProposalTerms::Trade {
            side: OrderSide::Sell,
            resource,
            quantity,
            unit_price,
        } => {
            let available = Inventory::new(Resource::ALL.map(|r| actor.account.available(r)));
            let desired = demand(actor.occupation, actor.food_reserve(), &available, resource);
            let Some(Action::Buy {
                quantity: affordable,
                limit_price,
                ..
            }) = buy(
                obs,
                resource,
                desired,
                resource == Resource::Food,
                resource != Resource::Food,
            )
            else {
                return reject();
            };
            (
                quantity.min(affordable),
                limit_price,
                quantity <= affordable && unit_price <= limit_price,
            )
        }
        ProposalTerms::Trade {
            side: OrderSide::Buy,
            resource,
            quantity,
            unit_price,
        } => {
            let stock = actor.account.available(resource).saturating_sub(reserve(
                actor.occupation,
                actor.food_reserve(),
                resource,
            ));
            if stock == 0 {
                return reject();
            }
            let Action::Sell { limit_price, .. } = sell(
                obs,
                resource,
                stock,
                actor.occupation == Occupation::Merchant,
            ) else {
                unreachable!()
            };
            (
                quantity.min(stock),
                limit_price,
                quantity <= stock && unit_price >= limit_price,
            )
        }
    };
    if can_accept {
        accept()
    } else if quantity > 0
        && proposal.counteroffers < MAX_COUNTEROFFERS
        && proposal.expires_at > obs.tick
    {
        Action::CounterOffer {
            proposal_id: proposal.id,
            quantity,
            unit_price: price,
        }
    } else {
        reject()
    }
}

fn reserve(occupation: Occupation, food_target: u64, resource: Resource) -> u64 {
    match resource {
        Resource::Food => food_target,
        Resource::Goods => 1,
        Resource::Wood if occupation == Occupation::Producer => 4,
        Resource::Metal if occupation == Occupation::Producer => 2,
        _ => 0,
    }
}

fn surplus(peer: &CounterpartyView, resource: Resource) -> u64 {
    peer.available_inventory
        .get(resource)
        .saturating_sub(reserve(peer.occupation, peer.food_reserve, resource))
}

fn demand(occupation: Occupation, food_target: u64, stock: &Inventory, resource: Resource) -> u64 {
    let target = match resource {
        Resource::Food => food_target,
        Resource::Goods if occupation != Occupation::Producer => 3,
        Resource::Wood if occupation == Occupation::Producer => 12,
        Resource::Metal if occupation == Occupation::Producer => 6,
        Resource::Wood | Resource::Metal if occupation == Occupation::Merchant => 6,
        _ => 0,
    };
    target.saturating_sub(stock.get(resource))
}
