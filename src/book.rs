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
                if self.best_bid.map_or(true, |b| price >= b) {
                    self.best_bid = self.bids.keys().next_back().copied();
                }
            }
            Side::Ask => {
                if self.best_ask.map_or(true, |a| price <= a) {
                    self.best_ask = self.asks.keys().next().copied();
                }
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
    fn nth_price(&self, side: Side, depth: usize) -> Option<Price> {
        match side {
            Side::Bid => self.bids.keys().rev().nth(depth.saturating_sub(1)).copied(),
            Side::Ask => self.asks.keys().nth(depth.saturating_sub(1)).copied(),
        }
    }

    #[inline]
    fn is_in_top_n(&self, side: Side, price: Price, depth: usize) -> bool {
        match (side, self.nth_price(side, depth)) {
            (Side::Bid, Some(boundary)) => price >= boundary,
            (Side::Ask, Some(boundary)) => price <= boundary,
            (_, None) => true, // fewer than depth levels exist
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_book_has_no_best() {
        let book = LimitOrderBook::new();
        assert!(book.best_bid().is_none());
        assert!(book.best_ask().is_none());
        assert_eq!(book.top_n_sum(Side::Bid, 5), 0);
        assert_eq!(book.top_n_sum(Side::Ask, 5), 0);
    }

    #[test]
    fn insert_single_bid() {
        let mut book = LimitOrderBook::new();
        book.update_level(Side::Bid, 100, 50);
        assert_eq!(book.best_bid(), Some((100, 50)));
        assert!(book.best_ask().is_none());
    }

    #[test]
    fn insert_single_ask() {
        let mut book = LimitOrderBook::new();
        book.update_level(Side::Ask, 101, 30);
        assert_eq!(book.best_ask(), Some((101, 30)));
        assert!(book.best_bid().is_none());
    }

    #[test]
    fn best_bid_tracks_highest_price() {
        let mut book = LimitOrderBook::new();
        book.update_level(Side::Bid, 100, 10);
        book.update_level(Side::Bid, 105, 20);
        book.update_level(Side::Bid, 102, 15);
        assert_eq!(book.best_bid(), Some((105, 20)));
    }

    #[test]
    fn best_ask_tracks_lowest_price() {
        let mut book = LimitOrderBook::new();
        book.update_level(Side::Ask, 110, 10);
        book.update_level(Side::Ask, 105, 20);
        book.update_level(Side::Ask, 108, 15);
        assert_eq!(book.best_ask(), Some((105, 20)));
    }

    #[test]
    fn remove_best_bid_updates_cache() {
        let mut book = LimitOrderBook::new();
        book.update_level(Side::Bid, 100, 10);
        book.update_level(Side::Bid, 105, 20);
        book.update_level(Side::Bid, 105, 0);
        assert_eq!(book.best_bid(), Some((100, 10)));
    }

    #[test]
    fn remove_best_ask_updates_cache() {
        let mut book = LimitOrderBook::new();
        book.update_level(Side::Ask, 100, 10);
        book.update_level(Side::Ask, 105, 20);
        book.update_level(Side::Ask, 100, 0);
        assert_eq!(book.best_ask(), Some((105, 20)));
    }

    #[test]
    fn remove_last_level_clears_best() {
        let mut book = LimitOrderBook::new();
        book.update_level(Side::Bid, 100, 10);
        book.update_level(Side::Bid, 100, 0);
        assert!(book.best_bid().is_none());
    }

    #[test]
    fn deep_book_update_does_not_affect_best_bid() {
        let mut book = LimitOrderBook::new();
        book.update_level(Side::Bid, 100, 10);
        book.update_level(Side::Bid, 105, 20);
        book.update_level(Side::Bid, 100, 15);
        assert_eq!(book.best_bid(), Some((105, 20)));
    }

    #[test]
    fn top_n_sum_bids() {
        let mut book = LimitOrderBook::new();
        for i in 0..10 {
            book.update_level(Side::Bid, 100 + i, 10);
        }
        // Top 5 bids: 109,108,107,106,105 => 50
        assert_eq!(book.top_n_sum(Side::Bid, 5), 50);
    }

    #[test]
    fn top_n_sum_asks() {
        let mut book = LimitOrderBook::new();
        for i in 0..10 {
            book.update_level(Side::Ask, 200 + i, 5);
        }
        // Top 5 asks: 200,201,202,203,204 => 25
        assert_eq!(book.top_n_sum(Side::Ask, 5), 25);
    }

    #[test]
    fn top_n_sum_fewer_than_n_levels() {
        let mut book = LimitOrderBook::new();
        book.update_level(Side::Bid, 100, 7);
        book.update_level(Side::Bid, 101, 3);
        assert_eq!(book.top_n_sum(Side::Bid, 5), 10);
    }

    #[test]
    fn level_change_reports_old_and_new_qty() {
        let mut book = LimitOrderBook::new();
        book.update_level(Side::Bid, 100, 10);
        let change = book.update_level(Side::Bid, 100, 25);
        assert_eq!(change.old_qty, 10);
        assert_eq!(change.new_qty, 25);
    }

    #[test]
    fn level_change_reports_best_shift() {
        let mut book = LimitOrderBook::new();
        book.update_level(Side::Bid, 100, 10);
        let change = book.update_level(Side::Bid, 110, 5);
        assert_eq!(change.old_best, Some(100));
        assert_eq!(change.new_best, Some(110));
    }

    #[test]
    fn update_qty_at_existing_level() {
        let mut book = LimitOrderBook::new();
        book.update_level(Side::Ask, 200, 100);
        book.update_level(Side::Ask, 200, 50);
        assert_eq!(book.best_ask(), Some((200, 50)));
    }
}
