//! Bilateral commitments must conserve assets and fail without partial mutation.
use agentsociety_core::*;
use agentsociety_economy::{OrderBook, ProposalBook, apply_action};
use agentsociety_world::World;

fn fixture() -> (World, ProposalBook, InteractionSettings) {
    let mut world = World::seeded(&SimulationConfig {
        agents: 6,
        ..Default::default()
    })
    .unwrap();
    for id in 0..6 {
        world
            .with_agent_mut(AgentId(id), |agent| {
                agent.account = Account::new(1_000, Inventory::new([20, 10, 10, 0]));
                agent.needs.energy = 20;
                Ok(())
            })
            .unwrap();
    }
    (
        world,
        ProposalBook::default(),
        InteractionSettings {
            negotiation: true,
            cooperation: true,
            ..Default::default()
        },
    )
}

fn offer(other: u64, side: OrderSide, quantity: u64, unit_price: u64) -> Action {
    Action::Negotiate {
        counterparty: AgentId(other),
        side,
        resource: Resource::Wood,
        quantity,
        unit_price,
    }
}

fn reject_unchanged(
    world: &mut World,
    book: &mut ProposalBook,
    settings: &InteractionSettings,
    id: u64,
    action: Action,
    tick: Tick,
) {
    let before_world = world.snapshot();
    let before_book = book.snapshot();
    assert!(
        book.apply(world, settings, AgentId(id), &action, tick)
            .is_err()
    );
    assert_eq!(world.snapshot(), before_world);
    assert_eq!(book.snapshot(), before_book);
}

#[test]
fn trade_collateral_survives_market_expiry_and_cannot_be_spent_twice() {
    let (mut world, mut book, settings) = fixture();
    book.apply(
        &mut world,
        &settings,
        AgentId(0),
        &offer(1, OrderSide::Buy, 4, 200),
        1,
    )
    .unwrap();
    let mut market = OrderBook::new([100, 70, 120, 400]).unwrap();
    assert!(
        apply_action(
            &mut world,
            &mut market,
            AgentId(0),
            &Action::Buy {
                resource: Resource::Metal,
                quantity: 3,
                limit_price: 100
            },
            1
        )
        .is_err()
    );
    apply_action(
        &mut world,
        &mut market,
        AgentId(0),
        &Action::Buy {
            resource: Resource::Metal,
            quantity: 2,
            limit_price: 100,
        },
        1,
    )
    .unwrap();
    assert_eq!(
        world.agent(AgentId(0)).unwrap().account.reserved_cash(),
        1_000
    );
    market.expire_all(&mut world).unwrap();
    assert_eq!(
        world.agent(AgentId(0)).unwrap().account.reserved_cash(),
        800
    );
    book.audit_collateral(&world).unwrap();
    let events = book
        .apply(
            &mut world,
            &settings,
            AgentId(1),
            &Action::AcceptProposal { proposal_id: 0 },
            2,
        )
        .unwrap();
    assert!(matches!(
        events[0],
        Event::NegotiatedTrade {
            buyer: AgentId(0),
            seller: AgentId(1),
            quantity: 4,
            price: 200,
            ..
        }
    ));
    let buyer = world.agent(AgentId(0)).unwrap();
    let seller = world.agent(AgentId(1)).unwrap();
    assert_eq!(
        (buyer.account.wallet(), seller.account.wallet()),
        (200, 1_800)
    );
    assert_eq!(
        (
            buyer.account.inventory().get(Resource::Wood),
            seller.account.inventory().get(Resource::Wood)
        ),
        (14, 6)
    );
    assert_eq!((buyer.account.spent(), seller.account.earned()), (800, 800));
    assert_eq!(buyer.last_purchase_prices[Resource::Wood.index()], 200);
    book.audit_collateral(&world).unwrap();
}

#[test]
fn counteroffers_transfer_collateral_keep_deadline_and_are_bounded() {
    let (mut world, mut book, settings) = fixture();
    book.apply(
        &mut world,
        &settings,
        AgentId(0),
        &offer(1, OrderSide::Buy, 2, 100),
        1,
    )
    .unwrap();
    reject_unchanged(
        &mut world,
        &mut book,
        &settings,
        1,
        Action::AcceptProposal { proposal_id: 0 },
        1,
    );
    reject_unchanged(
        &mut world,
        &mut book,
        &settings,
        2,
        Action::AcceptProposal { proposal_id: 0 },
        2,
    );
    reject_unchanged(
        &mut world,
        &mut book,
        &settings,
        0,
        Action::RejectProposal { proposal_id: 0 },
        2,
    );
    // Failed replacement must retain the original buyer's reservation.
    reject_unchanged(
        &mut world,
        &mut book,
        &settings,
        1,
        Action::CounterOffer {
            proposal_id: 0,
            quantity: 11,
            unit_price: 120,
        },
        2,
    );
    for (tick, id) in [(2, 1), (3, 0), (4, 1)] {
        book.apply(
            &mut world,
            &settings,
            AgentId(id),
            &Action::CounterOffer {
                proposal_id: 0,
                quantity: 3,
                unit_price: 110,
            },
            tick,
        )
        .unwrap();
        let proposal = book.proposals().next().unwrap();
        assert_eq!(
            (
                proposal.proposer,
                proposal.expires_at,
                proposal.counteroffers
            ),
            (AgentId(id), 5, (tick - 1) as u8)
        );
        book.audit_collateral(&world).unwrap();
    }
    assert_eq!(world.agent(AgentId(0)).unwrap().account.reserved_cash(), 0);
    assert_eq!(
        world
            .agent(AgentId(1))
            .unwrap()
            .account
            .reserved_inventory()
            .get(Resource::Wood),
        3
    );
    reject_unchanged(
        &mut world,
        &mut book,
        &settings,
        0,
        Action::CounterOffer {
            proposal_id: 0,
            quantity: 3,
            unit_price: 100,
        },
        5,
    );
    book.apply(
        &mut world,
        &settings,
        AgentId(0),
        &Action::AcceptProposal { proposal_id: 0 },
        5,
    )
    .unwrap();
    assert_eq!(world.agent(AgentId(0)).unwrap().account.wallet(), 670);
    assert_eq!(book.next_id(), 1);
    book.audit_collateral(&world).unwrap();
}

#[test]
fn rejection_cancellation_and_expiry_release_only_the_closed_offer() {
    for outcome in [
        ProposalOutcome::Rejected,
        ProposalOutcome::Cancelled,
        ProposalOutcome::Expired,
    ] {
        let (mut world, mut book, settings) = fixture();
        book.apply(
            &mut world,
            &settings,
            AgentId(0),
            &offer(1, OrderSide::Buy, 2, 100),
            1,
        )
        .unwrap();
        book.apply(
            &mut world,
            &settings,
            AgentId(0),
            &offer(2, OrderSide::Buy, 1, 150),
            2,
        )
        .unwrap();
        let before = world.agent(AgentId(0)).unwrap().account.wallet();
        let events = match outcome {
            ProposalOutcome::Rejected => book
                .apply(
                    &mut world,
                    &settings,
                    AgentId(1),
                    &Action::RejectProposal { proposal_id: 0 },
                    3,
                )
                .unwrap(),
            ProposalOutcome::Cancelled => book
                .apply(
                    &mut world,
                    &settings,
                    AgentId(0),
                    &Action::CancelProposal { proposal_id: 0 },
                    3,
                )
                .unwrap(),
            ProposalOutcome::Expired => {
                assert!(book.expire(&mut world, 5).unwrap().is_empty());
                reject_unchanged(
                    &mut world,
                    &mut book,
                    &settings,
                    1,
                    Action::AcceptProposal { proposal_id: 0 },
                    6,
                );
                book.expire(&mut world, 6).unwrap()
            }
            _ => unreachable!(),
        };
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0], Event::ProposalClosed { outcome: actual, .. } if actual == outcome)
        );
        assert_eq!(world.agent(AgentId(0)).unwrap().account.wallet(), before);
        assert_eq!(
            world.agent(AgentId(0)).unwrap().account.reserved_cash(),
            150
        );
        assert_eq!(book.count_for(AgentId(0)), 1);
        book.audit_collateral(&world).unwrap();
    }
}

#[test]
fn settlement_overflow_or_insufficient_funds_leaves_offer_and_accounts_intact() {
    let (mut world, mut book, settings) = fixture();
    book.apply(
        &mut world,
        &settings,
        AgentId(1),
        &offer(0, OrderSide::Sell, 2, 600),
        1,
    )
    .unwrap();
    reject_unchanged(
        &mut world,
        &mut book,
        &settings,
        0,
        Action::AcceptProposal { proposal_id: 0 },
        2,
    );
    book.apply(
        &mut world,
        &settings,
        AgentId(0),
        &Action::CounterOffer {
            proposal_id: 0,
            quantity: 2,
            unit_price: 100,
        },
        2,
    )
    .unwrap();
    world
        .with_agent_mut(AgentId(1), |agent| {
            agent.account = Account::new(u64::MAX, Inventory::new([20, 10, 10, 0]));
            Ok(())
        })
        .unwrap();
    reject_unchanged(
        &mut world,
        &mut book,
        &settings,
        1,
        Action::AcceptProposal { proposal_id: 0 },
        3,
    );
    book.audit_collateral(&world).unwrap();
}

#[test]
fn cooperative_production_consumes_both_inputs_and_pays_only_agreed_goods() {
    let (mut world, mut book, settings) = fixture();
    book.apply(
        &mut world,
        &settings,
        AgentId(1),
        &Action::Cooperate {
            partner: AgentId(0),
            producer_share: 3,
        },
        1,
    )
    .unwrap();
    assert_eq!(world.agent(AgentId(1)).unwrap().needs.energy, 15);
    assert_eq!(
        world
            .agent(AgentId(1))
            .unwrap()
            .account
            .available(Resource::Metal),
        8
    );
    let events = book
        .apply(
            &mut world,
            &settings,
            AgentId(0),
            &Action::AcceptProposal { proposal_id: 0 },
            2,
        )
        .unwrap();
    let producer = world.agent(AgentId(1)).unwrap();
    let partner = world.agent(AgentId(0)).unwrap();
    assert_eq!((producer.needs.energy, partner.needs.energy), (15, 15));
    assert_eq!(producer.account.inventory().get(Resource::Metal), 8);
    assert_eq!(partner.account.inventory().get(Resource::Wood), 6);
    assert_eq!(
        (
            producer.account.inventory().get(Resource::Goods),
            partner.account.inventory().get(Resource::Goods)
        ),
        (3, 1)
    );
    assert_eq!(
        (producer.account.wallet(), partner.account.wallet()),
        (1_000, 1_000)
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(
                e,
                Event::ResourceConsumed {
                    purpose: ConsumptionPurpose::ProductionInput,
                    ..
                }
            ))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::ResourceProduced { .. }))
            .count(),
        2
    );
    assert!(events.iter().any(|e| matches!(
        e,
        Event::CooperationCompleted {
            producer_share: 3,
            partner_share: 1,
            ..
        }
    )));
    assert!(!events.iter().any(|e| matches!(
        e,
        Event::TradeExecuted { .. } | Event::NegotiatedTrade { .. }
    )));
    book.audit_collateral(&world).unwrap();
}

#[test]
fn failed_joint_output_is_atomic_and_expiry_does_not_refund_prepared_labor() {
    let (mut world, mut book, settings) = fixture();
    book.apply(
        &mut world,
        &settings,
        AgentId(1),
        &Action::Cooperate {
            partner: AgentId(0),
            producer_share: 2,
        },
        1,
    )
    .unwrap();
    for stock in [[20, 3, 0, 0], [20, 4, 0, u64::MAX]] {
        world
            .with_agent_mut(AgentId(0), |agent| {
                agent.account = Account::new(1_000, Inventory::new(stock));
                Ok(())
            })
            .unwrap();
        reject_unchanged(
            &mut world,
            &mut book,
            &settings,
            0,
            Action::AcceptProposal { proposal_id: 0 },
            2,
        );
    }
    let events = book.expire(&mut world, 6).unwrap();
    assert_eq!(events.len(), 1);
    let producer = world.agent(AgentId(1)).unwrap();
    assert_eq!(producer.needs.energy, 15);
    assert_eq!(producer.account.available(Resource::Metal), 10);
    book.audit_collateral(&world).unwrap();
}

#[test]
fn pair_capacity_roles_disabled_features_and_self_targets_are_validated() {
    let (mut world, mut book, settings) = fixture();
    for action in [
        offer(0, OrderSide::Buy, 1, 1),
        offer(99, OrderSide::Buy, 1, 1),
        offer(1, OrderSide::Buy, 0, 1),
        Action::Cooperate {
            partner: AgentId(1),
            producer_share: 2,
        },
    ] {
        reject_unchanged(&mut world, &mut book, &settings, 0, action, 1);
    }
    reject_unchanged(
        &mut world,
        &mut book,
        &InteractionSettings::default(),
        0,
        offer(1, OrderSide::Buy, 1, 1),
        1,
    );
    reject_unchanged(
        &mut world,
        &mut book,
        &settings,
        1,
        Action::Cooperate {
            partner: AgentId(0),
            producer_share: 4,
        },
        1,
    );
    for other in 1..=4 {
        book.apply(
            &mut world,
            &settings,
            AgentId(0),
            &offer(other, OrderSide::Buy, 1, 1),
            1,
        )
        .unwrap();
    }
    reject_unchanged(
        &mut world,
        &mut book,
        &settings,
        0,
        offer(5, OrderSide::Buy, 1, 1),
        2,
    );
    reject_unchanged(
        &mut world,
        &mut book,
        &settings,
        1,
        offer(0, OrderSide::Sell, 1, 1),
        2,
    );
    book.audit_collateral(&world).unwrap();
}

#[test]
fn restore_requires_exact_collateral_valid_times_and_bounded_observations() {
    let (mut world, mut book, settings) = fixture();
    book.apply(
        &mut world,
        &settings,
        AgentId(0),
        &offer(1, OrderSide::Buy, 2, 100),
        1,
    )
    .unwrap();
    assert!(
        book.observe(&world, &settings, AgentId(1), 1)
            .unwrap()
            .unwrap()
            .proposals
            .is_empty()
    );
    world.advance_tick().unwrap();
    let saved = book.snapshot();
    let restored = ProposalBook::from_snapshot(&world, &settings, saved.clone()).unwrap();
    assert_eq!(restored.snapshot(), saved);
    for malformed in [0, 1, 2, 3] {
        let mut altered = saved.clone();
        match malformed {
            0 => altered.proposals[0].expires_at += 1,
            1 => altered.proposals[0].updated_at += 1,
            2 => altered.proposals.clear(),
            _ => {
                altered.proposals[0].terms = ProposalTerms::Trade {
                    side: OrderSide::Buy,
                    resource: Resource::Wood,
                    quantity: 3,
                    unit_price: 100,
                }
            }
        }
        assert!(ProposalBook::from_snapshot(&world, &settings, altered).is_err());
    }
    let observation = book
        .observe(&world, &settings, AgentId(1), 2)
        .unwrap()
        .unwrap();
    assert_eq!(observation.proposals.len(), 1);
    assert!(observation.counterparties.len() <= MAX_OBSERVED_PEERS);
    assert!(
        !observation
            .counterparties
            .iter()
            .any(|p| p.id == AgentId(1))
    );
    assert!(
        book.observe(&world, &settings, AgentId(1), 6)
            .unwrap()
            .unwrap()
            .proposals
            .is_empty()
    );
    assert!(
        book.observe(&world, &InteractionSettings::default(), AgentId(1), 2)
            .unwrap()
            .is_none()
    );
}

#[test]
fn combined_price_weights_both_markets_and_ignores_unaccepted_quotes() {
    let (mut world, mut book, settings) = fixture();
    let mut market = OrderBook::new([100, 70, 120, 400]).unwrap();
    let opening = book
        .apply(
            &mut world,
            &settings,
            AgentId(0),
            &offer(1, OrderSide::Buy, 2, 200),
            1,
        )
        .unwrap();
    market.include_negotiated_trades(&opening).unwrap();
    assert_eq!(market.prices()[Resource::Wood.index()], 70);
    let mut events = book
        .apply(
            &mut world,
            &settings,
            AgentId(1),
            &Action::AcceptProposal { proposal_id: 0 },
            2,
        )
        .unwrap();
    apply_action(
        &mut world,
        &mut market,
        AgentId(2),
        &Action::Buy {
            resource: Resource::Wood,
            quantity: 4,
            limit_price: 80,
        },
        2,
    )
    .unwrap();
    apply_action(
        &mut world,
        &mut market,
        AgentId(3),
        &Action::Sell {
            resource: Resource::Wood,
            quantity: 4,
            limit_price: 80,
        },
        2,
    )
    .unwrap();
    events.extend(market.match_orders(&mut world).unwrap());
    market.include_negotiated_trades(&events).unwrap();
    assert_eq!(*market.prices(), [100, 120, 120, 400]);
}
