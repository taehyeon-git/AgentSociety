use super::*;

impl Simulation {
    /// Validate and restore a completed state, including the exact RNG position.
    /// Unsupported versions, inconsistent statistics and partial ticks are rejected.
    pub fn from_snapshot(snapshot: SimulationSnapshot) -> Result<Self> {
        let invalid = |message: &str| {
            DomainError::Invariant(format!("invalid simulation snapshot: {message}"))
        };
        if snapshot.format_version != FORMAT_VERSION || snapshot.engine_version != ENGINE_VERSION {
            return Err(invalid("unsupported format or engine version"));
        }
        snapshot.config.validate()?;
        let initial = Self::new(snapshot.config.clone())?;
        let tick = snapshot.world.tick;
        let agent_ticks = u128::from(tick) * snapshot.config.agents as u128;
        if snapshot.random.seed != initial.rng.get_seed()
            || snapshot.random.stream != 1
            || snapshot.random.word_position != (agent_ticks * 2) % (1_u128 << 68)
        {
            return Err(invalid(
                "RNG seed, stream or position disagrees with completed ticks",
            ));
        }
        if snapshot.baseline != initial.baseline
            || u128::from(snapshot.next_event_sequence)
                < agent_ticks + u128::from(tick) + snapshot.config.agents as u128
            || u128::from(snapshot.market.next_order_id) > agent_ticks
        {
            return Err(invalid(
                "initial conservation baseline or sequence counters disagree",
            ));
        }
        if tick == 0 && snapshot != initial.snapshot()? {
            return Err(invalid(
                "tick-zero state differs from seeded initialization",
            ));
        }
        let world = World::from_snapshot(&snapshot.config, snapshot.world)?;
        let institutions = InstitutionBook::from_snapshot(
            &world,
            &snapshot.config.organizations,
            snapshot.institutions,
        )?;
        validate_scenario(&snapshot.config.scenario, &snapshot.scenario, tick)?;
        let proposals = ProposalBook::from_snapshot_with_cash(
            &world,
            &snapshot.config.interactions,
            snapshot.proposals,
            &institutions.personal_collateral()?,
        )?;
        let settings = &snapshot.config.interactions;
        let flows = &snapshot.cumulative;
        let (groups_created, contracts_created) = institutions.next_ids();
        if groups_created != flows.organizations_formed
            || contracts_created != flows.contracts_offered
            || flows.contract_defaults > flows.contracts_offered
            || flows.contract_deliveries > flows.trade_count
            || u128::from(flows.contract_deliveries) > u128::from(flows.contracts_offered) * 32
            || flows.scenario_events != snapshot.scenario.next_event as u64
            || flows.taxes != snapshot.scenario.treasury.transfers_in()
            || flows.redistributed != snapshot.scenario.treasury.transfers_out()
            || (!snapshot.config.organizations.enabled
                && (flows.group_productions != 0 || flows.compensation != 0))
        {
            return Err(invalid(
                "institutional or fiscal totals disagree with saved state",
            ));
        }
        let joint_offers = proposals.next_id().checked_sub(flows.negotiations);
        let live_trades = proposals
            .proposals()
            .filter(|p| matches!(p.terms, ProposalTerms::Trade { .. }))
            .count() as u64;
        let live_joint = proposals.proposals().count() as u64 - live_trades;
        let review_opportunities =
            u128::from(tick.checked_div(settings.goal_review_interval).unwrap_or(0))
                * world.len() as u128;
        if flows.negotiated_trades > flows.trade_count
            || flows.negotiated_trades > flows.negotiations
            || live_trades > flows.negotiations.saturating_sub(flows.negotiated_trades)
            || joint_offers.is_none_or(|offers| {
                flows.cooperations > offers
                    || live_joint > offers.saturating_sub(flows.cooperations)
            })
            || (!settings.negotiation && flows.negotiations != 0)
            || (!settings.cooperation && joint_offers != Some(0))
            || u128::from(flows.goal_changes) > review_opportunities
            || world
                .agents()
                .filter(|a| a.goal_progress.last_change > 0)
                .count() as u64
                > flows.goal_changes
            || (settings.goal_review_interval != 0
                && !tick.is_multiple_of(settings.goal_review_interval)
                && snapshot.stats.flows.goal_changes != 0)
        {
            return Err(invalid(
                "interaction totals disagree with proposals or goal reviews",
            ));
        }
        let mut maximum_wealth = snapshot.baseline.total_money;
        for resource in Resource::ALL {
            let index = resource.index();
            let value = snapshot.baseline.inventories[index]
                .checked_add(snapshot.cumulative.produced[index])
                .and_then(|quantity| {
                    quantity.checked_mul(u128::from(snapshot.config.initial_prices[index]))
                })
                .ok_or(DomainError::Overflow("goal reference wealth bound"))?;
            maximum_wealth = maximum_wealth
                .checked_add(value)
                .ok_or(DomainError::Overflow("goal reference wealth bound"))?;
        }
        let equity = institutions.equity(&snapshot.config.initial_prices);
        let valid_party = |party: Party| match party {
            Party::Agent(id) => id.0 < world.len() as u64,
            Party::Organization(id) => id.0 < institutions.next_ids().0,
        };
        for (agent, original) in world.agents().zip(initial.world.agents()) {
            if agent.id != original.id
                || agent.name != original.name
                || agent.age != original.age
                || agent.occupation != original.occupation
                || agent.personality != original.personality
                || agent.skills != original.skills
                || agent.policy != original.policy
            {
                return Err(invalid(
                    "immutable agent identity differs from initialization",
                ));
            }
            adaptation::validate_progress(
                agent,
                original,
                &snapshot.config,
                tick,
                &snapshot.cumulative,
                maximum_wealth,
                equity.get(&agent.id).copied().unwrap_or(0),
            )?;
            for memory in &agent.memories {
                match &memory.content {
                    MemoryContent::Organization { event }
                        if event.organization().0 >= institutions.next_ids().0 =>
                    {
                        return Err(invalid("memory references a future organization"));
                    }
                    MemoryContent::Contract { event }
                        if event.contract_id() >= institutions.next_ids().1
                            || !event.parties().into_iter().all(valid_party) =>
                    {
                        return Err(invalid(
                            "memory references a future contract or unknown owner",
                        ));
                    }
                    MemoryContent::Scenario {
                        event:
                            ScenarioEvent::InventoryLost { owner, .. }
                            | ScenarioEvent::TaxCollected { payer: owner, .. },
                    } if !valid_party(*owner) => {
                        return Err(invalid("scenario memory references an unknown owner"));
                    }
                    MemoryContent::GoalAssigned { goal } if *goal != original.goal => {
                        return Err(invalid("initial goal memory changed"));
                    }
                    MemoryContent::GoalChanged { .. }
                        if settings.goal_review_interval == 0
                            || !memory.tick.is_multiple_of(settings.goal_review_interval)
                            || memory.tick > agent.goal_progress.last_change =>
                    {
                        return Err(invalid(
                            "goal memory is outside the recorded review history",
                        ));
                    }
                    MemoryContent::GoalChanged { goal, .. }
                        if memory.tick == agent.goal_progress.last_change
                            && *goal != agent.goal =>
                    {
                        return Err(invalid("latest goal memory disagrees with objective"));
                    }
                    MemoryContent::Proposal { proposal_id, .. }
                    | MemoryContent::Cooperated { proposal_id, .. }
                        if *proposal_id >= proposals.next_id() =>
                    {
                        return Err(invalid("memory references a future proposal"));
                    }
                    _ => {}
                }
            }
            agent.account.validate_ledger(original.account.wallet())?;
            if agent
                .memories
                .iter()
                .any(|memory| memory.id.0 >= snapshot.next_event_sequence)
            {
                return Err(invalid("memory refers to a future event sequence"));
            }
        }
        audit_ledger_totals(&world, &institutions, &snapshot.scenario.treasury, flows)?;
        if !flows_within(&snapshot.stats.flows, &snapshot.cumulative) {
            return Err(invalid(
                "cumulative flows disagree with account history or latest flows",
            ));
        }
        let market = OrderBook::from_snapshot(snapshot.market)?;
        let groups: Vec<_> = institutions.organizations().collect();
        let stats = collect_with_ownership(
            &world,
            *market.prices(),
            snapshot.stats.flows.clone(),
            &groups,
            &snapshot.scenario.treasury,
            institutions.contracts().len(),
        )?;
        if stats != snapshot.stats {
            return Err(invalid("saved statistics differ from the restored world"));
        }
        audit_balances(&world, &stats, &snapshot.baseline, &snapshot.cumulative)?;
        let mut rng = ChaCha8Rng::from_seed(snapshot.random.seed);
        rng.set_stream(snapshot.random.stream);
        rng.set_word_pos(snapshot.random.word_position);
        Ok(Self {
            config: snapshot.config,
            world,
            market,
            proposals,
            institutions,
            scenario: snapshot.scenario,
            rng,
            baseline: snapshot.baseline,
            cumulative: snapshot.cumulative,
            stats,
            // A restored run never emits initialization events for a second time.
            initial_events: Vec::new(),
            next_event_sequence: snapshot.next_event_sequence,
            failed: false,
        })
    }

    /// Next global event sequence, also the total number of committed events.
    pub const fn next_event_sequence(&self) -> u64 {
        self.next_event_sequence
    }

    /// Create a new run from this exact state with a replacement future intervention schedule.
    /// Completed interventions, live shocks, property and random streams are retained.
    pub fn branch_scenario(&self, future: ScenarioConfig) -> Result<Self> {
        future.validate()?;
        if future.events.iter().any(|e| e.tick <= self.world.tick()) {
            return Err(DomainError::InvalidConfig(
                "branch interventions must be after the checkpoint tick".into(),
            ));
        }
        let mut snapshot = self.snapshot()?;
        let mut events: Vec<_> = snapshot
            .config
            .scenario
            .events
            .into_iter()
            .take_while(|e| e.tick <= self.world.tick())
            .collect();
        events.extend(future.events);
        snapshot.config.scenario = ScenarioConfig {
            name: future.name,
            events,
        };
        Self::from_snapshot(snapshot)
    }
}

pub(super) fn audit_ledger_totals(
    world: &World,
    institutions: &InstitutionBook,
    treasury: &Account,
    flows: &FlowTotals,
) -> Result<()> {
    let mut totals = [0_u128; 4];
    for account in world
        .agents()
        .map(|a| &a.account)
        .chain(institutions.organizations().map(|g| &g.account))
        .chain([institutions.retired(), treasury])
    {
        for (total, value) in totals.iter_mut().zip([
            account.earned(),
            account.spent(),
            account.transfers_in(),
            account.transfers_out(),
        ]) {
            *total = total
                .checked_add(value)
                .ok_or(DomainError::Overflow("aggregate cash ledger"))?;
        }
    }
    if totals[0] != flows.trade_value || totals[1] != flows.trade_value || totals[2] != totals[3] {
        return Err(DomainError::Invariant(
            "sales or non-sale transfers disagree across account ledgers".into(),
        ));
    }
    Ok(())
}

fn flows_within(recent: &FlowTotals, total: &FlowTotals) -> bool {
    [
        (&recent.produced, &total.produced),
        (&recent.household_consumed, &total.household_consumed),
        (&recent.production_inputs, &total.production_inputs),
        (&recent.traded, &total.traded),
        (&recent.buy_demand, &total.buy_demand),
        (&recent.sell_supply, &total.sell_supply),
        (&recent.destroyed, &total.destroyed),
    ]
    .into_iter()
    .all(|(left, right)| left.iter().zip(right).all(|(a, b)| a <= b))
        && recent.trade_count <= total.trade_count
        && recent.trade_value <= total.trade_value
        && recent.rejected_actions <= total.rejected_actions
        && recent.negotiations <= total.negotiations
        && recent.negotiated_trades <= total.negotiated_trades
        && recent.cooperations <= total.cooperations
        && recent.goal_changes <= total.goal_changes
        && recent.organizations_formed <= total.organizations_formed
        && recent.group_productions <= total.group_productions
        && recent.contracts_offered <= total.contracts_offered
        && recent.contract_deliveries <= total.contract_deliveries
        && recent.contract_defaults <= total.contract_defaults
        && recent.scenario_events <= total.scenario_events
        && recent.taxes <= total.taxes
        && recent.redistributed <= total.redistributed
        && recent.compensation <= total.compensation
}
