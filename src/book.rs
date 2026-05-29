use alloc::vec::Vec;

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

const MAX_LEVELS: usize = 200_000;

pub struct LimitOrderBook {
    bids: Vec<Quantity>,
    asks: Vec<Quantity>,
    base_price: Price,
    tick_size: Price,
    best_bid_idx: Option<usize>,
    best_ask_idx: Option<usize>,
}

impl LimitOrderBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_window(base_price: Price, tick_size: Price) -> Self {
        let mut book = Self::default();
        book.base_price = base_price;
        book.tick_size = tick_size.max(1);
        book
    }

    /// Shift the window base. Intended for non-hot paths; clears cached levels.
    pub fn shift_window(&mut self, new_base_price: Price) {
        self.base_price = new_base_price;
        self.bids.fill(0);
        self.asks.fill(0);
        self.best_bid_idx = None;
        self.best_ask_idx = None;
    }

    #[inline]
    pub fn best_bid(&self) -> Option<(Price, Quantity)> {
        self.best_bid_idx
            .map(|idx| (self.index_to_price(idx), self.bids[idx]))
    }

    #[inline]
    pub fn best_ask(&self) -> Option<(Price, Quantity)> {
        self.best_ask_idx
            .map(|idx| (self.index_to_price(idx), self.asks[idx]))
    }

    pub fn top_n_sum(&self, side: Side, depth: usize) -> Quantity {
        if depth == 0 {
            return 0;
        }

        match side {
            Side::Bid => {
                let mut sum = 0u64;
                let mut remaining = depth;
                if let Some(best) = self.best_bid_idx {
                    for idx in (0..=best).rev() {
                        let qty = self.bids[idx];
                        if qty == 0 {
                            continue;
                        }
                        sum = sum.saturating_add(qty);
                        remaining -= 1;
                        if remaining == 0 {
                            break;
                        }
                    }
                }
                sum
            }
            Side::Ask => {
                let mut sum = 0u64;
                let mut remaining = depth;
                if let Some(best) = self.best_ask_idx {
                    for idx in best..MAX_LEVELS {
                        let qty = self.asks[idx];
                        if qty == 0 {
                            continue;
                        }
                        sum = sum.saturating_add(qty);
                        remaining -= 1;
                        if remaining == 0 {
                            break;
                        }
                    }
                }
                sum
            }
        }
    }

    #[inline]
    pub fn update_level(&mut self, side: Side, price: Price, new_qty: Quantity) -> LevelChange {
        let old_best = match side {
            Side::Bid => self.best_bid_idx.map(|idx| self.index_to_price(idx)),
            Side::Ask => self.best_ask_idx.map(|idx| self.index_to_price(idx)),
        };

        let was_top_n = self.is_in_top_n(side, price, 5);

        let idx = match self.price_to_index(price) {
            Some(value) => value,
            None => {
                if self.is_out_of_window(price) {
                    self.recenter_window(price);
                }

                match self.price_to_index(price) {
                    Some(value) => value,
                    None => {
                        let new_best = old_best;
                        return LevelChange {
                            side,
                            price,
                            old_qty: 0,
                            new_qty: 0,
                            old_best,
                            new_best,
                            was_top_n,
                            is_top_n: was_top_n,
                        };
                    }
                }
            }
        };

        let old_qty = match side {
            Side::Bid => self.bids[idx],
            Side::Ask => self.asks[idx],
        };

        match side {
            Side::Bid => self.bids[idx] = new_qty,
            Side::Ask => self.asks[idx] = new_qty,
        }

        match side {
            Side::Bid => {
                if new_qty > 0 {
                    if self.best_bid_idx.map_or(true, |b| idx >= b) {
                        self.best_bid_idx = Some(idx);
                    }
                } else if self.best_bid_idx == Some(idx) {
                    self.best_bid_idx = self.find_prev_nonzero_bid(idx);
                }
            }
            Side::Ask => {
                if new_qty > 0 {
                    if self.best_ask_idx.map_or(true, |a| idx <= a) {
                        self.best_ask_idx = Some(idx);
                    }
                } else if self.best_ask_idx == Some(idx) {
                    self.best_ask_idx = self.find_next_nonzero_ask(idx);
                }
            }
        }

        let new_best = match side {
            Side::Bid => self.best_bid_idx.map(|value| self.index_to_price(value)),
            Side::Ask => self.best_ask_idx.map(|value| self.index_to_price(value)),
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
        if depth == 0 {
            return None;
        }

        match side {
            Side::Bid => {
                let mut remaining = depth;
                if let Some(best) = self.best_bid_idx {
                    for idx in (0..=best).rev() {
                        if self.bids[idx] == 0 {
                            continue;
                        }
                        remaining -= 1;
                        if remaining == 0 {
                            return Some(self.index_to_price(idx));
                        }
                    }
                }
                None
            }
            Side::Ask => {
                let mut remaining = depth;
                if let Some(best) = self.best_ask_idx {
                    for idx in best..MAX_LEVELS {
                        if self.asks[idx] == 0 {
                            continue;
                        }
                        remaining -= 1;
                        if remaining == 0 {
                            return Some(self.index_to_price(idx));
                        }
                    }
                }
                None
            }
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

    #[inline]
    fn price_to_index(&self, price: Price) -> Option<usize> {
        if price < self.base_price {
            return None;
        }
        let offset = price - self.base_price;
        if offset % self.tick_size != 0 {
            return None;
        }
        let idx = (offset / self.tick_size) as usize;
        if idx >= MAX_LEVELS {
            return None;
        }
        Some(idx)
    }

    #[inline]
    fn is_out_of_window(&self, price: Price) -> bool {
        if price < self.base_price {
            return true;
        }
        let window_span = (MAX_LEVELS as Price).saturating_sub(1) * self.tick_size;
        price > self.base_price.saturating_add(window_span)
    }

    #[inline]
    fn recenter_window(&mut self, price: Price) {
        let window_span = (MAX_LEVELS as Price).saturating_sub(1) * self.tick_size;
        let half_span = window_span / 2;
        let raw_base = price.saturating_sub(half_span);
        let aligned_base = raw_base - (raw_base % self.tick_size);
        self.shift_window(aligned_base);
    }

    #[inline]
    fn index_to_price(&self, idx: usize) -> Price {
        self.base_price + (idx as Price) * self.tick_size
    }

    #[inline]
    fn find_prev_nonzero_bid(&self, start: usize) -> Option<usize> {
        for idx in (0..start).rev() {
            if self.bids[idx] > 0 {
                return Some(idx);
            }
        }
        None
    }

    #[inline]
    fn find_next_nonzero_ask(&self, start: usize) -> Option<usize> {
        if start + 1 >= MAX_LEVELS {
            return None;
        }
        for idx in (start + 1)..MAX_LEVELS {
            if self.asks[idx] > 0 {
                return Some(idx);
            }
        }
        None
    }
}

impl Default for LimitOrderBook {
    fn default() -> Self {
        Self {
            bids: vec![0; MAX_LEVELS],
            asks: vec![0; MAX_LEVELS],
            base_price: 0,
            tick_size: 1,
            best_bid_idx: None,
            best_ask_idx: None,
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
