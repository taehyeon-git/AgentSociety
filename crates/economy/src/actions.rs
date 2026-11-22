use crate::{OrderBook, validate_order_values};
use agentsociety_core::*;
use agentsociety_world::World;

const LABOR_COST: u8 = 5;
const FACTORY_INPUTS: [(Resource, u64); 2] = [(Resource::Wood, 4), (Resource::Metal, 2)];

/// Compute a productivity-dependent output batch, with a hunger penalty.
pub fn production_quantity(agent: &Agent, resource: Resource) -> Result<u64> {
    let base = match resource {
        Resource::Food if agent.occupation == Occupation::Farmer => {
            10 + u64::from(agent.skills.farming) / 20
        }
        Resource::Food => 3,
        Resource::Goods if agent.occupation == Occupation::Producer => {
            2 + u64::from(agent.skills.manufacturing) / 80
        }
        _ => {
            return Err(DomainError::InvalidAction(
                "occupation cannot produce this resource".into(),
            ));
        }
    };
    Ok(if agent.needs.hunger >= 80 {
        (base / 2).max(1)
    } else {
        base
    })
}

/// Compute forestry or mining output; these actions are available to all roles.
pub fn gathering_quantity(agent: &Agent, resource: Resource) -> Result<u64> {
    let base = match resource {
        Resource::Wood => 4 + u64::from(agent.skills.gathering) / 25,
        Resource::Metal => 2 + u64::from(agent.skills.gathering) / 40,
        _ => {
            return Err(DomainError::InvalidAction(
                "only wood and metal can be gathered".into(),
            ));
        }
    };
    Ok(if agent.needs.hunger >= 80 {
        (base / 2).max(1)
    } else {
        base
    })
}

/// Validate an intent without mutating state; application repeats this check.
pub fn validate_action(world: &World, id: AgentId, action: &Action) -> Result<()> {
    let agent = world.agent(id)?;
    agent.account.validate()?;
    match *action {
        Action::Produce { resource } | Action::Gather { resource } => {
            if agent.needs.energy < LABOR_COST {
                return Err(DomainError::InvalidAction("insufficient energy".into()));
            }
            let quantity = if matches!(action, Action::Produce { .. }) {
                production_quantity(agent, resource)?
            } else {
                gathering_quantity(agent, resource)?
            };
            if resource == Resource::Goods {
                for (input, amount) in FACTORY_INPUTS {
                    if agent.account.available(input) < amount {
                        return Err(DomainError::InsufficientInventory(input));
                    }
                }
            } else if world.resource_pool().available.get(resource) < quantity {
                return Err(DomainError::InsufficientInventory(resource));
            }
            agent
                .account
                .inventory()
                .get(resource)
                .checked_add(quantity)
                .ok_or(DomainError::Overflow("production output"))?;
        }
        Action::Buy {
            resource: _,
            quantity,
            limit_price,
        } => {
            validate_order_values(quantity, limit_price)?;
            if agent.account.available_cash() < notional(limit_price, quantity)? {
                return Err(DomainError::InsufficientFunds);
            }
        }
        Action::Sell {
            resource,
            quantity,
            limit_price,
        } => {
            validate_order_values(quantity, limit_price)?;
            if agent.account.available(resource) < quantity {
                return Err(DomainError::InsufficientInventory(resource));
            }
        }
        Action::Consume { resource, quantity } => {
            if quantity == 0
                || quantity > MAX_ORDER_QUANTITY
                || !matches!(resource, Resource::Food | Resource::Goods)
            {
                return Err(DomainError::InvalidAction(
                    "only positive food/goods quantities can be consumed".into(),
                ));
            }
            if agent.account.available(resource) < quantity {
                return Err(DomainError::InsufficientInventory(resource));
            }
        }
        Action::Move { destination } => {
            if !world.contains_position(destination) || agent.needs.energy == 0 {
                return Err(DomainError::InvalidAction(
                    "invalid destination or insufficient energy".into(),
                ));
            }
        }
        Action::Rest {} | Action::Wait {} => {}
        _ => {
            return Err(DomainError::InvalidAction(
                "interaction requires the proposal book".into(),
            ));
        }
    }
    Ok(())
}

/// Validate and apply one intent. Every rejection leaves its account unchanged.
pub fn apply_action(
    world: &mut World,
    market: &mut OrderBook,
    id: AgentId,
    action: &Action,
    tick: Tick,
) -> Result<Vec<Event>> {
    validate_action(world, id, action)?;
    let mut events = Vec::new();
    match *action {
        Action::Produce { resource } | Action::Gather { resource } => {
            let quantity = if matches!(action, Action::Produce { .. }) {
                production_quantity(world.agent(id)?, resource)?
            } else {
                gathering_quantity(world.agent(id)?, resource)?
            };
            if resource == Resource::Goods {
                world.with_agent_mut(id, |agent| {
                    agent.account.transform(&FACTORY_INPUTS, resource, quantity)
                })?;
                for (input, amount) in FACTORY_INPUTS {
                    events.push(Event::ResourceConsumed {
                        agent_id: id,
                        resource: input,
                        quantity: amount,
                        purpose: ConsumptionPurpose::ProductionInput,
                    });
                }
            } else {
                world.extract(id, resource, quantity)?;
            }
            world.with_agent_mut(id, |agent| {
                agent.needs.energy -= LABOR_COST;
                Ok(())
            })?;
            events.push(Event::ResourceProduced {
                agent_id: id,
                resource,
                quantity,
            });
        }
        Action::Buy {
            resource,
            quantity,
            limit_price,
        } => {
            events.push(market.place(
                world,
                id,
                OrderSide::Buy,
                resource,
                quantity,
                limit_price,
                tick,
            )?);
        }
        Action::Sell {
            resource,
            quantity,
            limit_price,
        } => {
            events.push(market.place(
                world,
                id,
                OrderSide::Sell,
                resource,
                quantity,
                limit_price,
                tick,
            )?);
        }
        Action::Consume { resource, quantity } => {
            world.with_agent_mut(id, |agent| {
                agent.account.consume(resource, quantity)?;
                satisfy(agent, resource, quantity);
                Ok(())
            })?;
            events.push(Event::ResourceConsumed {
                agent_id: id,
                resource,
                quantity,
                purpose: ConsumptionPurpose::Household,
            });
        }
        Action::Move { destination } => {
            let from = world.agent(id)?.position;
            world.with_agent_mut(id, |agent| {
                agent.position = destination;
                agent.needs.energy -= 1;
                Ok(())
            })?;
            events.push(Event::AgentMoved {
                agent_id: id,
                from,
                to: destination,
            });
        }
        Action::Rest {} => {
            world.with_agent_mut(id, |agent| {
                agent.needs.energy = agent.needs.energy.saturating_add(25).min(100);
                Ok(())
            })?;
        }
        Action::Wait {} => {}
        _ => {
            return Err(DomainError::InvalidAction(
                "interaction requires the proposal book".into(),
            ));
        }
    }
    Ok(events)
}

fn satisfy(agent: &mut Agent, resource: Resource, quantity: u64) {
    let relief = (quantity.min(5) * 20) as u8;
    match resource {
        Resource::Food => agent.needs.hunger = agent.needs.hunger.saturating_sub(relief),
        Resource::Goods => {
            agent.needs.comfort = agent.needs.comfort.saturating_add(relief).min(100)
        }
        Resource::Wood | Resource::Metal => {}
    }
}

/// Apply household consumption after settlement and order expiry.
///
/// Agents eat one food unit each tick when available. Goods are consumed every
/// eighth tick with an ID offset, preventing synchronized demand spikes.
pub fn update_needs(world: &mut World, id: AgentId, tick: Tick) -> Result<Vec<Event>> {
    world.with_agent_mut(id, |agent| {
        let mut events = Vec::with_capacity(2);
        agent.needs.energy = agent.needs.energy.saturating_add(1).min(100);
        agent.needs.comfort = agent.needs.comfort.saturating_sub(2);
        if agent.account.available(Resource::Food) > 0 {
            agent.account.consume(Resource::Food, 1)?;
            satisfy(agent, Resource::Food, 1);
            events.push(Event::ResourceConsumed {
                agent_id: id,
                resource: Resource::Food,
                quantity: 1,
                purpose: ConsumptionPurpose::Household,
            });
        } else {
            agent.needs.hunger = agent.needs.hunger.saturating_add(10).min(100);
        }
        if tick % 8 == id.0 % 8
            && agent.needs.comfort <= 80
            && agent.account.available(Resource::Goods) > 0
        {
            agent.account.consume(Resource::Goods, 1)?;
            satisfy(agent, Resource::Goods, 1);
            events.push(Event::ResourceConsumed {
                agent_id: id,
                resource: Resource::Goods,
                quantity: 1,
                purpose: ConsumptionPurpose::Household,
            });
        }
        Ok(events)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_factory_input_cannot_be_used_twice() {
        let config = SimulationConfig {
            agents: 2,
            ..Default::default()
        };
        let mut world = World::seeded(&config).unwrap();
        let mut market = OrderBook::new(config.initial_prices).unwrap();
        market
            .place(
                &mut world,
                AgentId(1),
                OrderSide::Sell,
                Resource::Wood,
                8,
                70,
                1,
            )
            .unwrap();
        let before = world.snapshot();
        assert!(
            apply_action(
                &mut world,
                &mut market,
                AgentId(1),
                &Action::Produce {
                    resource: Resource::Goods
                },
                1
            )
            .is_err()
        );
        assert_eq!(world.snapshot(), before);
    }

    #[test]
    fn factory_logs_inputs_separately_from_output() {
        let config = SimulationConfig {
            agents: 2,
            ..Default::default()
        };
        let mut world = World::seeded(&config).unwrap();
        let mut market = OrderBook::new(config.initial_prices).unwrap();
        let events = apply_action(
            &mut world,
            &mut market,
            AgentId(1),
            &Action::Produce {
                resource: Resource::Goods,
            },
            1,
        )
        .unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[0],
            Event::ResourceConsumed {
                resource: Resource::Wood,
                quantity: 4,
                purpose: ConsumptionPurpose::ProductionInput,
                ..
            }
        ));
        assert_eq!(
            world
                .agent(AgentId(1))
                .unwrap()
                .account
                .inventory()
                .get(Resource::Wood),
            4
        );
    }

    #[test]
    fn unsupported_actions_are_rejected_without_mutation() {
        let config = SimulationConfig {
            agents: 1,
            ..Default::default()
        };
        let mut world = World::seeded(&config).unwrap();
        let mut market = OrderBook::new(config.initial_prices).unwrap();
        let before = world.snapshot();
        for action in [
            Action::Produce {
                resource: Resource::Metal,
            },
            Action::Gather {
                resource: Resource::Food,
            },
            Action::Consume {
                resource: Resource::Wood,
                quantity: 1,
            },
            Action::Move {
                destination: Position { x: 100, y: 0 },
            },
        ] {
            assert!(apply_action(&mut world, &mut market, AgentId(0), &action, 1).is_err());
        }
        assert_eq!(world.snapshot(), before);
    }
}
