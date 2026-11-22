//! Decisions use observed offers and actual outcome evidence without mutating the world.
use agentsociety_cognition::{DecisionPolicy, Observation, RuleBasedPolicy, review_goal};
use agentsociety_core::*;
use agentsociety_economy::ProposalBook;
use agentsociety_world::World;

fn agent() -> Agent {
    World::seeded(&SimulationConfig {
        agents: 4,
        ..Default::default()
    })
    .unwrap()
    .agent(AgentId(2))
    .unwrap()
    .clone()
}

fn evidence() -> GoalAssessment {
    GoalAssessment {
        elapsed_ticks: 20,
        hungry_ticks: 0,
        rejected_actions: 0,
        successful_trades: 0,
        cooperations: 0,
        wealth_declined: false,
    }
}

#[test]
fn food_security_overrides_profit_and_hysteresis_prevents_oscillation() {
    let mut agent = agent();
    agent.goal = Goal::TradeProfit;
    agent.account = Account::new(1000, Inventory::new([1, 0, 0, 0]));
    let mut outcomes = evidence();
    outcomes.successful_trades = 10;
    assert_eq!(
        review_goal(&agent, &outcomes),
        Some((Goal::SecureFood, GoalChangeReason::FoodInsecurity))
    );
    agent.goal = Goal::SecureFood;
    agent.account.produce(Resource::Food, 11).unwrap();
    assert_eq!(review_goal(&agent, &outcomes), None);
    agent.account.produce(Resource::Food, 2).unwrap();
    assert_eq!(
        review_goal(&agent, &outcomes),
        Some((Goal::TradeProfit, GoalChangeReason::TradingSuccess))
    );
    outcomes.wealth_declined = true;
    assert_eq!(
        review_goal(&agent, &outcomes),
        Some((Goal::BuildWealth, GoalChangeReason::WealthDecline))
    );
}

#[test]
fn goals_follow_completed_outcomes_and_do_not_treat_rejections_as_success() {
    let mut agent = agent();
    agent.goal = Goal::BuildWealth;
    agent.needs.comfort = 80;
    let mut outcomes = evidence();
    outcomes.rejected_actions = 20;
    assert_eq!(review_goal(&agent, &outcomes), None);
    outcomes.successful_trades = 2;
    assert_eq!(
        review_goal(&agent, &outcomes),
        Some((Goal::TradeProfit, GoalChangeReason::TradingSuccess))
    );
    agent.occupation = Occupation::Consumer;
    outcomes.cooperations = 1;
    assert_eq!(
        review_goal(&agent, &outcomes),
        Some((Goal::StableLife, GoalChangeReason::StableHousehold))
    );
    outcomes.hungry_ticks = 5;
    assert_eq!(
        review_goal(&agent, &outcomes),
        Some((Goal::SecureFood, GoalChangeReason::FoodInsecurity))
    );
    outcomes.elapsed_ticks = 0;
    assert_eq!(review_goal(&agent, &outcomes), None);
}

fn decide(
    world: &World,
    book: &ProposalBook,
    settings: &InteractionSettings,
    id: AgentId,
    tick: Tick,
) -> Action {
    let context = book.observe(world, settings, id, tick).unwrap();
    RuleBasedPolicy
        .decide(&Observation {
            agent: world.agent(id).unwrap(),
            tick,
            prices: &[100, 70, 120, 400],
            anchor_prices: &[100, 70, 120, 400],
            demand: &[0; 4],
            supply: &[0; 4],
            entropy: 0,
            interactions: context.as_ref(),
            institutions: None,
        })
        .action
}

#[test]
fn rules_counter_unreasonable_prices_then_accept_agreed_terms() {
    let mut world = World::seeded(&SimulationConfig {
        agents: 4,
        ..Default::default()
    })
    .unwrap();
    let settings = InteractionSettings {
        negotiation: true,
        ..Default::default()
    };
    let mut book = ProposalBook::default();
    book.apply(
        &mut world,
        &settings,
        AgentId(3),
        &Action::Negotiate {
            counterparty: AgentId(1),
            side: OrderSide::Sell,
            resource: Resource::Wood,
            quantity: 2,
            unit_price: 1000,
        },
        1,
    )
    .unwrap();
    let reply = decide(&world, &book, &settings, AgentId(1), 2);
    assert!(matches!(
        reply,
        Action::CounterOffer {
            proposal_id: 0,
            quantity: 2,
            unit_price: 1..1000
        }
    ));
    book.apply(&mut world, &settings, AgentId(1), &reply, 2)
        .unwrap();
    assert_eq!(
        decide(&world, &book, &settings, AgentId(3), 3),
        Action::AcceptProposal { proposal_id: 0 }
    );
}

#[test]
fn survival_defers_joint_work_and_cancels_a_food_sale() {
    let mut world = World::seeded(&SimulationConfig {
        agents: 4,
        ..Default::default()
    })
    .unwrap();
    let settings = InteractionSettings {
        negotiation: true,
        cooperation: true,
        ..Default::default()
    };
    let mut book = ProposalBook::default();
    book.apply(
        &mut world,
        &settings,
        AgentId(1),
        &Action::Cooperate {
            partner: AgentId(3),
            producer_share: 2,
        },
        1,
    )
    .unwrap();
    world
        .with_agent_mut(AgentId(3), |agent| {
            let food = agent.account.available(Resource::Food);
            agent.account.consume(Resource::Food, food - 1)
        })
        .unwrap();
    assert!(!decide(&world, &book, &settings, AgentId(3), 2).is_interaction());
    let stock = world
        .agent(AgentId(0))
        .unwrap()
        .account
        .available(Resource::Food);
    book.apply(
        &mut world,
        &settings,
        AgentId(0),
        &Action::Negotiate {
            counterparty: AgentId(2),
            side: OrderSide::Sell,
            resource: Resource::Food,
            quantity: stock,
            unit_price: 100,
        },
        1,
    )
    .unwrap();
    assert_eq!(
        decide(&world, &book, &settings, AgentId(0), 2),
        Action::CancelProposal { proposal_id: 1 }
    );
}
