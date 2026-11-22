//! Financial and ownership boundaries: rejected operations never consume partial assets.
use agentsociety_core::*;
use agentsociety_economy::{InstitutionBook, ProposalBook};
use agentsociety_world::World;

fn fixture() -> (World, InstitutionBook, OrganizationSettings) {
    let mut world = World::seeded(&SimulationConfig {
        agents: 6,
        ..Default::default()
    })
    .unwrap();
    for id in 0..6 {
        world
            .with_agent_mut(AgentId(id), |a| {
                a.account = Account::new(1000, Inventory::new([20, 12, 8, 0]));
                a.needs.energy = 50;
                Ok(())
            })
            .unwrap();
    }
    (
        world,
        InstitutionBook::default(),
        OrganizationSettings {
            enabled: true,
            membership_stake: 100,
            ..Default::default()
        },
    )
}
fn apply(
    world: &mut World,
    book: &mut InstitutionBook,
    settings: &OrganizationSettings,
    actor: u64,
    operation: InstitutionAction,
    tick: Tick,
) -> Vec<Event> {
    book.apply(world, settings, AgentId(actor), &operation, tick)
        .unwrap()
}
fn rejected(
    world: &mut World,
    book: &mut InstitutionBook,
    settings: &OrganizationSettings,
    actor: u64,
    operation: InstitutionAction,
    tick: Tick,
) {
    let before = (world.snapshot(), book.snapshot());
    assert!(
        book.apply(world, settings, AgentId(actor), &operation, tick)
            .is_err()
    );
    assert_eq!((world.snapshot(), book.snapshot()), before);
}
fn group(world: &mut World, book: &mut InstitutionBook, settings: &OrganizationSettings) {
    apply(world, book, settings, 1, InstitutionAction::Found {}, 1);
    for actor in [2, 3] {
        apply(
            world,
            book,
            settings,
            actor,
            InstitutionAction::Join {
                organization: OrganizationId(0),
            },
            1,
        );
    }
}
fn contract(
    organization: Option<OrganizationId>,
    other: Party,
    side: OrderSide,
) -> InstitutionAction {
    InstitutionAction::OfferContract {
        organization,
        counterparty: other,
        side,
        resource: Resource::Wood,
        quantity: 2,
        unit_price: 10,
        deliveries: 3,
        interval_ticks: 2,
    }
}

#[test]
fn equal_property_production_dividends_and_departure_conserve_assets() {
    let (mut world, mut book, settings) = fixture();
    group(&mut world, &mut book, &settings);
    assert_eq!(
        book.organization(OrganizationId(0))
            .unwrap()
            .account
            .wallet(),
        300
    );
    apply(
        &mut world,
        &mut book,
        &settings,
        2,
        InstitutionAction::Contribute {
            organization: OrganizationId(0),
            cash: 0,
            inventory: Inventory::new([0, 4, 2, 0]),
        },
        2,
    );
    let production = book.produce(&mut world, &settings, 4).unwrap();
    assert!(
        matches!(&production[0], Event::Organization { event: OrganizationEvent::Produced { workers, goods: 6, .. } } if workers.len() == 3)
    );
    assert_eq!(
        book.organization(OrganizationId(0))
            .unwrap()
            .account
            .inventory()
            .amounts(),
        [0, 0, 0, 6]
    );
    assert!(book.produce(&mut world, &settings, 5).unwrap().is_empty());
    assert!(book.produce(&mut world, &settings, 4).unwrap().is_empty());
    rejected(
        &mut world,
        &mut book,
        &settings,
        2,
        InstitutionAction::Distribute {
            organization: OrganizationId(0),
            cash_per_member: 10,
            goods_per_member: 1,
        },
        5,
    );
    rejected(
        &mut world,
        &mut book,
        &settings,
        1,
        InstitutionAction::Distribute {
            organization: OrganizationId(0),
            cash_per_member: 200,
            goods_per_member: 1,
        },
        5,
    );
    apply(
        &mut world,
        &mut book,
        &settings,
        1,
        InstitutionAction::Distribute {
            organization: OrganizationId(0),
            cash_per_member: 10,
            goods_per_member: 1,
        },
        5,
    );
    for actor in [1, 2, 3] {
        apply(
            &mut world,
            &mut book,
            &settings,
            actor,
            InstitutionAction::Leave {
                organization: OrganizationId(0),
            },
            6,
        );
        assert_eq!(world.agent(AgentId(actor)).unwrap().account.wallet(), 1000);
        assert_eq!(
            world
                .agent(AgentId(actor))
                .unwrap()
                .account
                .inventory()
                .get(Resource::Goods),
            2
        );
    }
    assert_eq!(book.organizations().len(), 0);
    assert_eq!(book.retired().wallet(), 0);
    assert_eq!(book.retired().transfers_in(), 300);
    assert_eq!(book.retired().transfers_out(), 300);
    book.retired().validate_ledger(0).unwrap();
    assert_eq!(
        world.agents().map(|a| a.account.wallet()).sum::<u64>(),
        6000
    );
}

#[test]
fn accepted_installments_keep_funding_and_release_bond_only_at_completion() {
    let (mut world, mut book, settings) = fixture();
    apply(
        &mut world,
        &mut book,
        &settings,
        0,
        contract(None, Party::Agent(AgentId(4)), OrderSide::Buy),
        1,
    );
    rejected(
        &mut world,
        &mut book,
        &settings,
        4,
        InstitutionAction::AcceptContract { contract_id: 0 },
        1,
    );
    apply(
        &mut world,
        &mut book,
        &settings,
        4,
        InstitutionAction::AcceptContract { contract_id: 0 },
        2,
    );
    assert_eq!(world.agent(AgentId(0)).unwrap().account.reserved_cash(), 60);
    assert_eq!(world.agent(AgentId(4)).unwrap().account.reserved_cash(), 6);
    for tick in [4, 6, 8] {
        let events = book.settle_due(&mut world, tick).unwrap();
        assert!(matches!(
            events[0],
            Event::Contract {
                event: ContractEvent::Delivered {
                    quantity: 2,
                    price: 10,
                    ..
                }
            }
        ));
        ProposalBook::default()
            .audit_with_cash(&world, &book.personal_collateral().unwrap())
            .unwrap();
    }
    assert_eq!(book.contracts().len(), 0);
    assert_eq!(world.agent(AgentId(0)).unwrap().account.wallet(), 940);
    assert_eq!(world.agent(AgentId(4)).unwrap().account.wallet(), 1060);
    assert_eq!(world.agent(AgentId(4)).unwrap().account.reserved_cash(), 0);
    assert_eq!(
        world.agent(AgentId(0)).unwrap().last_purchase_prices[Resource::Wood.index()],
        10
    );
    assert_eq!(world.agent(AgentId(0)).unwrap().account.spent(), 60);
}

#[test]
fn default_pays_reserved_bond_and_never_records_an_undelivered_trade() {
    let (mut world, mut book, settings) = fixture();
    apply(
        &mut world,
        &mut book,
        &settings,
        0,
        contract(None, Party::Agent(AgentId(4)), OrderSide::Buy),
        1,
    );
    apply(
        &mut world,
        &mut book,
        &settings,
        4,
        InstitutionAction::AcceptContract { contract_id: 0 },
        2,
    );
    world
        .with_agent_mut(AgentId(4), |a| a.account.consume(Resource::Wood, 12))
        .unwrap();
    let events = book.settle_due(&mut world, 4).unwrap();
    assert!(matches!(
        events.as_slice(),
        [Event::Contract {
            event: ContractEvent::Closed {
                outcome: ContractOutcome::Defaulted,
                compensation: 6,
                ..
            }
        }]
    ));
    assert_eq!(world.agent(AgentId(0)).unwrap().account.wallet(), 1006);
    assert_eq!(world.agent(AgentId(4)).unwrap().account.wallet(), 994);
    for actor in [0, 4] {
        let a = &world.agent(AgentId(actor)).unwrap().account;
        a.validate_ledger(1000).unwrap();
        assert_eq!((a.earned(), a.spent(), a.reserved_cash()), (0, 0, 0));
    }
}

#[test]
fn group_agreements_lock_membership_and_cannot_trade_with_their_owners() {
    let (mut world, mut book, settings) = fixture();
    group(&mut world, &mut book, &settings);
    rejected(
        &mut world,
        &mut book,
        &settings,
        1,
        contract(
            Some(OrganizationId(0)),
            Party::Agent(AgentId(2)),
            OrderSide::Buy,
        ),
        2,
    );
    rejected(
        &mut world,
        &mut book,
        &settings,
        2,
        contract(
            Some(OrganizationId(0)),
            Party::Agent(AgentId(4)),
            OrderSide::Buy,
        ),
        2,
    );
    apply(
        &mut world,
        &mut book,
        &settings,
        1,
        contract(
            Some(OrganizationId(0)),
            Party::Agent(AgentId(4)),
            OrderSide::Buy,
        ),
        2,
    );
    rejected(
        &mut world,
        &mut book,
        &settings,
        2,
        InstitutionAction::Leave {
            organization: OrganizationId(0),
        },
        3,
    );
    rejected(
        &mut world,
        &mut book,
        &settings,
        5,
        InstitutionAction::Join {
            organization: OrganizationId(0),
        },
        3,
    );
    apply(
        &mut world,
        &mut book,
        &settings,
        4,
        InstitutionAction::AcceptContract { contract_id: 0 },
        3,
    );
    rejected(
        &mut world,
        &mut book,
        &settings,
        1,
        InstitutionAction::CancelContract { contract_id: 0 },
        4,
    );
    book.settle_due(&mut world, 5).unwrap();
    book.audit_groups().unwrap();
    assert_eq!(
        book.organization(OrganizationId(0))
            .unwrap()
            .account
            .reserved_cash(),
        40
    );
}

#[test]
fn pending_rejection_cancellation_and_expiration_refund_exactly_once() {
    for method in 0..3 {
        let (mut world, mut book, settings) = fixture();
        apply(
            &mut world,
            &mut book,
            &settings,
            0,
            contract(None, Party::Agent(AgentId(4)), OrderSide::Sell),
            1,
        );
        match method {
            0 => {
                apply(
                    &mut world,
                    &mut book,
                    &settings,
                    4,
                    InstitutionAction::RejectContract { contract_id: 0 },
                    2,
                );
            }
            1 => {
                apply(
                    &mut world,
                    &mut book,
                    &settings,
                    0,
                    InstitutionAction::CancelContract { contract_id: 0 },
                    2,
                );
            }
            _ => {
                assert!(book.expire(&mut world, 5).unwrap().is_empty());
                assert_eq!(book.expire(&mut world, 6).unwrap().len(), 1);
                assert!(book.expire(&mut world, 7).unwrap().is_empty());
            }
        }
        assert_eq!(world.agent(AgentId(0)).unwrap().account.wallet(), 1000);
        assert_eq!(world.agent(AgentId(0)).unwrap().account.reserved_cash(), 0);
        assert_eq!(book.contracts().len(), 0);
    }
}

#[test]
fn proposal_and_recurring_contract_cash_are_audited_together() {
    let (mut world, mut book, settings) = fixture();
    let mut proposals = ProposalBook::default();
    let interactions = InteractionSettings {
        negotiation: true,
        ..Default::default()
    };
    proposals
        .apply(
            &mut world,
            &interactions,
            AgentId(0),
            &Action::Negotiate {
                counterparty: AgentId(5),
                side: OrderSide::Buy,
                resource: Resource::Metal,
                quantity: 2,
                unit_price: 15,
            },
            1,
        )
        .unwrap();
    apply(
        &mut world,
        &mut book,
        &settings,
        0,
        contract(None, Party::Agent(AgentId(4)), OrderSide::Buy),
        1,
    );
    world.advance_tick().unwrap();
    let restored = InstitutionBook::from_snapshot(&world, &settings, book.snapshot()).unwrap();
    ProposalBook::from_snapshot_with_cash(
        &world,
        &interactions,
        proposals.snapshot(),
        &restored.personal_collateral().unwrap(),
    )
    .unwrap();
    assert_eq!(world.agent(AgentId(0)).unwrap().account.reserved_cash(), 90);
    assert!(proposals.audit_collateral(&world).is_err());
}
