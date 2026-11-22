use super::*;
impl InstitutionBook {
    /// Validate all identities, memberships, funds and delivery clocks before publishing a book.
    /// Personal reservations are additionally reconciled with the bilateral proposal book.
    pub fn from_snapshot(
        world: &World,
        settings: &OrganizationSettings,
        snapshot: InstitutionSnapshot,
    ) -> Result<Self> {
        let malformed = || {
            DomainError::Invariant("invalid shared ownership or delivery contract snapshot".into())
        };
        let opportunities = u128::from(world.tick()) * world.len() as u128;
        if u128::from(snapshot.next_organization) > opportunities
            || u128::from(snapshot.next_contract) > opportunities
            || snapshot.organizations.len() > world.len()
            || snapshot.contracts.len() > world.len().saturating_mul(MAX_CONTRACTS)
            || !snapshot.organizations.windows(2).all(|p| p[0].id < p[1].id)
            || !snapshot.contracts.windows(2).all(|p| p[0].id < p[1].id)
            || (!settings.enabled
                && (snapshot.next_organization != 0
                    || snapshot.next_contract != 0
                    || snapshot.retired != Account::new(0, Inventory::default())))
        {
            return Err(malformed());
        }
        snapshot.retired.validate_ledger(0)?;
        if snapshot.retired.wallet() != 0
            || snapshot.retired.inventory() != &Inventory::default()
            || snapshot.retired.reserved_cash() != 0
            || snapshot.retired.reserved_inventory() != &Inventory::default()
        {
            return Err(malformed());
        }
        let mut book = Self {
            next_organization: snapshot.next_organization,
            next_contract: snapshot.next_contract,
            retired: snapshot.retired,
            ..Default::default()
        };
        for group in snapshot.organizations {
            if group.id.0 >= book.next_organization
                || group.members.is_empty()
                || group.members.len() > settings.max_members
                || !group.members.contains(&group.leader)
                || group.founded_at == 0
                || group.founded_at > world.tick()
                || group.last_production > world.tick()
                || (group.last_production != 0 && group.last_production < group.founded_at)
                || (group.last_production != 0
                    && !(group.last_production.wrapping_add(group.id.0))
                        .is_multiple_of(settings.production_interval))
            {
                return Err(malformed());
            }
            group.account.validate_ledger(0)?;
            for &id in &group.members {
                world.agent(id).map_err(|_| malformed())?;
                if book.membership(id).is_some() {
                    return Err(malformed());
                }
            }
            book.put_group(group.id, Some(group));
        }
        for contract in snapshot.contracts {
            crate::validate_order_values(contract.quantity, contract.unit_price)
                .map_err(|_| malformed())?;
            if contract.id >= book.next_contract
                || contract.created_at == 0
                || contract.created_at > world.tick()
                || contract.created_at.checked_add(4) != Some(contract.expires_at)
                || !(2..=32).contains(&contract.deliveries)
                || contract.remaining == 0
                || contract.remaining > contract.deliveries
                || !(1..=64).contains(&contract.interval_ticks)
            {
                return Err(malformed());
            }
            let total = notional(contract.unit_price, contract.quantity)?
                .checked_mul(u64::from(contract.deliveries))
                .ok_or_else(malformed)?;
            if contract.bond != total.div_ceil(10).max(1) {
                return Err(malformed());
            }
            if let Some(accepted) = contract.accepted_at {
                let offset = contract.interval_ticks
                    * u64::from(contract.deliveries - contract.remaining + 1);
                if accepted <= contract.created_at
                    || accepted > contract.expires_at
                    || accepted > world.tick()
                    || accepted.checked_add(offset) != contract.next_delivery
                    || contract.next_delivery.is_none_or(|at| at <= world.tick())
                {
                    return Err(malformed());
                }
            } else if contract.next_delivery.is_some()
                || contract.expires_at < world.tick()
                || contract.remaining != contract.deliveries
            {
                return Err(malformed());
            }
            book.check_pair(world, contract.proposer, contract.recipient)
                .map_err(|_| malformed())?;
            book.put_contract(contract.id, Some(contract));
        }
        book.audit_groups()?;
        Ok(book)
    }
    /// Match each shared account to its exact contract cash obligations; no group stock is pledged.
    pub fn audit_groups(&self) -> Result<()> {
        let expected = self.collateral()?;
        for group in self.organizations.values() {
            group.account.validate_ledger(0)?;
            if group.account.reserved_cash()
                != expected
                    .get(&Party::Organization(group.id))
                    .copied()
                    .unwrap_or(0)
                || group.account.reserved_inventory() != &Inventory::default()
            {
                return Err(DomainError::Invariant(
                    "unattributed group collateral".into(),
                ));
            }
        }
        Ok(())
    }
    /// Institutional cash collateral held in individual accounts.
    pub fn personal_collateral(&self) -> Result<BTreeMap<AgentId, Money>> {
        Ok(self
            .collateral()?
            .into_iter()
            .filter_map(|(party, cash)| match party {
                Party::Agent(id) => Some((id, cash)),
                Party::Organization(_) => None,
            })
            .collect())
    }
    /// Per-agent beneficial ownership, valued once and divided by equal shares with stable remainders.
    pub fn equity(&self, prices: &Prices) -> BTreeMap<AgentId, u128> {
        let mut shares = BTreeMap::new();
        for group in self.organizations.values() {
            let wealth = group.account.wealth(prices);
            let count = group.members.len() as u128;
            for (index, &id) in group.members.iter().enumerate() {
                shares.insert(
                    id,
                    wealth / count + u128::from((index as u128) < wealth % count),
                );
            }
        }
        shares
    }
}
