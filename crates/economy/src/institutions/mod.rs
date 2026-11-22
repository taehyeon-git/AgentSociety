//! Cooperative ownership and funded recurring agreements share the same checked accounts.
use agentsociety_core::*;
use agentsociety_world::World;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
mod contracts;
mod organizations;
mod restore;

/// Portable shared property and active agreements; indexes are reconstructed on restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstitutionSnapshot {
    /// Next cooperative ID, including dissolved organizations.
    pub next_organization: u64,
    /// Next contract ID, including closed agreements.
    pub next_contract: u64,
    /// Existing groups in stable ID order.
    pub organizations: Vec<Organization>,
    /// Pending and active agreements in stable ID order.
    pub contracts: Vec<DeliveryContract>,
    /// Empty account retaining the cash history of dissolved groups.
    pub retired: Account,
}

/// Single-writer institutional state, with bounded memberships and indexed agreements.
pub struct InstitutionBook {
    next_organization: u64,
    next_contract: u64,
    organizations: BTreeMap<OrganizationId, Organization>,
    memberships: BTreeMap<AgentId, OrganizationId>,
    contracts: BTreeMap<u64, DeliveryContract>,
    commitments: BTreeMap<Party, BTreeSet<u64>>,
    retired: Account,
}
impl Default for InstitutionBook {
    fn default() -> Self {
        Self {
            next_organization: 0,
            next_contract: 0,
            organizations: BTreeMap::new(),
            memberships: BTreeMap::new(),
            contracts: BTreeMap::new(),
            commitments: BTreeMap::new(),
            retired: Account::new(0, Inventory::default()),
        }
    }
}

struct Plan {
    accounts: Vec<(Party, Account)>,
    group: Option<(OrganizationId, Option<Organization>)>,
    agreement: Option<(u64, Option<DeliveryContract>)>,
    retired: Option<Account>,
    next_organization: u64,
    next_contract: u64,
    events: Vec<Event>,
    purchased: Option<(AgentId, Resource, Money)>,
}
fn invalid(message: &str) -> DomainError {
    DomainError::InvalidAction(message.into())
}
impl InstitutionBook {
    /// Canonical groups, without retired history.
    pub fn organizations(&self) -> impl ExactSizeIterator<Item = &Organization> {
        self.organizations.values()
    }
    /// Canonical pending and active agreements.
    pub fn contracts(&self) -> impl ExactSizeIterator<Item = &DeliveryContract> {
        self.contracts.values()
    }
    /// Membership lookup in logarithmic time.
    pub fn membership(&self, agent: AgentId) -> Option<OrganizationId> {
        self.memberships.get(&agent).copied()
    }
    /// Look up shared property by its own identity namespace.
    pub fn organization(&self, id: OrganizationId) -> Result<&Organization> {
        self.organizations
            .get(&id)
            .ok_or_else(|| invalid("unknown organization"))
    }
    /// Existing agreements of a legal owner, at most four.
    pub fn contracts_for(&self, party: Party) -> impl Iterator<Item = &DeliveryContract> {
        self.commitments
            .get(&party)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .map(|id| &self.contracts[id])
    }
    /// Closed ownership ledgers remain available for aggregate audit.
    pub const fn retired(&self) -> &Account {
        &self.retired
    }
    /// Stable identifiers for validating retained historical references.
    pub const fn next_ids(&self) -> (u64, u64) {
        (self.next_organization, self.next_contract)
    }
    /// Full authoritative state, without redundant indexes.
    pub fn snapshot(&self) -> InstitutionSnapshot {
        InstitutionSnapshot {
            next_organization: self.next_organization,
            next_contract: self.next_contract,
            organizations: self.organizations.values().cloned().collect(),
            contracts: self.contracts.values().cloned().collect(),
            retired: self.retired.clone(),
        }
    }
    /// Account lookup by legal owner, independent of whether it is a person or a group.
    pub fn account<'a>(&'a self, world: &'a World, party: Party) -> Result<&'a Account> {
        match party {
            Party::Agent(id) => Ok(&world
                .agent(id)
                .map_err(|_| invalid("unknown contract participant"))?
                .account),
            Party::Organization(id) => Ok(&self.organization(id)?.account),
        }
    }
    /// Replace a previously validated owner's complete account.
    pub fn replace_account(
        &mut self,
        world: &mut World,
        party: Party,
        account: Account,
    ) -> Result<()> {
        account.validate()?;
        match party {
            Party::Agent(id) => world.with_agent_mut(id, |a| {
                a.account = account;
                Ok(())
            }),
            Party::Organization(id) => {
                self.organizations
                    .get_mut(&id)
                    .ok_or_else(|| invalid("unknown organization"))?
                    .account = account;
                Ok(())
            }
        }
    }
    /// Current representative who may bind an owner to a contract.
    pub fn representative(&self, party: Party) -> Result<AgentId> {
        match party {
            Party::Agent(id) => Ok(id),
            Party::Organization(id) => Ok(self.organization(id)?.leader),
        }
    }
    /// All people owning the affected assets, in canonical order.
    pub fn owners(&self, party: Party) -> Result<Vec<AgentId>> {
        match party {
            Party::Agent(id) => Ok(vec![id]),
            Party::Organization(id) => Ok(self.organization(id)?.members.iter().copied().collect()),
        }
    }
    fn participants(&self, contract: &DeliveryContract) -> Result<Vec<AgentId>> {
        let mut ids = self.owners(contract.proposer)?;
        ids.extend(self.owners(contract.recipient)?);
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }
    fn authorize(&self, world: &World, actor: AgentId, party: Party) -> Result<()> {
        self.account(world, party)?;
        if self.representative(party)? != actor {
            return Err(invalid(
                "only the owner or group leader may authorize this agreement",
            ));
        }
        Ok(())
    }
    fn blank_plan(&self) -> Plan {
        Plan {
            accounts: Vec::new(),
            group: None,
            agreement: None,
            retired: None,
            next_organization: self.next_organization,
            next_contract: self.next_contract,
            events: Vec::new(),
            purchased: None,
        }
    }
    fn put_group(&mut self, id: OrganizationId, group: Option<Organization>) {
        if let Some(old) = self.organizations.remove(&id) {
            for member in old.members {
                self.memberships.remove(&member);
            }
        }
        if let Some(group) = group {
            for &member in &group.members {
                self.memberships.insert(member, id);
            }
            self.organizations.insert(id, group);
        }
    }
    fn put_contract(&mut self, id: u64, contract: Option<DeliveryContract>) {
        if let Some(old) = self.contracts.remove(&id) {
            for party in [old.proposer, old.recipient] {
                if let Some(ids) = self.commitments.get_mut(&party) {
                    ids.remove(&id);
                    if ids.is_empty() {
                        self.commitments.remove(&party);
                    }
                }
            }
        }
        if let Some(contract) = contract {
            for party in [contract.proposer, contract.recipient] {
                self.commitments.entry(party).or_default().insert(id);
            }
            self.contracts.insert(id, contract);
        }
    }
    /// Validate an operation without changing any account, membership, clock, or identifier.
    pub fn validate_action(
        &self,
        world: &World,
        settings: &OrganizationSettings,
        actor: AgentId,
        operation: &InstitutionAction,
        tick: Tick,
    ) -> Result<()> {
        self.prepare(world, settings, actor, operation, tick)
            .map(|_| ())
    }
    /// Commit a fully checked action; errors leave all involved owners and indexes untouched.
    pub fn apply(
        &mut self,
        world: &mut World,
        settings: &OrganizationSettings,
        actor: AgentId,
        operation: &InstitutionAction,
        tick: Tick,
    ) -> Result<Vec<Event>> {
        let plan = self.prepare(world, settings, actor, operation, tick)?;
        self.commit(world, plan)
    }
    fn commit(&mut self, world: &mut World, plan: Plan) -> Result<Vec<Event>> {
        for (owner, account) in plan.accounts {
            self.replace_account(world, owner, account)?;
        }
        if let Some((buyer, resource, price)) = plan.purchased {
            world.with_agent_mut(buyer, |a| {
                a.last_purchase_prices[resource.index()] = price;
                Ok(())
            })?;
        }
        if let Some((id, group)) = plan.group {
            self.put_group(id, group);
        }
        if let Some((id, contract)) = plan.agreement {
            self.put_contract(id, contract);
        }
        if let Some(retired) = plan.retired {
            self.retired = retired;
        }
        self.next_organization = plan.next_organization;
        self.next_contract = plan.next_contract;
        Ok(plan.events)
    }
    fn prepare(
        &self,
        world: &World,
        settings: &OrganizationSettings,
        actor: AgentId,
        operation: &InstitutionAction,
        tick: Tick,
    ) -> Result<Plan> {
        if !settings.enabled {
            return Err(invalid(
                "organizations and recurring contracts are disabled",
            ));
        }
        world.agent(actor)?;
        match operation {
            InstitutionAction::Found { .. }
            | InstitutionAction::Join { .. }
            | InstitutionAction::Leave { .. }
            | InstitutionAction::Contribute { .. }
            | InstitutionAction::Distribute { .. } => {
                self.prepare_group(world, settings, actor, operation, tick)
            }
            _ => self.prepare_contract(world, actor, operation, tick),
        }
    }
    fn group_view(&self, group: &Organization) -> OrganizationView {
        OrganizationView {
            id: group.id,
            leader: group.leader,
            members: group.members.iter().copied().collect(),
            available_cash: group.account.available_cash(),
            available_inventory: Inventory::new(Resource::ALL.map(|r| group.account.available(r))),
            contracts: self.contracts_for(Party::Organization(group.id)).count(),
        }
    }
    /// Bounded public information shared by local and model policies.
    pub fn observe(
        &self,
        world: &World,
        settings: &OrganizationSettings,
        actor: AgentId,
        tick: Tick,
    ) -> Result<Option<InstitutionObservation>> {
        if !settings.enabled {
            return Ok(None);
        }
        let own = self
            .membership(actor)
            .map(|id| self.group_view(&self.organizations[&id]));
        let start = OrganizationId(actor.0.wrapping_add(tick / 8) % self.next_organization.max(1));
        let organizations = self
            .organizations
            .range(start..)
            .chain(self.organizations.range(..start))
            .take(8)
            .map(|(_, g)| self.group_view(g))
            .collect();
        let counterparties = world
            .ids_from(AgentId(
                actor.0.wrapping_add(1).wrapping_add(tick / 8) % (world.len() as u64).max(1),
            ))
            .filter(|&id| id != actor)
            .take(8)
            .map(|id| {
                let a = world.agent(id)?;
                Ok(ContractCounterpartyView {
                    id,
                    occupation: a.occupation,
                    available_inventory: Inventory::new(
                        Resource::ALL.map(|r| a.account.available(r)),
                    ),
                    available_cash: a.account.available_cash(),
                    energy: a.needs.energy,
                    food_reserve: a.food_reserve(),
                    open_contracts: self.contracts_for(Party::Agent(id)).count(),
                })
            })
            .collect::<Result<_>>()?;
        let mut contracts: Vec<_> = self
            .contracts_for(Party::Agent(actor))
            .filter(|c| c.accepted_at.is_some() || (c.created_at < tick && c.expires_at >= tick))
            .cloned()
            .collect();
        if let Some(group) = &own
            && group.leader == actor
        {
            contracts.extend(
                self.contracts_for(Party::Organization(group.id))
                    .filter(|c| {
                        c.accepted_at.is_some() || (c.created_at < tick && c.expires_at >= tick)
                    })
                    .cloned(),
            );
        }
        contracts.sort_by_key(|c| c.id);
        Ok(Some(InstitutionObservation {
            settings: settings.clone(),
            own,
            organizations,
            counterparties,
            contracts,
        }))
    }
}
