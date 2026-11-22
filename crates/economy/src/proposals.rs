//! Persistent bilateral offers. Every rejected action leaves both accounts and the book intact.
use crate::validate_order_values;
use agentsociety_core::*;
use agentsociety_world::World;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Portable live proposals; participant indexes are reconstructed and validated on restore.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalSnapshot {
    /// Next globally unique proposal ID, never reused after closure.
    pub next_id: u64,
    /// Live proposals sorted by ID.
    pub proposals: Vec<Proposal>,
}

/// Bounded proposal state with per-participant indexes and attributable collateral.
#[derive(Default)]
pub struct ProposalBook {
    next_id: u64,
    proposals: BTreeMap<u64, Proposal>,
    participants: BTreeMap<AgentId, BTreeSet<u64>>,
}

enum Change {
    Put(Proposal),
    Remove(u64),
}

struct Prepared {
    actor: AgentId,
    other: AgentId,
    actor_account: Account,
    other_account: Account,
    actor_energy: u8,
    other_energy: u8,
    purchased: Option<(AgentId, Resource, Money)>,
    next_id: u64,
    change: Change,
    events: Vec<Event>,
}

fn invalid(message: &str) -> DomainError {
    DomainError::InvalidAction(message.into())
}

impl ProposalBook {
    /// Next proposal identifier, including already closed historical proposals.
    pub const fn next_id(&self) -> u64 {
        self.next_id
    }
    /// All live proposals in stable ID order.
    pub fn proposals(&self) -> impl Iterator<Item = &Proposal> {
        self.proposals.values()
    }

    /// At most four incoming and outgoing proposals for one participant.
    pub fn for_agent(&self, id: AgentId) -> impl Iterator<Item = &Proposal> {
        self.participants
            .get(&id)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .map(|id| &self.proposals[id])
    }

    /// Number of live proposals involving this agent.
    pub fn count_for(&self, id: AgentId) -> usize {
        self.participants.get(&id).map_or(0, BTreeSet::len)
    }

    /// Serialize only authoritative state, not indexes or completed history.
    pub fn snapshot(&self) -> ProposalSnapshot {
        ProposalSnapshot {
            next_id: self.next_id,
            proposals: self.proposals.values().cloned().collect(),
        }
    }

    /// Validate live state, all deadlines, capacity limits, and exact account collateral.
    pub fn from_snapshot(
        world: &World,
        settings: &InteractionSettings,
        snapshot: ProposalSnapshot,
    ) -> Result<Self> {
        Self::from_snapshot_with_cash(world, settings, snapshot, &BTreeMap::new())
    }

    /// Restore proposals while attributing separately funded recurring contracts.
    pub fn from_snapshot_with_cash(
        world: &World,
        settings: &InteractionSettings,
        snapshot: ProposalSnapshot,
        other_cash: &BTreeMap<AgentId, Money>,
    ) -> Result<Self> {
        let malformed = || DomainError::Invariant("invalid proposal snapshot".into());
        if snapshot.proposals.len() > world.len().saturating_mul(MAX_OPEN_PROPOSALS) / 2
            || !snapshot
                .proposals
                .windows(2)
                .all(|pair| pair[0].id < pair[1].id)
            || u128::from(snapshot.next_id) > u128::from(world.tick()) * world.len() as u128
            || (!settings.negotiation && !settings.cooperation && snapshot.next_id != 0)
        {
            return Err(malformed());
        }
        let mut book = Self {
            next_id: snapshot.next_id,
            ..Default::default()
        };
        for proposal in snapshot.proposals {
            if proposal.id >= book.next_id
                || proposal.proposer == proposal.recipient
                || proposal.created_at == 0
                || proposal.created_at > proposal.updated_at
                || proposal.updated_at > world.tick()
                || proposal.expires_at < world.tick()
                || proposal
                    .created_at
                    .checked_add(settings.offer_lifetime_ticks)
                    != Some(proposal.expires_at)
                || proposal.counteroffers > MAX_COUNTEROFFERS
                || u64::from(proposal.counteroffers) > proposal.updated_at - proposal.created_at
                || (proposal.counteroffers == 0 && proposal.updated_at != proposal.created_at)
            {
                return Err(malformed());
            }
            let proposer = world.agent(proposal.proposer).map_err(|_| malformed())?;
            world.agent(proposal.recipient).map_err(|_| malformed())?;
            match proposal.terms {
                ProposalTerms::Trade {
                    quantity,
                    unit_price,
                    ..
                } => {
                    if !settings.negotiation {
                        return Err(malformed());
                    }
                    validate_order_values(quantity, unit_price).map_err(|_| malformed())?;
                }
                ProposalTerms::Cooperation { producer_share } => {
                    if !settings.cooperation
                        || proposer.occupation != Occupation::Producer
                        || !(1..COOPERATIVE_GOODS).contains(&producer_share)
                        || proposal.counteroffers != 0
                    {
                        return Err(malformed());
                    }
                }
            }
            book.check_new_pair(proposal.proposer, proposal.recipient)
                .map_err(|_| malformed())?;
            book.put(proposal);
        }
        book.audit_with_cash(world, other_cash)?;
        Ok(book)
    }

    /// Verify every reservation belongs to exactly the live offers, after market expiry.
    pub fn audit_collateral(&self, world: &World) -> Result<()> {
        self.audit_with_cash(world, &BTreeMap::new())
    }

    /// Attribute every personal reservation to proposals and funded contracts exactly once.
    pub fn audit_with_cash(
        &self,
        world: &World,
        other_cash: &BTreeMap<AgentId, Money>,
    ) -> Result<()> {
        let mut expected: BTreeMap<AgentId, (Money, Inventory)> = BTreeMap::new();
        for (&owner, &cash) in other_cash {
            expected.insert(owner, (cash, Inventory::default()));
        }
        for proposal in self.proposals.values() {
            let (cash, stock) = expected.entry(proposal.proposer).or_default();
            match proposal.terms {
                ProposalTerms::Trade {
                    side: OrderSide::Buy,
                    quantity,
                    unit_price,
                    ..
                } => {
                    *cash = cash
                        .checked_add(notional(unit_price, quantity)?)
                        .ok_or(DomainError::Overflow("proposal collateral"))?;
                }
                ProposalTerms::Trade {
                    side: OrderSide::Sell,
                    resource,
                    quantity,
                    ..
                } => stock.add(resource, quantity)?,
                ProposalTerms::Cooperation { .. } => {
                    stock.add(Resource::Metal, COOPERATIVE_METAL)?
                }
            }
        }
        for agent in world.agents() {
            agent.account.validate()?;
            let (cash, stock) = expected.remove(&agent.id).unwrap_or_default();
            if agent.account.reserved_cash() != cash || agent.account.reserved_inventory() != &stock
            {
                return Err(DomainError::Invariant(format!(
                    "unattributed proposal collateral for agent {}",
                    agent.id
                )));
            }
        }
        if !expected.is_empty() {
            return Err(DomainError::Invariant("unknown collateral owner".into()));
        }
        Ok(())
    }

    /// Read-only validation used by external policies; never changes the live world.
    pub fn validate_action(
        &self,
        world: &World,
        settings: &InteractionSettings,
        id: AgentId,
        action: &Action,
        tick: Tick,
    ) -> Result<()> {
        self.prepare(world, settings, id, action, tick).map(|_| ())
    }

    /// Plan both account changes first, then commit the complete accepted action.
    pub fn apply(
        &mut self,
        world: &mut World,
        settings: &InteractionSettings,
        id: AgentId,
        action: &Action,
        tick: Tick,
    ) -> Result<Vec<Event>> {
        let plan = self.prepare(world, settings, id, action, tick)?;
        world.with_pair_mut(plan.actor, plan.other, |actor, other| {
            actor.account = plan.actor_account;
            other.account = plan.other_account;
            actor.needs.energy = plan.actor_energy;
            other.needs.energy = plan.other_energy;
            if let Some((buyer, resource, price)) = plan.purchased {
                let agent = if buyer == actor.id { actor } else { other };
                agent.last_purchase_prices[resource.index()] = price;
            }
            Ok(())
        })?;
        self.next_id = plan.next_id;
        match plan.change {
            Change::Put(proposal) => self.put(proposal),
            Change::Remove(id) => {
                self.remove(id);
            }
        }
        Ok(plan.events)
    }

    /// Release expired promises before action application and automatic consumption.
    pub fn expire(&mut self, world: &mut World, tick: Tick) -> Result<Vec<Event>> {
        let expired: Vec<_> = self
            .proposals
            .values()
            .filter(|offer| offer.expires_at < tick)
            .cloned()
            .collect();
        let mut events = Vec::with_capacity(expired.len());
        for proposal in expired {
            world.with_agent_mut(proposal.proposer, |agent| {
                release(&mut agent.account, &proposal.terms)
            })?;
            self.remove(proposal.id);
            events.push(Event::ProposalClosed {
                proposal,
                outcome: ProposalOutcome::Expired,
            });
        }
        Ok(events)
    }

    /// Build bounded public context using trusted peers and a rotating stable ID neighborhood.
    pub fn observe(
        &self,
        world: &World,
        settings: &InteractionSettings,
        id: AgentId,
        tick: Tick,
    ) -> Result<Option<InteractionObservation>> {
        if !settings.negotiation && !settings.cooperation {
            return Ok(None);
        }
        let actor = world.agent(id)?;
        let mut trusted: Vec<_> = actor.relationships.iter().collect();
        trusted.sort_by_key(|(id, edge)| (std::cmp::Reverse(edge.trust), **id));
        let mut ids: Vec<_> = trusted
            .into_iter()
            .take(MAX_OBSERVED_PEERS / 2)
            .map(|(&id, _)| id)
            .collect();
        let start = AgentId(id.0.wrapping_add(1).wrapping_add(tick / 8));
        for other in world.ids_from(start) {
            if ids.len() >= MAX_OBSERVED_PEERS {
                break;
            }
            if other != id && !ids.contains(&other) {
                ids.push(other);
            }
        }
        let counterparties = ids
            .into_iter()
            .map(|id| {
                let agent = world.agent(id)?;
                Ok(CounterpartyView {
                    id,
                    occupation: agent.occupation,
                    available_inventory: Inventory::new(
                        Resource::ALL.map(|r| agent.account.available(r)),
                    ),
                    available_cash: agent.account.available_cash(),
                    energy: agent.needs.energy,
                    food_reserve: agent.food_reserve(),
                    open_proposals: self.count_for(id),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(InteractionObservation {
            settings: settings.clone(),
            proposals: self
                .for_agent(id)
                .filter(|p| p.expires_at >= tick && p.updated_at < tick)
                .cloned()
                .collect(),
            counterparties,
        }))
    }

    fn check_new_pair(&self, one: AgentId, two: AgentId) -> Result<()> {
        if one == two {
            return Err(invalid("self proposal is not allowed"));
        }
        if self.count_for(one) >= MAX_OPEN_PROPOSALS || self.count_for(two) >= MAX_OPEN_PROPOSALS {
            return Err(invalid("participant proposal capacity reached"));
        }
        if self
            .for_agent(one)
            .any(|p| p.proposer == two || p.recipient == two)
        {
            return Err(invalid(
                "a proposal between these participants is already open",
            ));
        }
        Ok(())
    }

    fn put(&mut self, proposal: Proposal) {
        self.remove(proposal.id);
        for id in [proposal.proposer, proposal.recipient] {
            self.participants.entry(id).or_default().insert(proposal.id);
        }
        self.proposals.insert(proposal.id, proposal);
    }

    fn remove(&mut self, id: u64) {
        if let Some(proposal) = self.proposals.remove(&id) {
            for agent in [proposal.proposer, proposal.recipient] {
                if let Some(ids) = self.participants.get_mut(&agent) {
                    ids.remove(&id);
                    if ids.is_empty() {
                        self.participants.remove(&agent);
                    }
                }
            }
        }
    }

    fn prepare(
        &self,
        world: &World,
        settings: &InteractionSettings,
        id: AgentId,
        action: &Action,
        tick: Tick,
    ) -> Result<Prepared> {
        let actor = world.agent(id)?;
        let existing = match *action {
            Action::CounterOffer { proposal_id, .. }
            | Action::AcceptProposal { proposal_id }
            | Action::RejectProposal { proposal_id }
            | Action::CancelProposal { proposal_id } => {
                let proposal = self
                    .proposals
                    .get(&proposal_id)
                    .ok_or_else(|| invalid("proposal is no longer open"))?;
                if proposal.expires_at < tick || proposal.updated_at >= tick {
                    return Err(invalid("proposal cannot be answered at this tick"));
                }
                let required = if matches!(action, Action::CancelProposal { .. }) {
                    proposal.proposer
                } else {
                    proposal.recipient
                };
                if required != id {
                    return Err(invalid(
                        "agent is not authorized to respond to this proposal",
                    ));
                }
                Some(proposal)
            }
            _ => None,
        };
        let other_id = match *action {
            Action::Negotiate { counterparty, .. } => counterparty,
            Action::Cooperate { partner, .. } => partner,
            _ => existing
                .map(|p| {
                    if p.proposer == id {
                        p.recipient
                    } else {
                        p.proposer
                    }
                })
                .ok_or_else(|| invalid("action does not use proposals"))?,
        };
        if other_id == id {
            return Err(invalid("self proposal is not allowed"));
        }
        let other = world
            .agent(other_id)
            .map_err(|_| invalid("unknown proposal counterparty"))?;
        actor.account.validate()?;
        other.account.validate()?;
        let mut plan = Prepared {
            actor: id,
            other: other_id,
            actor_account: actor.account.clone(),
            other_account: other.account.clone(),
            actor_energy: actor.needs.energy,
            other_energy: other.needs.energy,
            purchased: None,
            next_id: self.next_id,
            change: Change::Remove(0),
            events: Vec::new(),
        };
        match *action {
            Action::Negotiate {
                side,
                resource,
                quantity,
                unit_price,
                ..
            } => {
                if !settings.negotiation {
                    return Err(invalid("negotiation is disabled"));
                }
                validate_order_values(quantity, unit_price)?;
                let terms = ProposalTerms::Trade {
                    side,
                    resource,
                    quantity,
                    unit_price,
                };
                reserve(&mut plan.actor_account, &terms)?;
                self.open(&mut plan, settings, terms, tick)?;
            }
            Action::Cooperate { producer_share, .. } => {
                if !settings.cooperation {
                    return Err(invalid("cooperation is disabled"));
                }
                if actor.occupation != Occupation::Producer
                    || !(1..COOPERATIVE_GOODS).contains(&producer_share)
                {
                    return Err(invalid(
                        "joint production requires a producer and positive shares of four goods",
                    ));
                }
                if plan.actor_energy < COOPERATIVE_LABOR {
                    return Err(invalid("insufficient preparation energy"));
                }
                let terms = ProposalTerms::Cooperation { producer_share };
                reserve(&mut plan.actor_account, &terms)?;
                plan.actor_energy -= COOPERATIVE_LABOR;
                self.open(&mut plan, settings, terms, tick)?;
            }
            Action::CounterOffer {
                quantity,
                unit_price,
                ..
            } => {
                let previous = existing.expect("response checked above");
                let ProposalTerms::Trade { side, resource, .. } = previous.terms else {
                    return Err(invalid("only trade proposals can be countered"));
                };
                if !settings.negotiation || previous.counteroffers >= MAX_COUNTEROFFERS {
                    return Err(invalid(
                        "counteroffer limit reached or negotiation disabled",
                    ));
                }
                validate_order_values(quantity, unit_price)?;
                let terms = ProposalTerms::Trade {
                    side: match side {
                        OrderSide::Buy => OrderSide::Sell,
                        OrderSide::Sell => OrderSide::Buy,
                    },
                    resource,
                    quantity,
                    unit_price,
                };
                release(&mut plan.other_account, &previous.terms)?;
                reserve(&mut plan.actor_account, &terms)?;
                let proposal = Proposal {
                    proposer: id,
                    recipient: other_id,
                    terms,
                    updated_at: tick,
                    counteroffers: previous.counteroffers + 1,
                    ..previous.clone()
                };
                plan.events.push(Event::ProposalCountered {
                    proposal: proposal.clone(),
                });
                plan.change = Change::Put(proposal);
            }
            Action::AcceptProposal { .. } => {
                let proposal = existing.expect("response checked above");
                self.accept(&mut plan, proposal, settings)?;
                plan.change = Change::Remove(proposal.id);
            }
            Action::RejectProposal { .. } | Action::CancelProposal { .. } => {
                let proposal = existing.expect("response checked above");
                let account = if proposal.proposer == id {
                    &mut plan.actor_account
                } else {
                    &mut plan.other_account
                };
                release(account, &proposal.terms)?;
                let outcome = if proposal.proposer == id {
                    ProposalOutcome::Cancelled
                } else {
                    ProposalOutcome::Rejected
                };
                plan.events.push(Event::ProposalClosed {
                    proposal: proposal.clone(),
                    outcome,
                });
                plan.change = Change::Remove(proposal.id);
            }
            _ => return Err(invalid("action does not use proposals")),
        }
        plan.actor_account.validate()?;
        plan.other_account.validate()?;
        Ok(plan)
    }

    fn open(
        &self,
        plan: &mut Prepared,
        settings: &InteractionSettings,
        terms: ProposalTerms,
        tick: Tick,
    ) -> Result<()> {
        self.check_new_pair(plan.actor, plan.other)?;
        let expires_at = tick
            .checked_add(settings.offer_lifetime_ticks)
            .ok_or(DomainError::Overflow("proposal expiry"))?;
        plan.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(DomainError::Overflow("proposal ID"))?;
        let proposal = Proposal {
            id: self.next_id,
            proposer: plan.actor,
            recipient: plan.other,
            terms,
            created_at: tick,
            updated_at: tick,
            expires_at,
            counteroffers: 0,
        };
        plan.events.push(Event::ProposalOpened {
            proposal: proposal.clone(),
        });
        plan.change = Change::Put(proposal);
        Ok(())
    }

    fn accept(
        &self,
        plan: &mut Prepared,
        proposal: &Proposal,
        settings: &InteractionSettings,
    ) -> Result<()> {
        match proposal.terms {
            ProposalTerms::Trade {
                side,
                resource,
                quantity,
                unit_price,
            } => {
                if !settings.negotiation {
                    return Err(invalid("negotiation is disabled"));
                }
                let (buyer, seller) = match side {
                    OrderSide::Sell => {
                        plan.actor_account.reserve_buy(unit_price, quantity)?;
                        settle_trade(
                            &mut plan.actor_account,
                            &mut plan.other_account,
                            resource,
                            quantity,
                            unit_price,
                            unit_price,
                        )?;
                        (plan.actor, plan.other)
                    }
                    OrderSide::Buy => {
                        plan.actor_account.reserve_sell(resource, quantity)?;
                        settle_trade(
                            &mut plan.other_account,
                            &mut plan.actor_account,
                            resource,
                            quantity,
                            unit_price,
                            unit_price,
                        )?;
                        (plan.other, plan.actor)
                    }
                };
                plan.purchased = Some((buyer, resource, unit_price));
                plan.events.push(Event::NegotiatedTrade {
                    proposal_id: proposal.id,
                    buyer,
                    seller,
                    resource,
                    quantity,
                    price: unit_price,
                });
            }
            ProposalTerms::Cooperation { producer_share } => {
                if !settings.cooperation {
                    return Err(invalid("cooperation is disabled"));
                }
                if plan.actor_energy < COOPERATIVE_LABOR {
                    return Err(invalid("insufficient cooperative labor energy"));
                }
                let partner_share = COOPERATIVE_GOODS - producer_share;
                plan.other_account
                    .release_sell(Resource::Metal, COOPERATIVE_METAL)?;
                plan.other_account
                    .consume(Resource::Metal, COOPERATIVE_METAL)?;
                plan.actor_account
                    .consume(Resource::Wood, COOPERATIVE_WOOD)?;
                plan.other_account
                    .produce(Resource::Goods, producer_share)?;
                plan.actor_account.produce(Resource::Goods, partner_share)?;
                plan.actor_energy -= COOPERATIVE_LABOR;
                for (agent_id, resource, quantity) in [
                    (plan.other, Resource::Metal, COOPERATIVE_METAL),
                    (plan.actor, Resource::Wood, COOPERATIVE_WOOD),
                ] {
                    plan.events.push(Event::ResourceConsumed {
                        agent_id,
                        resource,
                        quantity,
                        purpose: ConsumptionPurpose::ProductionInput,
                    });
                }
                for (agent_id, quantity) in
                    [(plan.other, producer_share), (plan.actor, partner_share)]
                {
                    plan.events.push(Event::ResourceProduced {
                        agent_id,
                        resource: Resource::Goods,
                        quantity,
                    });
                }
                plan.events.push(Event::CooperationCompleted {
                    proposal_id: proposal.id,
                    producer: plan.other,
                    partner: plan.actor,
                    producer_share,
                    partner_share,
                });
            }
        }
        plan.events.push(Event::ProposalClosed {
            proposal: proposal.clone(),
            outcome: ProposalOutcome::Accepted,
        });
        Ok(())
    }
}

fn reserve(account: &mut Account, terms: &ProposalTerms) -> Result<()> {
    match *terms {
        ProposalTerms::Trade {
            side: OrderSide::Buy,
            quantity,
            unit_price,
            ..
        } => account.reserve_buy(unit_price, quantity),
        ProposalTerms::Trade {
            side: OrderSide::Sell,
            resource,
            quantity,
            ..
        } => account.reserve_sell(resource, quantity),
        ProposalTerms::Cooperation { .. } => {
            account.reserve_sell(Resource::Metal, COOPERATIVE_METAL)
        }
    }
}

fn release(account: &mut Account, terms: &ProposalTerms) -> Result<()> {
    match *terms {
        ProposalTerms::Trade {
            side: OrderSide::Buy,
            quantity,
            unit_price,
            ..
        } => account.release_buy(unit_price, quantity),
        ProposalTerms::Trade {
            side: OrderSide::Sell,
            resource,
            quantity,
            ..
        } => account.release_sell(resource, quantity),
        ProposalTerms::Cooperation { .. } => {
            account.release_sell(Resource::Metal, COOPERATIVE_METAL)
        }
    }
}
