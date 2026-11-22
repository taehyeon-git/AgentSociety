use super::*;
use crate::validate_order_values;
impl InstitutionBook {
    pub(super) fn check_pair(&self, world: &World, one: Party, two: Party) -> Result<()> {
        self.account(world, one)?;
        self.account(world, two)?;
        if one == two
            || self
                .owners(one)?
                .iter()
                .any(|id| self.owners(two).is_ok_and(|other| other.contains(id)))
        {
            return Err(invalid("self or affiliated contract is not allowed"));
        }
        if [one, two]
            .iter()
            .any(|&p| self.contracts_for(p).count() >= MAX_CONTRACTS)
        {
            return Err(invalid("contract capacity reached"));
        }
        if self
            .contracts_for(one)
            .any(|c| c.proposer == two || c.recipient == two)
        {
            return Err(invalid("an agreement between these owners is already open"));
        }
        Ok(())
    }
    pub(super) fn prepare_contract(
        &self,
        world: &World,
        actor: AgentId,
        operation: &InstitutionAction,
        tick: Tick,
    ) -> Result<Plan> {
        let mut plan = self.blank_plan();
        if let InstitutionAction::OfferContract {
            organization,
            counterparty,
            side,
            resource,
            quantity,
            unit_price,
            deliveries,
            interval_ticks,
        } = *operation
        {
            let proposer = organization.map_or(Party::Agent(actor), Party::Organization);
            self.authorize(world, actor, proposer)?;
            self.check_pair(world, proposer, counterparty)?;
            validate_order_values(quantity, unit_price)?;
            if !(2..=32).contains(&deliveries) || !(1..=64).contains(&interval_ticks) {
                return Err(invalid(
                    "contracts require 2..=32 installments at 1..=64 tick intervals",
                ));
            }
            let total = notional(unit_price, quantity)?
                .checked_mul(u64::from(deliveries))
                .ok_or(DomainError::Overflow("contract total"))?;
            let bond = total.div_ceil(10).max(1);
            let expires_at = tick
                .checked_add(4)
                .ok_or(DomainError::Overflow("contract offer deadline"))?;
            expires_at
                .checked_add(interval_ticks * u64::from(deliveries))
                .ok_or(DomainError::Overflow("final delivery deadline"))?;
            let contract = DeliveryContract {
                id: self.next_contract,
                proposer,
                recipient: counterparty,
                side,
                resource,
                quantity,
                unit_price,
                deliveries,
                remaining: deliveries,
                interval_ticks,
                created_at: tick,
                expires_at,
                accepted_at: None,
                next_delivery: None,
                bond,
            };
            let mut account = self.account(world, proposer)?.clone();
            account.reserve_buy(if side == OrderSide::Buy { total } else { bond }, 1)?;
            plan.accounts.push((proposer, account));
            plan.next_contract = self
                .next_contract
                .checked_add(1)
                .ok_or(DomainError::Overflow("contract ID"))?;
            plan.events.push(Event::Contract {
                event: ContractEvent::Offered {
                    participants: self.participants(&contract)?,
                    contract: contract.clone(),
                },
            });
            plan.agreement = Some((contract.id, Some(contract)));
            return Ok(plan);
        }
        let id = match *operation {
            InstitutionAction::AcceptContract { contract_id }
            | InstitutionAction::RejectContract { contract_id }
            | InstitutionAction::CancelContract { contract_id } => contract_id,
            _ => return Err(invalid("not a contract operation")),
        };
        let contract = self
            .contracts
            .get(&id)
            .ok_or_else(|| invalid("contract is no longer open"))?;
        if contract.accepted_at.is_some()
            || contract.created_at >= tick
            || contract.expires_at < tick
        {
            return Err(invalid(
                "only an observed pending agreement may be answered",
            ));
        }
        let party = if matches!(operation, InstitutionAction::CancelContract { .. }) {
            contract.proposer
        } else {
            contract.recipient
        };
        self.authorize(world, actor, party)?;
        if matches!(operation, InstitutionAction::AcceptContract { .. }) {
            let mut account = self.account(world, party)?.clone();
            account.reserve_buy(
                if party == contract.buyer() {
                    contract.outstanding()?
                } else {
                    contract.bond
                },
                1,
            )?;
            let mut accepted = contract.clone();
            accepted.accepted_at = Some(tick);
            accepted.next_delivery = Some(
                tick.checked_add(contract.interval_ticks)
                    .ok_or(DomainError::Overflow("delivery deadline"))?,
            );
            tick.checked_add(contract.interval_ticks * u64::from(contract.deliveries))
                .ok_or(DomainError::Overflow("final delivery deadline"))?;
            plan.accounts.push((party, account));
            plan.events.push(Event::Contract {
                event: ContractEvent::Accepted {
                    participants: self.participants(contract)?,
                    contract: accepted.clone(),
                },
            });
            plan.agreement = Some((id, Some(accepted)));
            Ok(plan)
        } else {
            self.close_plan(
                world,
                contract,
                if party == contract.proposer {
                    ContractOutcome::Cancelled
                } else {
                    ContractOutcome::Rejected
                },
            )
        }
    }
    fn close_plan(
        &self,
        world: &World,
        contract: &DeliveryContract,
        outcome: ContractOutcome,
    ) -> Result<Plan> {
        let mut plan = self.blank_plan();
        let compensation = if contract.accepted_at.is_some() {
            let buyer = contract.buyer();
            let seller = contract.seller();
            let mut buyer_account = self.account(world, buyer)?.clone();
            let mut seller_account = self.account(world, seller)?.clone();
            buyer_account.release_buy(contract.outstanding()?, 1)?;
            seller_account.release_buy(contract.bond, 1)?;
            let paid = if outcome == ContractOutcome::Defaulted {
                contract.bond
            } else {
                0
            };
            transfer_assets(
                &mut seller_account,
                &mut buyer_account,
                paid,
                &Inventory::default(),
            )?;
            plan.accounts.push((buyer, buyer_account));
            plan.accounts.push((seller, seller_account));
            paid
        } else {
            let mut account = self.account(world, contract.proposer)?.clone();
            account.release_buy(
                if contract.side == OrderSide::Buy {
                    contract.outstanding()?
                } else {
                    contract.bond
                },
                1,
            )?;
            plan.accounts.push((contract.proposer, account));
            0
        };
        plan.agreement = Some((contract.id, None));
        plan.events.push(Event::Contract {
            event: ContractEvent::Closed {
                contract: contract.clone(),
                outcome,
                compensation,
                representatives: [
                    self.representative(contract.buyer())?,
                    self.representative(contract.seller())?,
                ],
                participants: self.participants(contract)?,
            },
        });
        Ok(plan)
    }
    /// Expire unanswered offers before intents; active contracts keep their funded obligations.
    pub fn expire(&mut self, world: &mut World, tick: Tick) -> Result<Vec<Event>> {
        let expired: Vec<_> = self
            .contracts
            .values()
            .filter(|c| c.accepted_at.is_none() && c.expires_at < tick)
            .cloned()
            .collect();
        let mut events = Vec::new();
        for contract in expired {
            let plan = self.close_plan(world, &contract, ContractOutcome::Expired)?;
            events.extend(self.commit(world, plan)?);
        }
        Ok(events)
    }
    /// Fulfill due shipments after production; missed supply pays the funded bond and closes.
    pub fn settle_due(&mut self, world: &mut World, tick: Tick) -> Result<Vec<Event>> {
        let due: Vec<_> = self
            .contracts
            .values()
            .filter(|c| c.next_delivery.is_some_and(|at| at <= tick))
            .cloned()
            .collect();
        let mut events = Vec::new();
        for contract in due {
            if self
                .account(world, contract.seller())?
                .available(contract.resource)
                < contract.quantity
            {
                let plan = self.close_plan(world, &contract, ContractOutcome::Defaulted)?;
                events.extend(self.commit(world, plan)?);
                continue;
            }
            let mut buyer = self.account(world, contract.buyer())?.clone();
            let mut seller = self.account(world, contract.seller())?.clone();
            seller.reserve_sell(contract.resource, contract.quantity)?;
            settle_trade(
                &mut buyer,
                &mut seller,
                contract.resource,
                contract.quantity,
                contract.unit_price,
                contract.unit_price,
            )?;
            let mut next = contract.clone();
            next.remaining -= 1;
            let mut plan = self.blank_plan();
            let representatives = [
                self.representative(contract.buyer())?,
                self.representative(contract.seller())?,
            ];
            let participants = self.participants(&contract)?;
            plan.events.push(Event::Contract {
                event: ContractEvent::Delivered {
                    contract_id: contract.id,
                    buyer: contract.buyer(),
                    seller: contract.seller(),
                    representatives,
                    participants: participants.clone(),
                    resource: contract.resource,
                    quantity: contract.quantity,
                    price: contract.unit_price,
                },
            });
            if next.remaining == 0 {
                seller.release_buy(contract.bond, 1)?;
                next.next_delivery = None;
                plan.events.push(Event::Contract {
                    event: ContractEvent::Closed {
                        contract: next,
                        outcome: ContractOutcome::Completed,
                        compensation: 0,
                        representatives,
                        participants,
                    },
                });
                plan.agreement = Some((contract.id, None));
            } else {
                next.next_delivery = Some(
                    tick.checked_add(next.interval_ticks)
                        .ok_or(DomainError::Overflow("delivery deadline"))?,
                );
                plan.agreement = Some((contract.id, Some(next)));
            }
            plan.accounts.push((contract.buyer(), buyer));
            plan.accounts.push((contract.seller(), seller));
            if let Party::Agent(id) = contract.buyer() {
                plan.purchased = Some((id, contract.resource, contract.unit_price));
            }
            events.extend(self.commit(world, plan)?);
        }
        Ok(events)
    }
    /// Expected persistent cash reservations, including pending and active commitments.
    pub fn collateral(&self) -> Result<BTreeMap<Party, Money>> {
        let mut values: BTreeMap<Party, Money> = BTreeMap::new();
        for contract in self.contracts.values() {
            for (party, amount) in [
                (contract.buyer(), contract.outstanding()?),
                (contract.seller(), contract.bond),
            ] {
                if contract.accepted_at.is_some() || party == contract.proposer {
                    let value = values.entry(party).or_default();
                    *value = value
                        .checked_add(amount)
                        .ok_or(DomainError::Overflow("contract collateral"))?;
                }
            }
        }
        Ok(values)
    }
}
