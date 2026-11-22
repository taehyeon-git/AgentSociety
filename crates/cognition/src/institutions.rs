use super::{Observation, decision};
use agentsociety_core::*;

fn intent(operation: InstitutionAction) -> Decision {
    let reason = if matches!(
        operation,
        InstitutionAction::OfferContract { .. }
            | InstitutionAction::AcceptContract { .. }
            | InstitutionAction::RejectContract { .. }
            | InstitutionAction::CancelContract { .. }
    ) {
        DecisionReason::RecurringAgreement
    } else {
        DecisionReason::CollectiveOrganization
    };
    decision(Action::Institution { operation }, reason)
}

pub(super) fn choose(obs: &Observation<'_>, _baseline: &Decision) -> Option<Decision> {
    let context = obs.institutions?;
    let actor = obs.agent;
    let food = actor.account.available(Resource::Food);
    if actor.needs.energy < 6 || actor.needs.hunger >= 30 || food <= actor.food_reserve() / 2 {
        return None;
    }
    let managed = context.own.as_ref().filter(|g| g.leader == actor.id);
    for contract in &context.contracts {
        if contract.accepted_at.is_some() {
            continue;
        }
        let recipient = contract.recipient == Party::Agent(actor.id)
            || managed.is_some_and(|g| contract.recipient == Party::Organization(g.id));
        if !recipient {
            continue;
        }
        let group = managed.filter(|g| contract.recipient == Party::Organization(g.id));
        let (cash, inventory) = group.map_or(
            (
                actor.account.available_cash(),
                Inventory::new(Resource::ALL.map(|r| actor.account.available(r))),
            ),
            |g| (g.available_cash, g.available_inventory),
        );
        let total = contract.outstanding().ok()?;
        let reference = obs.prices[contract.resource.index()];
        let fair = u128::from(contract.unit_price) * 100 >= u128::from(reference) * 75
            && u128::from(contract.unit_price) * 100 <= u128::from(reference) * 125;
        let buyer = contract.buyer() == contract.recipient;
        let useful = match contract.resource {
            Resource::Food => group.is_none() && food < actor.food_reserve() * 2,
            Resource::Goods => group.is_none() && inventory.get(Resource::Goods) < 4,
            Resource::Wood | Resource::Metal => {
                group.is_some() || actor.occupation == Occupation::Producer
            }
        };
        let surplus = inventory.get(contract.resource).saturating_sub(
            if contract.resource == Resource::Food {
                actor.food_reserve()
            } else {
                0
            },
        );
        let funded = if buyer {
            useful && cash / 2 >= total
        } else {
            cash >= contract.bond && surplus >= contract.quantity * u64::from(contract.deliveries)
        };
        return Some(intent(if fair && funded {
            InstitutionAction::AcceptContract {
                contract_id: contract.id,
            }
        } else {
            InstitutionAction::RejectContract {
                contract_id: contract.id,
            }
        }));
    }
    let turn = obs.tick.wrapping_add(actor.id.0);
    if context.own.is_none()
        && turn.is_multiple_of(4)
        && actor.account.available_cash()
            >= context
                .settings
                .membership_stake
                .saturating_add(obs.prices[0].saturating_mul(8))
    {
        if let Some(group) = context
            .organizations
            .iter()
            .filter(|g| g.members.len() < context.settings.max_members.min(6) && g.contracts == 0)
            .max_by_key(|g| (g.members.len(), std::cmp::Reverse(g.id)))
        {
            return Some(intent(InstitutionAction::Join {
                organization: group.id,
            }));
        }
        if actor.occupation == Occupation::Producer && turn.is_multiple_of(8) {
            return Some(intent(InstitutionAction::Found {}));
        }
    }
    if let Some(group) = &context.own {
        let count = group.members.len() as u64;
        if group.leader == actor.id && turn.is_multiple_of(6) {
            let goods = (group.available_inventory.get(Resource::Goods) / count).min(2);
            let cash = group
                .available_cash
                .saturating_sub(context.settings.membership_stake * count * 2)
                / count;
            if goods > 0 || cash > 0 {
                return Some(intent(InstitutionAction::Distribute {
                    organization: group.id,
                    cash_per_member: cash.min(1000),
                    goods_per_member: goods,
                }));
            }
        }
        if group.members.len() >= 3 && turn.is_multiple_of(6) {
            let mut stock = Inventory::default();
            for (resource, target) in [(Resource::Wood, 12_u64), (Resource::Metal, 6_u64)] {
                let amount = target
                    .saturating_sub(group.available_inventory.get(resource))
                    .min(actor.account.available(resource))
                    .min(4);
                stock.add(resource, amount).ok()?;
            }
            if stock != Inventory::default() {
                return Some(intent(InstitutionAction::Contribute {
                    organization: group.id,
                    cash: 0,
                    inventory: stock,
                }));
            }
            if actor.occupation != Occupation::Farmer {
                let resource = if group.available_inventory.get(Resource::Wood) < 4 {
                    Some(Resource::Wood)
                } else if group.available_inventory.get(Resource::Metal) < 2 {
                    Some(Resource::Metal)
                } else {
                    None
                };
                if let Some(resource) = resource {
                    return Some(decision(
                        Action::Gather { resource },
                        DecisionReason::CollectiveOrganization,
                    ));
                }
            }
        }
        if group.leader == actor.id
            && group.members.len() >= 3
            && group.contracts < MAX_CONTRACTS
            && turn.is_multiple_of(8)
        {
            for resource in [Resource::Wood, Resource::Metal, Resource::Goods] {
                let selling = resource == Resource::Goods;
                if selling && group.available_inventory.get(resource) < count + 4 {
                    continue;
                }
                if !selling && group.available_inventory.get(resource) >= 8 {
                    continue;
                }
                let price = obs.prices[resource.index()];
                if (!selling && group.available_cash / 2 < price.saturating_mul(4))
                    || (selling && group.available_cash < price.saturating_mul(4).div_ceil(10))
                {
                    continue;
                }
                let peer = context.counterparties.iter().find(|p| {
                    !group.members.contains(&p.id)
                        && p.open_contracts < MAX_CONTRACTS
                        && !context.contracts.iter().any(|c| {
                            c.proposer == Party::Agent(p.id) || c.recipient == Party::Agent(p.id)
                        })
                        && actor.relationships.get(&p.id).is_none_or(|r| r.trust >= 0)
                        && if selling {
                            p.available_cash / 2 >= price.saturating_mul(4)
                                && p.available_inventory.get(resource) < 4
                        } else {
                            p.available_inventory.get(resource) >= 4
                                && p.available_cash >= price.saturating_mul(4).div_ceil(10)
                        }
                });
                if let Some(peer) = peer {
                    return Some(intent(InstitutionAction::OfferContract {
                        organization: Some(group.id),
                        counterparty: Party::Agent(peer.id),
                        side: if selling {
                            OrderSide::Sell
                        } else {
                            OrderSide::Buy
                        },
                        resource,
                        quantity: 1,
                        unit_price: price,
                        deliveries: 4,
                        interval_ticks: 4,
                    }));
                }
            }
        }
    }
    if turn.is_multiple_of(16)
        && context
            .contracts
            .iter()
            .filter(|c| {
                c.proposer == Party::Agent(actor.id) || c.recipient == Party::Agent(actor.id)
            })
            .count()
            < MAX_CONTRACTS
        && food < actor.food_reserve()
    {
        let price = obs.prices[Resource::Food.index()];
        if actor.account.available_cash() / 2 >= price.saturating_mul(4) {
            let peer = context.counterparties.iter().find(|p| {
                p.available_inventory.get(Resource::Food) >= p.food_reserve + 4
                    && p.available_cash >= price.saturating_mul(4).div_ceil(10)
                    && p.open_contracts < MAX_CONTRACTS
                    && !context.contracts.iter().any(|c| {
                        c.proposer == Party::Agent(p.id) || c.recipient == Party::Agent(p.id)
                    })
            });
            if let Some(peer) = peer {
                return Some(intent(InstitutionAction::OfferContract {
                    organization: None,
                    counterparty: Party::Agent(peer.id),
                    side: OrderSide::Buy,
                    resource: Resource::Food,
                    quantity: 1,
                    unit_price: price,
                    deliveries: 4,
                    interval_ticks: 2,
                }));
            }
        }
    }
    None
}
