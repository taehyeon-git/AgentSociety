//! Pure decision selection. Policies cannot mutate accounts or execute actions.

use agentsociety_core::*;

mod institutions;
mod interactions;
mod settings;
pub use interactions::review_goal;
pub use settings::{LlmSettings, Provider};

#[cfg(feature = "llm")]
pub mod llm;

/// Bounded, read-only information available to a decision policy.
pub struct Observation<'a> {
    /// Acting agent's state at the beginning of the tick.
    pub agent: &'a Agent,
    /// Tick being decided.
    pub tick: Tick,
    /// Last published transaction prices.
    pub prices: &'a Prices,
    /// Initial reservation anchors, shared by all agents.
    pub anchor_prices: &'a Prices,
    /// Previous tick's submitted bid quantities.
    pub demand: &'a ResourceTotals,
    /// Previous tick's submitted ask quantities.
    pub supply: &'a ResourceTotals,
    /// Deterministic random tie breaker assigned in stable AgentId order.
    pub entropy: u64,
    /// Optional bounded proposals and public counterparties, enabled by run configuration.
    pub interactions: Option<&'a InteractionObservation>,
    /// Optional common-property and recurring-contract information, bounded by the engine.
    pub institutions: Option<&'a InstitutionObservation>,
}

/// Replaceable policy interface. Returned intents still require engine validation.
pub trait DecisionPolicy {
    /// Select one deliberate action; household consumption is a separate system.
    fn decide(&self, observation: &Observation<'_>) -> Decision;
}

/// Default offline policy with individual reserves, production, and trading rules.
#[derive(Debug, Default, Clone, Copy)]
pub struct RuleBasedPolicy;

impl DecisionPolicy for RuleBasedPolicy {
    fn decide(&self, obs: &Observation<'_>) -> Decision {
        let baseline = self.economic_decision(obs);
        institutions::choose(obs, &baseline)
            .or_else(|| interactions::choose(obs, &baseline))
            .unwrap_or(baseline)
    }
}

impl RuleBasedPolicy {
    fn economic_decision(&self, obs: &Observation<'_>) -> Decision {
        let agent = obs.agent;
        let food = agent.account.available(Resource::Food);
        let food_target = food_reserve(agent);
        if agent.needs.energy < 6 {
            return decision(Action::Rest {}, DecisionReason::RecoverEnergy);
        }
        if agent.needs.hunger >= 30 && food >= 2 {
            return decision(
                Action::Consume {
                    resource: Resource::Food,
                    quantity: 1,
                },
                DecisionReason::RelieveHunger,
            );
        }
        if food <= food_target / 2 {
            if agent.occupation == Occupation::Farmer {
                return decision(
                    Action::Produce {
                        resource: Resource::Food,
                    },
                    DecisionReason::ProduceOutput,
                );
            }
            if let Some(action) = buy(obs, Resource::Food, food_target - food, true, false) {
                return decision(action, DecisionReason::SecureFood);
            }
            return decision(
                Action::Produce {
                    resource: Resource::Food,
                },
                DecisionReason::Subsistence,
            );
        }

        let (comfort_threshold, goods_target) = match agent.goal {
            Goal::BuildWealth => (20, 1),
            Goal::SecureFood => (35, 1),
            Goal::StableLife => (65, 3),
            Goal::TradeProfit => (30, 1),
        };
        let goods = agent.account.available(Resource::Goods);
        if agent.occupation != Occupation::Producer
            && goods < goods_target
            && agent.needs.comfort < comfort_threshold
            && let Some(action) = buy(obs, Resource::Goods, goods_target - goods, false, true)
        {
            return decision(action, DecisionReason::ImproveComfort);
        }

        match agent.occupation {
            Occupation::Farmer => {
                if food > food_target {
                    decision(
                        sell(obs, Resource::Food, (food - food_target).min(20), false),
                        DecisionReason::SellSurplus,
                    )
                } else {
                    decision(
                        Action::Produce {
                            resource: Resource::Food,
                        },
                        DecisionReason::ProduceOutput,
                    )
                }
            }
            Occupation::Producer => {
                if goods > 1 {
                    return decision(
                        sell(obs, Resource::Goods, (goods - 1).min(8), false),
                        DecisionReason::SellSurplus,
                    );
                }
                for (resource, required, target) in
                    [(Resource::Wood, 4, 12), (Resource::Metal, 2, 6)]
                {
                    let stock = agent.account.available(resource);
                    if stock < required {
                        if let Some(action) = buy(obs, resource, target - stock, false, true) {
                            return decision(action, DecisionReason::AcquireInputs);
                        }
                        return decision(Action::Gather { resource }, DecisionReason::EarnIncome);
                    }
                }
                decision(
                    Action::Produce {
                        resource: Resource::Goods,
                    },
                    DecisionReason::ProduceOutput,
                )
            }
            Occupation::Merchant => {
                let best_stock = Resource::ALL
                    .into_iter()
                    .filter_map(|resource| {
                        let reserve = match resource {
                            Resource::Food => food_target,
                            Resource::Goods => 1,
                            _ => 0,
                        };
                        let surplus = agent.account.available(resource).saturating_sub(reserve);
                        (surplus > 0).then_some((resource, surplus))
                    })
                    .max_by_key(|&(resource, quantity)| sale_priority(obs, resource, quantity));
                if let Some((resource, quantity)) = best_stock {
                    return decision(
                        sell(obs, resource, quantity.min(10), true),
                        DecisionReason::SellSurplus,
                    );
                }
                // Visit each market with a seeded rotation; excess asks get first consideration.
                let offset = (obs.entropy % 4) as usize;
                let preferred = (0..4)
                    .map(|i| Resource::ALL[(i + offset) % 4])
                    .max_by_key(|r| obs.supply[r.index()].saturating_sub(obs.demand[r.index()]));
                if let Some(resource) = preferred
                    && let Some(action) = buy(obs, resource, 6, false, true)
                {
                    return decision(action, DecisionReason::TradeOpportunity);
                }
                decision(
                    Action::Gather {
                        resource: raw_material(obs),
                    },
                    DecisionReason::EarnIncome,
                )
            }
            Occupation::Consumer => {
                let surplus = [Resource::Wood, Resource::Metal]
                    .into_iter()
                    .filter(|&r| agent.account.available(r) > 0)
                    .max_by_key(|&r| sale_priority(obs, r, agent.account.available(r)));
                if let Some(resource) = surplus {
                    return decision(
                        sell(
                            obs,
                            resource,
                            agent.account.available(resource).min(16),
                            false,
                        ),
                        DecisionReason::SellSurplus,
                    );
                }
                decision(
                    Action::Gather {
                        resource: raw_material(obs),
                    },
                    DecisionReason::EarnIncome,
                )
            }
        }
    }
}

/// Goal-specific household food buffer, also affected by patience.
pub fn food_reserve(agent: &Agent) -> u64 {
    agent.food_reserve()
}

fn decision(action: Action, reason: DecisionReason) -> Decision {
    Decision { action, reason }
}

fn sale_priority(
    obs: &Observation<'_>,
    resource: Resource,
    quantity: u64,
) -> (bool, u128, u128, Resource) {
    let i = resource.index();
    // A persistent highest-value-only ask can deadlock complementary input markets:
    // households offer metal forever while factories first wait for wood.
    (
        obs.demand[i] > 0,
        obs.demand[i].saturating_sub(obs.supply[i]),
        u128::from(quantity) * u128::from(obs.prices[i]),
        resource,
    )
}

fn quote(obs: &Observation<'_>, resource: Resource, percent: u64) -> Money {
    // Anchors represent reservation preferences; published prices remain actual VWAPs.
    let reference = (obs.prices[resource.index()] * 3 + obs.anchor_prices[resource.index()]) / 4;
    (reference * percent / 100).clamp(1, MAX_PRICE)
}

fn buy(
    obs: &Observation<'_>,
    resource: Resource,
    desired: u64,
    urgent: bool,
    keep_food_cash: bool,
) -> Option<Action> {
    let agent = obs.agent;
    let percent = if urgent {
        115 + u64::from(agent.personality.risk_tolerance) / 10
    } else if agent.occupation == Occupation::Merchant && resource != Resource::Goods {
        94 + u64::from(agent.skills.trading) / 30
    } else {
        108 + u64::from(agent.personality.risk_tolerance) / 20
    };
    let price = quote(obs, resource, percent);
    let cash_buffer = if keep_food_cash { obs.prices[0] * 3 } else { 0 };
    let budget = agent.account.available_cash().saturating_sub(cash_buffer);
    let quantity = desired.min(budget / price).min(MAX_ORDER_QUANTITY);
    (quantity > 0).then_some(Action::Buy {
        resource,
        quantity,
        limit_price: price,
    })
}

fn sell(obs: &Observation<'_>, resource: Resource, quantity: u64, merchant: bool) -> Action {
    let agent = obs.agent;
    let percent = if merchant {
        103 + u64::from(agent.goal == Goal::TradeProfit) * 3
    } else {
        93 + u64::from(agent.personality.patience) / 15
    };
    let mut price = quote(obs, resource, percent);
    if merchant && !obs.tick.is_multiple_of(12) {
        // Periodic liquidation lets unsuccessful merchants recognize losses.
        price = price
            .max(agent.last_purchase_prices[resource.index()].saturating_mul(102) / 100)
            .min(MAX_PRICE);
    }
    Action::Sell {
        resource,
        quantity: quantity.min(MAX_ORDER_QUANTITY),
        limit_price: price,
    }
}

fn raw_material(obs: &Observation<'_>) -> Resource {
    let wood_shortage = obs.demand[1].saturating_sub(obs.supply[1]);
    let metal_shortage = obs.demand[2].saturating_sub(obs.supply[2]);
    if wood_shortage > metal_shortage.saturating_mul(2) {
        Resource::Wood
    } else if metal_shortage > wood_shortage {
        Resource::Metal
    } else if obs.entropy % 3 < 2 {
        Resource::Wood
    } else {
        Resource::Metal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentsociety_economy::validate_action;
    use agentsociety_world::World;

    #[test]
    fn policy_proposes_valid_initial_actions_for_all_roles() {
        for seed in 0..10 {
            let config = SimulationConfig {
                seed,
                ..Default::default()
            };
            let world = World::seeded(&config).unwrap();
            for agent in world.agents() {
                let obs = Observation {
                    agent,
                    tick: 1,
                    prices: &config.initial_prices,
                    anchor_prices: &config.initial_prices,
                    demand: &[0; 4],
                    supply: &[0; 4],
                    entropy: agent.id.0,
                    interactions: None,
                    institutions: None,
                };
                let selected = RuleBasedPolicy.decide(&obs);
                validate_action(&world, agent.id, &selected.action).unwrap();
            }
        }
    }

    #[test]
    fn different_goals_change_food_purchase_quantity() {
        let config = SimulationConfig::default();
        let world = World::seeded(&config).unwrap();
        let mut agent = world.agent(AgentId(3)).unwrap().clone();
        agent.account = Account::new(10_000, Inventory::new([1, 0, 0, 0]));
        let choose = |agent: &Agent| {
            RuleBasedPolicy.decide(&Observation {
                agent,
                tick: 1,
                prices: &config.initial_prices,
                anchor_prices: &config.initial_prices,
                demand: &[0; 4],
                supply: &[0; 4],
                entropy: 0,
                interactions: None,
                institutions: None,
            })
        };
        agent.goal = Goal::BuildWealth;
        let thrift = choose(&agent);
        agent.goal = Goal::SecureFood;
        let security = choose(&agent);
        match (thrift.action, security.action) {
            (Action::Buy { quantity: a, .. }, Action::Buy { quantity: b, .. }) => assert!(b > a),
            other => panic!("expected food purchases: {other:?}"),
        }
    }

    #[test]
    fn penniless_household_uses_subsistence_instead_of_invalid_credit() {
        let config = SimulationConfig::default();
        let world = World::seeded(&config).unwrap();
        let mut agent = world.agent(AgentId(3)).unwrap().clone();
        agent.account = Account::new(0, Inventory::default());
        let selected = RuleBasedPolicy.decide(&Observation {
            agent: &agent,
            tick: 1,
            prices: &config.initial_prices,
            anchor_prices: &config.initial_prices,
            demand: &[0; 4],
            supply: &[0; 4],
            entropy: 1,
            interactions: None,
            institutions: None,
        });
        assert_eq!(selected.reason, DecisionReason::Subsistence);
        assert_eq!(
            selected.action,
            Action::Produce {
                resource: Resource::Food
            }
        );
    }

    #[test]
    fn malformed_or_unsupported_action_json_is_rejected() {
        for json in [
            r#"{"action":"hack"}"#,
            r#"{"action":"buy","resource":"food","quantity":-1,"limit_price":10}"#,
            r#"{"action":"wait","unexpected":true}"#,
            r#"not json"#,
        ] {
            assert!(serde_json::from_str::<Action>(json).is_err());
        }
    }

    #[test]
    fn raw_material_seller_responds_to_buyers_instead_of_repeating_unsold_metal() {
        let config = SimulationConfig::default();
        let world = World::seeded(&config).unwrap();
        let mut agent = world.agent(AgentId(3)).unwrap().clone();
        agent.account = Account::new(10_000, Inventory::new([30, 6, 4, 3]));
        let selected = RuleBasedPolicy.decide(&Observation {
            agent: &agent,
            tick: 2,
            prices: &config.initial_prices,
            anchor_prices: &config.initial_prices,
            demand: &[0, 12, 0, 0],
            supply: &[0, 0, 16, 0],
            entropy: 0,
            interactions: None,
            institutions: None,
        });
        assert!(matches!(
            selected.action,
            Action::Sell {
                resource: Resource::Wood,
                ..
            }
        ));
    }
}
