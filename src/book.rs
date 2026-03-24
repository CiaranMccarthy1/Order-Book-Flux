use alloc::collections::BTreeMap;

use crate::types::{Price, Quantity, Side};

#[derive(Debug, Copy, Clone)]
pub struct LevelChange {
    pub side: Side,
    pub price: Price,
    pub old_qty: Quantity,
    pub new_qty: Quantity,
    pub old_best: Option<Price>,
    pub new_best: Option<Price>,
    pub was_top_n: bool,
    pub is_top_n: bool,
}

#[derive(Default)]
pub struct LimitOrderBook {
    bids: BTreeMap<Price, Quantity>,
    asks: BTreeMap<Price, Quantity>,
    best_bid: Option<Price>,
    best_ask: Option<Price>,
}

impl LimitOrderBook {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn best_bid(&self) -> Option<(Price, Quantity)> {
        self.best_bid
            .and_then(|p| self.bids.get(&p).copied().map(|q| (p, q)))
    }

    #[inline]
    pub fn best_ask(&self) -> Option<(Price, Quantity)> {
        self.best_ask
            .and_then(|p| self.asks.get(&p).copied().map(|q| (p, q)))
    }

    pub fn top_n_sum(&self, side: Side, depth: usize) -> Quantity {
        match side {
            Side::Bid => self
                .bids
                .iter()
                .rev()
                .take(depth)
                .fold(0u64, |acc, (_, q)| acc.saturating_add(*q)),
            Side::Ask => self
                .asks
                .iter()
                .take(depth)
                .fold(0u64, |acc, (_, q)| acc.saturating_add(*q)),
        }
    }

    #[inline]
    pub fn update_level(&mut self, side: Side, price: Price, new_qty: Quantity) -> LevelChange {
        let old_best = match side {
            Side::Bid => self.best_bid,
            Side::Ask => self.best_ask,
        };

        let was_top_n = self.is_in_top_n(side, price, 5);

        let (old_qty, map) = match side {
            Side::Bid => {
                let old = self.bids.get(&price).copied().unwrap_or(0);
                (old, &mut self.bids)
            }
            Side::Ask => {
                let old = self.asks.get(&price).copied().unwrap_or(0);
                (old, &mut self.asks)
            }
        };

        if new_qty == 0 {
            map.remove(&price);
        } else {
            map.insert(price, new_qty);
        }

        match side {
            Side::Bid => {
                self.best_bid = self.bids.keys().next_back().copied();
            }
            Side::Ask => {
                self.best_ask = self.asks.keys().next().copied();
            }
        }

        let new_best = match side {
            Side::Bid => self.best_bid,
            Side::Ask => self.best_ask,
        };

        let is_top_n = self.is_in_top_n(side, price, 5);

        LevelChange {
            side,
            price,
            old_qty,
            new_qty,
            old_best,
            new_best,
            was_top_n,
            is_top_n,
        }
    }

    #[inline]
    fn is_in_top_n(&self, side: Side, price: Price, depth: usize) -> bool {
        match side {
            Side::Bid => self.bids.keys().rev().take(depth).any(|p| *p == price),
            Side::Ask => self.asks.keys().take(depth).any(|p| *p == price),
        }
    }
}
