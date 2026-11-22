use super::*;
impl InstitutionBook {
    pub(super) fn prepare_group(
        &self,
        world: &World,
        settings: &OrganizationSettings,
        actor: AgentId,
        operation: &InstitutionAction,
        tick: Tick,
    ) -> Result<Plan> {
        let mut plan = self.blank_plan();
        let mut individual = world.agent(actor)?.account.clone();
        if matches!(operation, InstitutionAction::Found {}) {
            if self.membership(actor).is_some() {
                return Err(invalid("already a cooperative member"));
            }
            let id = OrganizationId(self.next_organization);
            let mut account = Account::new(0, Inventory::default());
            transfer_assets(
                &mut individual,
                &mut account,
                settings.membership_stake,
                &Inventory::default(),
            )?;
            plan.next_organization = self
                .next_organization
                .checked_add(1)
                .ok_or(DomainError::Overflow("organization ID"))?;
            plan.group = Some((
                id,
                Some(Organization {
                    id,
                    leader: actor,
                    members: BTreeSet::from([actor]),
                    account,
                    founded_at: tick,
                    last_production: 0,
                }),
            ));
            plan.accounts.push((Party::Agent(actor), individual));
            plan.events.push(Event::Organization {
                event: OrganizationEvent::Founded {
                    organization: id,
                    founder: actor,
                    stake: settings.membership_stake,
                },
            });
            return Ok(plan);
        }
        let id = match *operation {
            InstitutionAction::Join { organization }
            | InstitutionAction::Leave { organization }
            | InstitutionAction::Contribute { organization, .. }
            | InstitutionAction::Distribute { organization, .. } => organization,
            _ => return Err(invalid("not a membership operation")),
        };
        let mut group = self.organization(id)?.clone();
        match operation {
            InstitutionAction::Join { .. } => {
                if self.membership(actor).is_some() || group.members.len() >= settings.max_members {
                    return Err(invalid("membership or group capacity unavailable"));
                }
                if self.contracts_for(Party::Organization(id)).next().is_some() {
                    return Err(invalid("membership is fixed while agreements are open"));
                }
                transfer_assets(
                    &mut individual,
                    &mut group.account,
                    settings.membership_stake,
                    &Inventory::default(),
                )?;
                group.members.insert(actor);
                plan.events.push(Event::Organization {
                    event: OrganizationEvent::Joined {
                        organization: id,
                        agent_id: actor,
                        stake: settings.membership_stake,
                    },
                });
            }
            InstitutionAction::Leave { .. } => {
                if self.membership(actor) != Some(id)
                    || self.contracts_for(Party::Organization(id)).next().is_some()
                {
                    return Err(invalid("only uncommitted members may leave"));
                }
                let count = group.members.len() as u64;
                let cash = group.account.wallet() / count;
                let inventory =
                    Inventory::new(Resource::ALL.map(|r| group.account.inventory().get(r) / count));
                transfer_assets(&mut group.account, &mut individual, cash, &inventory)?;
                group.members.remove(&actor);
                if group.leader == actor
                    && let Some(&successor) = group.members.first()
                {
                    group.leader = successor;
                }
                plan.events.push(Event::Organization {
                    event: OrganizationEvent::Left {
                        organization: id,
                        agent_id: actor,
                        cash,
                        inventory,
                        leader: (!group.members.is_empty()).then_some(group.leader),
                    },
                });
                if group.members.is_empty() {
                    let mut retired = self.retired.clone();
                    retired.absorb_closed_history(&group.account)?;
                    plan.retired = Some(retired);
                    plan.group = Some((id, None));
                    plan.accounts.push((Party::Agent(actor), individual));
                    return Ok(plan);
                }
            }
            InstitutionAction::Contribute {
                cash, inventory, ..
            } => {
                if self.membership(actor) != Some(id)
                    || (*cash == 0 && *inventory == Inventory::default())
                    || *cash > 1_000_000_000
                    || inventory.amounts().iter().any(|&q| q > MAX_ORDER_QUANTITY)
                {
                    return Err(invalid("invalid member contribution"));
                }
                transfer_assets(&mut individual, &mut group.account, *cash, inventory)?;
                plan.events.push(Event::Organization {
                    event: OrganizationEvent::Contributed {
                        organization: id,
                        agent_id: actor,
                        cash: *cash,
                        inventory: *inventory,
                    },
                });
            }
            InstitutionAction::Distribute {
                cash_per_member,
                goods_per_member,
                ..
            } => {
                if group.leader != actor
                    || (*cash_per_member == 0 && *goods_per_member == 0)
                    || *cash_per_member > 1_000_000_000
                    || *goods_per_member > MAX_ORDER_QUANTITY
                {
                    return Err(invalid(
                        "only a leader may authorize bounded equal dividends",
                    ));
                }
                let inventory = Inventory::new([0, 0, 0, *goods_per_member]);
                for &member in &group.members {
                    let mut account = world.agent(member)?.account.clone();
                    transfer_assets(
                        &mut group.account,
                        &mut account,
                        *cash_per_member,
                        &inventory,
                    )?;
                    plan.accounts.push((Party::Agent(member), account));
                }
                plan.events.push(Event::Organization {
                    event: OrganizationEvent::Distributed {
                        organization: id,
                        members: group.members.iter().copied().collect(),
                        cash_per_member: *cash_per_member,
                        goods_per_member: *goods_per_member,
                    },
                });
                plan.group = Some((id, Some(group)));
                return Ok(plan);
            }
            _ => return Err(invalid("not a membership operation")),
        }
        plan.accounts.push((Party::Agent(actor), individual));
        plan.group = Some((id, Some(group)));
        Ok(plan)
    }

    /// The charter commits three eligible members to periodic work; no resources are gifted.
    pub fn produce(
        &mut self,
        world: &mut World,
        settings: &OrganizationSettings,
        tick: Tick,
    ) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        if !settings.enabled {
            return Ok(events);
        }
        for group in self.organizations.values_mut() {
            if group.members.len() < 3
                || group.last_production >= tick
                || !(tick.wrapping_add(group.id.0)).is_multiple_of(settings.production_interval)
                || group.account.available(Resource::Wood) < 4
                || group.account.available(Resource::Metal) < 2
            {
                continue;
            }
            let mut eligible: Vec<_> = group
                .members
                .iter()
                .copied()
                .filter(|&id| {
                    world.agent(id).is_ok_and(|a| {
                        a.needs.energy >= 5 && a.account.available(Resource::Food) >= 3
                    })
                })
                .collect();
            let Some(producer) = eligible.iter().copied().find(|&id| {
                world
                    .agent(id)
                    .is_ok_and(|a| a.occupation == Occupation::Producer)
            }) else {
                continue;
            };
            if eligible.len() < 3 {
                continue;
            }
            eligible.retain(|&id| id != producer);
            let offset = (tick % eligible.len() as u64) as usize;
            eligible.rotate_left(offset);
            let workers = vec![producer, eligible[0], eligible[1]];
            let mut account = group.account.clone();
            account.transform(
                &[(Resource::Wood, 4), (Resource::Metal, 2)],
                Resource::Goods,
                6,
            )?;
            for &worker in &workers {
                world.with_agent_mut(worker, |a| {
                    a.needs.energy -= 5;
                    Ok(())
                })?;
            }
            group.account = account;
            group.last_production = tick;
            events.push(Event::Organization {
                event: OrganizationEvent::Produced {
                    organization: group.id,
                    workers,
                    inputs: Inventory::new([0, 4, 2, 0]),
                    goods: 6,
                },
            });
        }
        Ok(events)
    }
}
