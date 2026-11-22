use agentsociety_core::*;
use agentsociety_world::World;
use serde::{Deserialize, Serialize};
use std::{cmp::Reverse, collections::BTreeMap};

/// A fully collateralized, tick-local limit order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    /// Globally monotonic order sequence.
    pub id: u64,
    /// Account owner.
    pub agent_id: AgentId,
    /// Bid or ask.
    pub side: OrderSide,
    /// Traded resource.
    pub resource: Resource,
    /// Unfilled quantity.
    pub remaining: u64,
    /// Unit price bound.
    pub limit_price: Money,
    /// Tick at submission; sequence breaks ties within a tick.
    pub submitted_at: Tick,
}

/// Tick-boundary market state; orders are always expired before a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketSnapshot {
    /// Last nonempty tick's volume-weighted execution price per resource.
    pub prices: Prices,
    /// Next globally unique order sequence.
    pub next_order_id: u64,
}

/// Central order book. No concurrent mutation and no implicit credit or shorting.
pub struct OrderBook {
    orders: BTreeMap<u64, Order>,
    prices: Prices,
    next_order_id: u64,
}

impl OrderBook {
    /// Restore the empty book at a fully settled tick boundary.
    pub fn from_snapshot(snapshot: MarketSnapshot) -> Result<Self> {
        let mut market = Self::new(snapshot.prices)?;
        market.next_order_id = snapshot.next_order_id;
        Ok(market)
    }

    /// Start an empty market with positive reference prices.
    pub fn new(prices: Prices) -> Result<Self> {
        if prices.iter().any(|&p| p == 0 || p > MAX_PRICE) {
            return Err(DomainError::InvalidConfig(
                "invalid market reference price".into(),
            ));
        }
        Ok(Self {
            orders: BTreeMap::new(),
            prices,
            next_order_id: 0,
        })
    }

    /// Current reference prices, unchanged for resources without a trade.
    pub const fn prices(&self) -> &Prices {
        &self.prices
    }
    /// Orders in monotonic submission order, including fully filled entries until expiry.
    pub fn orders(&self) -> impl Iterator<Item = &Order> {
        self.orders.values()
    }
    /// Whether the book has no entries (required at tick boundaries).
    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }
    /// Snapshot after all orders have expired.
    pub fn snapshot(&self) -> Result<MarketSnapshot> {
        if !self.is_empty() {
            return Err(DomainError::Invariant(
                "snapshot requires settled market".into(),
            ));
        }
        Ok(MarketSnapshot {
            prices: self.prices,
            next_order_id: self.next_order_id,
        })
    }

    /// Validate and reserve an order. Failure leaves both book and account unchanged.
    #[allow(clippy::too_many_arguments)]
    pub fn place(
        &mut self,
        world: &mut World,
        agent_id: AgentId,
        side: OrderSide,
        resource: Resource,
        quantity: u64,
        limit_price: Money,
        tick: Tick,
    ) -> Result<Event> {
        validate_order_values(quantity, limit_price)?;
        let next_id = self
            .next_order_id
            .checked_add(1)
            .ok_or(DomainError::Overflow("order ID"))?;
        world.with_agent_mut(agent_id, |agent| match side {
            OrderSide::Buy => agent.account.reserve_buy(limit_price, quantity),
            OrderSide::Sell => agent.account.reserve_sell(resource, quantity),
        })?;
        let id = self.next_order_id;
        self.next_order_id = next_id;
        self.orders.insert(
            id,
            Order {
                id,
                agent_id,
                side,
                resource,
                remaining: quantity,
                limit_price,
                submitted_at: tick,
            },
        );
        Ok(Event::OrderPlaced {
            order_id: id,
            agent_id,
            side,
            resource,
            quantity,
            limit_price,
        })
    }

    /// Match bids in price/time order; equally priced asks prefer buyer trust.
    ///
    /// Self-orders are skipped without blocking other counterparties. Each trade
    /// executes at the older order's price, and each settlement is atomic.
    pub fn match_orders(&mut self, world: &mut World) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        for resource in Resource::ALL {
            let mut bids: Vec<_> = self
                .orders
                .values()
                .filter(|o| o.resource == resource && o.side == OrderSide::Buy && o.remaining > 0)
                .map(|o| (Reverse(o.limit_price), o.submitted_at, o.id))
                .collect();
            let mut asks: Vec<_> = self
                .orders
                .values()
                .filter(|o| o.resource == resource && o.side == OrderSide::Sell && o.remaining > 0)
                .map(|o| (o.limit_price, o.submitted_at, o.id))
                .collect();
            bids.sort_unstable();
            asks.sort_unstable();
            let mut ask_start = 0;
            let mut volume = 0_u128;
            let mut turnover = 0_u128;
            for (_, _, bid_id) in bids {
                loop {
                    let bid = self.orders[&bid_id].clone();
                    if bid.remaining == 0 {
                        break;
                    }
                    while ask_start < asks.len() && self.orders[&asks[ask_start].2].remaining == 0 {
                        ask_start += 1;
                    }
                    let buyer = world.agent(bid.agent_id)?;
                    let mut candidate: Option<&Order> = None;
                    let mut best_key = None;
                    for &(price, submitted_at, id) in &asks[ask_start..] {
                        // Sorted prices let us stop after the cheapest eligible
                        // group. Filled/self orders alone must not end the search.
                        if price > bid.limit_price
                            || candidate.is_some_and(|ask| price > ask.limit_price)
                        {
                            break;
                        }
                        let ask = &self.orders[&id];
                        if ask.remaining == 0 || ask.agent_id == bid.agent_id {
                            continue;
                        }
                        let trust = buyer
                            .relationships
                            .get(&ask.agent_id)
                            .map_or(0, |edge| edge.trust);
                        let key = (Reverse(trust), submitted_at, id);
                        if best_key.is_none_or(|best| key < best) {
                            candidate = Some(ask);
                            best_key = Some(key);
                        }
                    }
                    let Some(ask) = candidate.cloned() else {
                        break;
                    };
                    let quantity = bid.remaining.min(ask.remaining);
                    let price = if (bid.submitted_at, bid.id) < (ask.submitted_at, ask.id) {
                        bid.limit_price
                    } else {
                        ask.limit_price
                    };
                    world.with_pair_mut(bid.agent_id, ask.agent_id, |buyer, seller| {
                        settle_trade(
                            &mut buyer.account,
                            &mut seller.account,
                            resource,
                            quantity,
                            bid.limit_price,
                            price,
                        )?;
                        buyer.last_purchase_prices[resource.index()] = price;
                        Ok(())
                    })?;
                    // Both entries exist and quantity is bounded by both remaining amounts.
                    self.orders
                        .get_mut(&bid.id)
                        .expect("matched bid exists")
                        .remaining -= quantity;
                    self.orders
                        .get_mut(&ask.id)
                        .expect("matched ask exists")
                        .remaining -= quantity;
                    volume += u128::from(quantity);
                    turnover += u128::from(quantity) * u128::from(price);
                    events.push(Event::TradeExecuted {
                        buy_order_id: bid.id,
                        sell_order_id: ask.id,
                        buyer: bid.agent_id,
                        seller: ask.agent_id,
                        resource,
                        quantity,
                        price,
                    });
                }
            }
            if let Some(weighted_price) = turnover.checked_div(volume) {
                self.prices[resource.index()] = u64::try_from(weighted_price)
                    .map_err(|_| DomainError::Overflow("volume weighted price"))?;
            }
        }
        Ok(events)
    }

    /// Include actual bilateral settlements in the tick's volume-weighted reference prices.
    /// Ordinary matching remains independently usable when no negotiations occurred.
    pub fn include_negotiated_trades(&mut self, events: &[Event]) -> Result<()> {
        if !events.iter().any(|event| {
            matches!(
                event,
                Event::NegotiatedTrade { .. }
                    | Event::Contract {
                        event: ContractEvent::Delivered { .. }
                    }
            )
        }) {
            return Ok(());
        }
        let mut quantities = [0_u128; RESOURCE_COUNT];
        let mut values = [0_u128; RESOURCE_COUNT];
        for event in events {
            if let Event::TradeExecuted {
                resource,
                quantity,
                price,
                ..
            }
            | Event::NegotiatedTrade {
                resource,
                quantity,
                price,
                ..
            }
            | Event::Contract {
                event:
                    ContractEvent::Delivered {
                        resource,
                        quantity,
                        price,
                        ..
                    },
            } = event
            {
                let index = resource.index();
                quantities[index] += u128::from(*quantity);
                values[index] += u128::from(*quantity) * u128::from(*price);
            }
        }
        for resource in Resource::ALL {
            let index = resource.index();
            if let Some(price) = values[index].checked_div(quantities[index]) {
                self.prices[index] = u64::try_from(price)
                    .map_err(|_| DomainError::Overflow("combined market price"))?;
            }
        }
        Ok(())
    }

    /// Expire all unfilled remainders and release collateral before household use.
    pub fn expire_all(&mut self, world: &mut World) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        while let Some((&id, order)) = self.orders.first_key_value() {
            if order.remaining > 0 {
                world.with_agent_mut(order.agent_id, |agent| match order.side {
                    OrderSide::Buy => agent
                        .account
                        .release_buy(order.limit_price, order.remaining),
                    OrderSide::Sell => agent.account.release_sell(order.resource, order.remaining),
                })?;
                events.push(Event::OrderExpired {
                    order_id: id,
                    agent_id: order.agent_id,
                    remaining: order.remaining,
                });
            }
            self.orders.remove(&id);
        }
        Ok(events)
    }
}

/// Guard external order quantities and prices before any multiplication.
pub fn validate_order_values(quantity: u64, price: Money) -> Result<()> {
    if quantity == 0 || quantity > MAX_ORDER_QUANTITY || price == 0 || price > MAX_PRICE {
        return Err(DomainError::InvalidAction(format!(
            "quantity must be 1..={MAX_ORDER_QUANTITY}; price 1..={MAX_PRICE}"
        )));
    }
    notional(price, quantity)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn market() -> (World, OrderBook) {
        let config = SimulationConfig {
            agents: 3,
            ..Default::default()
        };
        let mut world = World::seeded(&config).unwrap();
        for id in 0..3 {
            world
                .with_agent_mut(AgentId(id), |a| {
                    a.account = Account::new(1000, Inventory::new([10, 0, 0, 0]));
                    Ok(())
                })
                .unwrap();
        }
        (world, OrderBook::new(config.initial_prices).unwrap())
    }

    #[test]
    fn trust_breaks_equal_price_ties_but_never_overrides_a_better_price() {
        for (trusted_price, expected_seller) in [(80, AgentId(2)), (81, AgentId(1))] {
            let (mut world, mut book) = market();
            world
                .with_agent_mut(AgentId(0), |agent| {
                    agent.relationships.insert(
                        AgentId(2),
                        Relationship {
                            trust: 90,
                            ..Default::default()
                        },
                    );
                    Ok(())
                })
                .unwrap();
            book.place(
                &mut world,
                AgentId(1),
                OrderSide::Sell,
                Resource::Food,
                1,
                80,
                1,
            )
            .unwrap();
            book.place(
                &mut world,
                AgentId(2),
                OrderSide::Sell,
                Resource::Food,
                1,
                trusted_price,
                1,
            )
            .unwrap();
            book.place(
                &mut world,
                AgentId(0),
                OrderSide::Buy,
                Resource::Food,
                1,
                100,
                1,
            )
            .unwrap();
            let events = book.match_orders(&mut world).unwrap();
            assert!(
                matches!(events.as_slice(), [Event::TradeExecuted { seller, .. }] if *seller == expected_seller)
            );
        }
    }

    #[test]
    fn price_groups_advance_after_partial_fills_and_skip_cheaper_self_orders() {
        let (mut world, mut book) = market();
        world
            .with_agent_mut(AgentId(0), |agent| {
                agent.relationships.insert(
                    AgentId(2),
                    Relationship {
                        trust: 90,
                        ..Default::default()
                    },
                );
                Ok(())
            })
            .unwrap();
        for (seller, quantity, price) in [(0, 1, 60), (1, 1, 70), (2, 2, 70), (1, 1, 90)] {
            book.place(
                &mut world,
                AgentId(seller),
                OrderSide::Sell,
                Resource::Food,
                quantity,
                price,
                1,
            )
            .unwrap();
        }
        book.place(
            &mut world,
            AgentId(0),
            OrderSide::Buy,
            Resource::Food,
            4,
            100,
            1,
        )
        .unwrap();
        let fills: Vec<_> = book
            .match_orders(&mut world)
            .unwrap()
            .into_iter()
            .map(|event| match event {
                Event::TradeExecuted {
                    seller,
                    quantity,
                    price,
                    ..
                } => (seller, quantity, price),
                _ => panic!("matching emits only trades"),
            })
            .collect();
        assert_eq!(
            fills,
            [
                (AgentId(2), 2, 70),
                (AgentId(1), 1, 70),
                (AgentId(1), 1, 90)
            ]
        );
        book.expire_all(&mut world).unwrap();
        for (id, money, food) in [(0, 700, 14), (1, 1160, 8), (2, 1140, 8)] {
            let account = &world.agent(AgentId(id)).unwrap().account;
            assert_eq!(account.available_cash(), money);
            assert_eq!(account.available(Resource::Food), food);
        }
        assert!(book.is_empty());
    }

    #[test]
    fn rejects_unfunded_and_unbacked_orders_without_mutation() {
        let (mut world, mut book) = market();
        let before = world.snapshot();
        assert!(
            book.place(
                &mut world,
                AgentId(0),
                OrderSide::Buy,
                Resource::Food,
                11,
                100,
                1
            )
            .is_err()
        );
        assert!(
            book.place(
                &mut world,
                AgentId(0),
                OrderSide::Sell,
                Resource::Wood,
                1,
                100,
                1
            )
            .is_err()
        );
        assert!(
            book.place(
                &mut world,
                AgentId(0),
                OrderSide::Buy,
                Resource::Food,
                0,
                100,
                1
            )
            .is_err()
        );
        assert_eq!(world.snapshot(), before);
        assert_eq!(book.snapshot().unwrap().next_order_id, 0);
    }

    #[test]
    fn partial_fill_expiry_and_maker_price_conserve_assets() {
        let (mut world, mut book) = market();
        book.place(
            &mut world,
            AgentId(1),
            OrderSide::Sell,
            Resource::Food,
            3,
            80,
            1,
        )
        .unwrap();
        book.place(
            &mut world,
            AgentId(0),
            OrderSide::Buy,
            Resource::Food,
            5,
            100,
            1,
        )
        .unwrap();
        let trades = book.match_orders(&mut world).unwrap();
        assert!(matches!(
            trades.as_slice(),
            [Event::TradeExecuted {
                quantity: 3,
                price: 80,
                ..
            }]
        ));
        assert_eq!(
            world.agent(AgentId(0)).unwrap().account.reserved_cash(),
            200
        );
        assert_eq!(book.prices()[0], 80);
        assert_eq!(book.expire_all(&mut world).unwrap().len(), 1);
        assert!(book.is_empty());
        assert_eq!(
            world.agents().map(|a| a.account.wallet()).sum::<u64>(),
            3000
        );
        assert_eq!(
            world
                .agents()
                .map(|a| a.account.inventory().get(Resource::Food))
                .sum::<u64>(),
            30
        );
        assert!(world.agents().all(|a| a.account.reserved_cash() == 0));
    }

    #[test]
    fn higher_bid_then_earlier_bid_wins() {
        let (mut world, mut book) = market();
        book.place(
            &mut world,
            AgentId(0),
            OrderSide::Buy,
            Resource::Food,
            2,
            90,
            1,
        )
        .unwrap();
        book.place(
            &mut world,
            AgentId(1),
            OrderSide::Buy,
            Resource::Food,
            2,
            100,
            1,
        )
        .unwrap();
        book.place(
            &mut world,
            AgentId(2),
            OrderSide::Sell,
            Resource::Food,
            2,
            80,
            1,
        )
        .unwrap();
        assert!(matches!(
            book.match_orders(&mut world).unwrap().as_slice(),
            [Event::TradeExecuted {
                buyer: AgentId(1),
                price: 100,
                ..
            }]
        ));
        book.expire_all(&mut world).unwrap();
        book.place(
            &mut world,
            AgentId(0),
            OrderSide::Buy,
            Resource::Food,
            2,
            100,
            2,
        )
        .unwrap();
        book.place(
            &mut world,
            AgentId(1),
            OrderSide::Buy,
            Resource::Food,
            2,
            100,
            2,
        )
        .unwrap();
        book.place(
            &mut world,
            AgentId(2),
            OrderSide::Sell,
            Resource::Food,
            2,
            80,
            2,
        )
        .unwrap();
        assert!(matches!(
            book.match_orders(&mut world).unwrap().as_slice(),
            [Event::TradeExecuted {
                buyer: AgentId(0),
                ..
            }]
        ));
    }

    #[test]
    fn noncrossing_orders_expire_and_prices_remain_defined() {
        let (mut world, mut book) = market();
        let prices = *book.prices();
        book.place(
            &mut world,
            AgentId(0),
            OrderSide::Buy,
            Resource::Food,
            2,
            50,
            1,
        )
        .unwrap();
        book.place(
            &mut world,
            AgentId(1),
            OrderSide::Sell,
            Resource::Food,
            2,
            80,
            1,
        )
        .unwrap();
        assert!(book.match_orders(&mut world).unwrap().is_empty());
        assert_eq!(book.expire_all(&mut world).unwrap().len(), 2);
        assert_eq!(book.prices(), &prices);
        assert_eq!(
            world.agent(AgentId(0)).unwrap().account.available_cash(),
            1000
        );
        assert_eq!(
            world
                .agent(AgentId(1))
                .unwrap()
                .account
                .available(Resource::Food),
            10
        );
    }

    #[test]
    fn self_order_does_not_block_a_valid_counterparty() {
        let (mut world, mut book) = market();
        book.place(
            &mut world,
            AgentId(0),
            OrderSide::Sell,
            Resource::Food,
            1,
            50,
            1,
        )
        .unwrap();
        book.place(
            &mut world,
            AgentId(1),
            OrderSide::Sell,
            Resource::Food,
            1,
            60,
            1,
        )
        .unwrap();
        book.place(
            &mut world,
            AgentId(0),
            OrderSide::Buy,
            Resource::Food,
            1,
            70,
            1,
        )
        .unwrap();
        assert!(matches!(
            book.match_orders(&mut world).unwrap().as_slice(),
            [Event::TradeExecuted {
                buyer: AgentId(0),
                seller: AgentId(1),
                price: 60,
                ..
            }]
        ));
    }

    #[test]
    fn duplicate_orders_cannot_reuse_collateral() {
        let (mut world, mut book) = market();
        book.place(
            &mut world,
            AgentId(0),
            OrderSide::Buy,
            Resource::Food,
            8,
            100,
            1,
        )
        .unwrap();
        assert!(
            book.place(
                &mut world,
                AgentId(0),
                OrderSide::Buy,
                Resource::Food,
                3,
                100,
                1
            )
            .is_err()
        );
        book.place(
            &mut world,
            AgentId(1),
            OrderSide::Sell,
            Resource::Food,
            8,
            100,
            1,
        )
        .unwrap();
        assert!(
            book.place(
                &mut world,
                AgentId(1),
                OrderSide::Sell,
                Resource::Food,
                3,
                100,
                1
            )
            .is_err()
        );
    }
}
