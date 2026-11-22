use super::*;

#[derive(Default)]
struct Feedback {
    rejected: u64,
    trades: u64,
    cooperations: u64,
}

impl Simulation {
    pub(super) fn review_goals(&mut self, events: &[Event], tick: Tick) -> Result<Vec<Event>> {
        let interval = self.config.interactions.goal_review_interval;
        if interval == 0 {
            return Ok(Vec::new());
        }
        let mut feedback: BTreeMap<AgentId, Feedback> = BTreeMap::new();
        let equity = self.institutions.equity(&self.config.initial_prices);
        for event in events {
            match *event {
                Event::Contract {
                    event:
                        ContractEvent::Delivered {
                            ref participants, ..
                        },
                } => {
                    for &id in participants {
                        feedback.entry(id).or_default().trades += 1;
                    }
                }
                Event::Organization {
                    event: OrganizationEvent::Produced { ref workers, .. },
                } => {
                    for &id in workers {
                        feedback.entry(id).or_default().cooperations += 1;
                    }
                }
                Event::TradeExecuted { buyer, seller, .. }
                | Event::NegotiatedTrade { buyer, seller, .. } => {
                    feedback.entry(buyer).or_default().trades += 1;
                    feedback.entry(seller).or_default().trades += 1;
                }
                Event::CooperationCompleted {
                    producer, partner, ..
                } => {
                    feedback.entry(producer).or_default().cooperations += 1;
                    feedback.entry(partner).or_default().cooperations += 1;
                }
                Event::ActionRejected { agent_id, .. } => {
                    feedback.entry(agent_id).or_default().rejected += 1
                }
                _ => {}
            }
        }
        let ids: Vec<_> = self.world.ids().collect();
        let mut changes = Vec::new();
        for id in ids {
            let recent = feedback.remove(&id).unwrap_or_default();
            self.world.with_agent_mut(id, |agent| {
                let progress = &mut agent.goal_progress;
                progress.hungry_ticks += u64::from(agent.needs.hunger > 0);
                progress.rejected_actions += recent.rejected;
                progress.successful_trades += recent.trades;
                progress.cooperations += recent.cooperations;
                let elapsed_ticks = tick - progress.last_review;
                if elapsed_ticks < interval {
                    return Ok(());
                }
                let wealth = agent.account.wealth(&self.config.initial_prices)
                    + equity.get(&id).copied().unwrap_or(0);
                let wealth_declined = wealth
                    .checked_mul(100)
                    .ok_or(DomainError::Overflow("goal wealth comparison"))?
                    < progress
                        .reference_wealth
                        .checked_mul(85)
                        .ok_or(DomainError::Overflow("goal wealth comparison"))?;
                let evidence = GoalAssessment {
                    elapsed_ticks,
                    hungry_ticks: progress.hungry_ticks,
                    rejected_actions: progress.rejected_actions,
                    successful_trades: progress.successful_trades,
                    cooperations: progress.cooperations,
                    wealth_declined,
                };
                if let Some((goal, reason)) = agentsociety_cognition::review_goal(agent, &evidence)
                {
                    let previous = agent.goal;
                    agent.goal = goal;
                    agent.goal_progress.last_change = tick;
                    changes.push(Event::GoalChanged {
                        agent_id: id,
                        previous,
                        goal,
                        reason,
                        assessment: evidence,
                    });
                }
                agent.goal_progress = GoalProgress {
                    last_review: tick,
                    last_change: agent.goal_progress.last_change,
                    ..GoalProgress::new(wealth)
                };
                Ok(())
            })?;
        }
        Ok(changes)
    }
}

pub(super) fn validate_progress(
    agent: &Agent,
    original: &Agent,
    config: &SimulationConfig,
    tick: Tick,
    cumulative: &FlowTotals,
    maximum_wealth: u128,
    equity: u128,
) -> Result<()> {
    let invalid = || DomainError::Invariant("invalid goal progress or adaptive objective".into());
    let progress = &agent.goal_progress;
    let interval = config.interactions.goal_review_interval;
    if interval == 0 {
        return if progress == &original.goal_progress && agent.goal == original.goal {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    let expected_review = tick - tick % interval;
    if progress.last_review != expected_review
        || progress.last_change > expected_review
        || !progress.last_change.is_multiple_of(interval)
        || progress.hungry_ticks > tick - expected_review
        || progress.rejected_actions > tick - expected_review
        || progress.successful_trades > cumulative.trade_count
        || u128::from(progress.cooperations)
            > u128::from(cumulative.cooperations) + u128::from(cumulative.group_productions)
        || progress.reference_wealth > maximum_wealth
        || (expected_review == 0
            && progress.reference_wealth != original.goal_progress.reference_wealth)
        || (progress.last_change == 0 && agent.goal != original.goal)
    {
        return Err(invalid());
    }
    if tick == expected_review
        && (progress.hungry_ticks != 0
            || progress.rejected_actions != 0
            || progress.successful_trades != 0
            || progress.cooperations != 0
            || progress.reference_wealth != agent.account.wealth(&config.initial_prices) + equity)
    {
        return Err(invalid());
    }
    Ok(())
}
