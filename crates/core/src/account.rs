use crate::{DomainError, Inventory, Money, Prices, Resource, Result};
use serde::{Deserialize, Serialize};

/// Cash and stock including encumbered assets. Reservations never create assets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    wallet: Money,
    reserved_cash: Money,
    inventory: Inventory,
    reserved_inventory: Inventory,
    earned: u128,
    spent: u128,
    transfers_in: u128,
    transfers_out: u128,
}

impl Account {
    /// Create an account with no open reservations or historical income.
    pub const fn new(wallet: Money, inventory: Inventory) -> Self {
        Self {
            wallet,
            inventory,
            reserved_cash: 0,
            reserved_inventory: Inventory::new([0; 4]),
            earned: 0,
            spent: 0,
            transfers_in: 0,
            transfers_out: 0,
        }
    }

    /// Total money, including money reserved by open bids.
    pub const fn wallet(&self) -> Money {
        self.wallet
    }
    /// Money committed to open bids.
    pub const fn reserved_cash(&self) -> Money {
        self.reserved_cash
    }
    /// Money available for new bids.
    pub const fn available_cash(&self) -> Money {
        self.wallet - self.reserved_cash
    }
    /// All physical stock, including stock reserved by open asks.
    pub const fn inventory(&self) -> &Inventory {
        &self.inventory
    }
    /// Stock committed to open asks.
    pub const fn reserved_inventory(&self) -> &Inventory {
        &self.reserved_inventory
    }
    /// Stock available for production, consumption, or new asks.
    pub const fn available(&self, resource: Resource) -> u64 {
        self.inventory.get(resource) - self.reserved_inventory.get(resource)
    }
    /// Lifetime sales receipts; this is revenue, not profit.
    pub const fn earned(&self) -> u128 {
        self.earned
    }
    /// Lifetime expenditure on market purchases.
    pub const fn spent(&self) -> u128 {
        self.spent
    }
    /// Lifetime non-sale receipts: contributions, dividends, taxes, and compensation.
    pub const fn transfers_in(&self) -> u128 {
        self.transfers_in
    }
    /// Lifetime non-purchase payments, separate from trade expenditure.
    pub const fn transfers_out(&self) -> u128 {
        self.transfers_out
    }
    /// Check the complete cash ledger against the owner's initial endowment.
    pub fn validate_ledger(&self, initial: Money) -> Result<()> {
        let credits = u128::from(initial)
            .checked_add(self.earned)
            .and_then(|v| v.checked_add(self.transfers_in));
        let debits = u128::from(self.wallet)
            .checked_add(self.spent)
            .and_then(|v| v.checked_add(self.transfers_out));
        if credits.is_none() || credits != debits {
            return Err(DomainError::Invariant(
                "account transfer ledger disagrees with wallet".into(),
            ));
        }
        self.validate()
    }
    /// Retain the accounting history of a dissolved, completely empty owner.
    pub fn absorb_closed_history(&mut self, closed: &Self) -> Result<()> {
        if closed.wallet != 0
            || closed.inventory != Inventory::default()
            || closed.reserved_cash != 0
            || closed.reserved_inventory != Inventory::default()
        {
            return Err(DomainError::Invariant("retiring a nonempty account".into()));
        }
        let mut next = self.clone();
        for (value, amount) in [
            (&mut next.earned, closed.earned),
            (&mut next.spent, closed.spent),
            (&mut next.transfers_in, closed.transfers_in),
            (&mut next.transfers_out, closed.transfers_out),
        ] {
            *value = value
                .checked_add(amount)
                .ok_or(DomainError::Overflow("retired account history"))?;
        }
        next.validate_ledger(0)?;
        *self = next;
        Ok(())
    }

    /// Mark-to-market wealth; reserved assets are counted exactly once.
    pub fn wealth(&self, prices: &Prices) -> u128 {
        u128::from(self.wallet)
            + Resource::ALL
                .into_iter()
                .map(|r| u128::from(self.inventory.get(r)) * u128::from(prices[r.index()]))
                .sum::<u128>()
    }

    /// Validate reservations after deserialization or at an audit boundary.
    pub fn validate(&self) -> Result<()> {
        if self.reserved_cash > self.wallet {
            return Err(DomainError::Invariant(
                "reserved cash exceeds wallet".into(),
            ));
        }
        for r in Resource::ALL {
            if self.reserved_inventory.get(r) > self.inventory.get(r) {
                return Err(DomainError::Invariant(format!(
                    "reserved {r} exceeds stock"
                )));
            }
        }
        Ok(())
    }

    /// Reserve the maximum payment of a bid, atomically.
    pub fn reserve_buy(&mut self, price: Money, quantity: u64) -> Result<()> {
        let cost = notional(price, quantity)?;
        if self.available_cash() < cost {
            return Err(DomainError::InsufficientFunds);
        }
        self.reserved_cash = self
            .reserved_cash
            .checked_add(cost)
            .ok_or(DomainError::Overflow("cash reservation"))?;
        Ok(())
    }

    /// Release a bid's unused maximum payment.
    pub fn release_buy(&mut self, price: Money, quantity: u64) -> Result<()> {
        let cost = notional(price, quantity)?;
        self.reserved_cash = self
            .reserved_cash
            .checked_sub(cost)
            .ok_or_else(|| DomainError::Invariant("releasing unreserved cash".into()))?;
        Ok(())
    }

    /// Reserve physical stock, atomically.
    pub fn reserve_sell(&mut self, resource: Resource, quantity: u64) -> Result<()> {
        if quantity == 0 {
            return Err(DomainError::InvalidAction("zero quantity".into()));
        }
        if self.available(resource) < quantity {
            return Err(DomainError::InsufficientInventory(resource));
        }
        self.reserved_inventory.add(resource, quantity)
    }

    /// Release an ask's unfilled stock.
    pub fn release_sell(&mut self, resource: Resource, quantity: u64) -> Result<()> {
        self.reserved_inventory.remove(resource, quantity)
    }

    /// Deposit newly produced stock. This does not alter monetary income.
    pub fn produce(&mut self, resource: Resource, quantity: u64) -> Result<()> {
        self.inventory.add(resource, quantity)
    }

    /// Consume unreserved stock only.
    pub fn consume(&mut self, resource: Resource, quantity: u64) -> Result<()> {
        if self.available(resource) < quantity {
            return Err(DomainError::InsufficientInventory(resource));
        }
        self.inventory.remove(resource, quantity)
    }

    /// Apply a production recipe atomically, even if a later input or output fails.
    pub fn transform(
        &mut self,
        inputs: &[(Resource, u64)],
        output: Resource,
        quantity: u64,
    ) -> Result<()> {
        let mut next = self.clone();
        for &(resource, amount) in inputs {
            next.consume(resource, amount)?;
        }
        next.produce(output, quantity)?;
        *self = next;
        Ok(())
    }
}

/// Atomically transfer unreserved assets without inventing production or trade receipts.
pub fn transfer_assets(
    from: &mut Account,
    to: &mut Account,
    cash: Money,
    stock: &Inventory,
) -> Result<()> {
    let mut debit = from.clone();
    let mut credit = to.clone();
    if debit.available_cash() < cash {
        return Err(DomainError::InsufficientFunds);
    }
    debit.wallet -= cash;
    credit.wallet = credit
        .wallet
        .checked_add(cash)
        .ok_or(DomainError::Overflow("transfer recipient cash"))?;
    debit.transfers_out = debit
        .transfers_out
        .checked_add(u128::from(cash))
        .ok_or(DomainError::Overflow("outgoing transfers"))?;
    credit.transfers_in = credit
        .transfers_in
        .checked_add(u128::from(cash))
        .ok_or(DomainError::Overflow("incoming transfers"))?;
    for resource in Resource::ALL {
        let quantity = stock.get(resource);
        if quantity > 0 {
            debit.consume(resource, quantity)?;
            credit.produce(resource, quantity)?;
        }
    }
    debit.validate()?;
    credit.validate()?;
    *from = debit;
    *to = credit;
    Ok(())
}

/// Calculate a positive order value without overflow.
pub fn notional(price: Money, quantity: u64) -> Result<Money> {
    if price == 0 || quantity == 0 {
        return Err(DomainError::InvalidAction(
            "price and quantity must be positive".into(),
        ));
    }
    price
        .checked_mul(quantity)
        .ok_or(DomainError::Overflow("order notional"))
}

/// Settle a reserved trade as one atomic transition across both accounts.
///
/// The buyer's full limit-price reservation for the filled quantity is released;
/// only the execution price is debited. Callers must prohibit self-trading.
pub fn settle_trade(
    buyer: &mut Account,
    seller: &mut Account,
    resource: Resource,
    quantity: u64,
    bid_limit: Money,
    price: Money,
) -> Result<()> {
    if price > bid_limit {
        return Err(DomainError::InvalidAction(
            "execution above bid limit".into(),
        ));
    }
    let cost = notional(price, quantity)?;
    let mut next_buyer = buyer.clone();
    let mut next_seller = seller.clone();
    next_buyer.release_buy(bid_limit, quantity)?;
    next_seller.release_sell(resource, quantity)?;
    next_buyer.wallet = next_buyer
        .wallet
        .checked_sub(cost)
        .ok_or(DomainError::InsufficientFunds)?;
    next_seller.wallet = next_seller
        .wallet
        .checked_add(cost)
        .ok_or(DomainError::Overflow("seller wallet"))?;
    next_seller.consume(resource, quantity)?;
    next_buyer.produce(resource, quantity)?;
    next_seller.earned = next_seller
        .earned
        .checked_add(u128::from(cost))
        .ok_or(DomainError::Overflow("revenue"))?;
    next_buyer.spent = next_buyer
        .spent
        .checked_add(u128::from(cost))
        .ok_or(DomainError::Overflow("spending"))?;
    next_buyer.validate()?;
    next_seller.validate()?;
    *buyer = next_buyer;
    *seller = next_seller;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservations_prevent_double_spending_and_double_selling() {
        let mut account = Account::new(100, Inventory::new([5, 0, 0, 0]));
        account.reserve_buy(10, 6).unwrap();
        assert_eq!(
            account.reserve_buy(10, 5),
            Err(DomainError::InsufficientFunds)
        );
        account.reserve_sell(Resource::Food, 4).unwrap();
        assert!(account.reserve_sell(Resource::Food, 2).is_err());
        assert!(account.consume(Resource::Food, 2).is_err());
        assert_eq!(account.available_cash(), 40);
        assert_eq!(account.available(Resource::Food), 1);
    }

    #[test]
    fn atomic_settlement_conserves_money_and_stock_and_refunds_price_improvement() {
        let mut buyer = Account::new(1000, Inventory::default());
        let mut seller = Account::new(100, Inventory::new([10, 0, 0, 0]));
        buyer.reserve_buy(100, 5).unwrap();
        seller.reserve_sell(Resource::Food, 5).unwrap();
        settle_trade(&mut buyer, &mut seller, Resource::Food, 3, 100, 80).unwrap();
        assert_eq!(buyer.wallet() + seller.wallet(), 1100);
        assert_eq!(
            buyer.inventory().get(Resource::Food) + seller.inventory().get(Resource::Food),
            10
        );
        assert_eq!(buyer.reserved_cash(), 200);
        assert_eq!(buyer.available_cash(), 560);
        assert_eq!(seller.reserved_inventory().get(Resource::Food), 2);
    }

    #[test]
    fn overflow_does_not_half_commit_a_trade() {
        let mut buyer = Account::new(100, Inventory::default());
        let mut seller = Account::new(u64::MAX, Inventory::new([1, 0, 0, 0]));
        buyer.reserve_buy(10, 1).unwrap();
        seller.reserve_sell(Resource::Food, 1).unwrap();
        let before = (buyer.clone(), seller.clone());
        assert!(settle_trade(&mut buyer, &mut seller, Resource::Food, 1, 10, 10).is_err());
        assert_eq!((buyer, seller), before);
        assert!(notional(u64::MAX, 2).is_err());
    }

    #[test]
    fn recipe_failure_preserves_every_input() {
        let mut account = Account::new(0, Inventory::new([0, 4, 0, 0]));
        let before = account.clone();
        assert!(
            account
                .transform(
                    &[(Resource::Wood, 2), (Resource::Metal, 1)],
                    Resource::Goods,
                    2
                )
                .is_err()
        );
        assert_eq!(account, before);
    }
}
