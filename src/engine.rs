use core::sync::atomic::{AtomicI64, Ordering};

#[cfg(feature = "std")]
use serde_json;

use crate::book::{LevelChange, LimitOrderBook};
use crate::pool::OrderPool;
#[cfg(feature = "std")]
use crate::types::MarketDataMessage;
use crate::types::Side;

pub struct OfiEngine {
    book: LimitOrderBook,
    order_pool: OrderPool,
    top_depth: usize,
    latest_signal: AtomicI64,
}

impl OfiEngine {
    pub fn with_capacity(pool_capacity: usize) -> Self {
        Self {
            book: LimitOrderBook::new(),
            order_pool: OrderPool::with_capacity(pool_capacity),
            top_depth: 5,
            latest_signal: AtomicI64::new(0),
        }
    }

    #[inline]
    pub fn latest_signal(&self) -> i64 {
        self.latest_signal.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn top5_snapshot_imbalance(&self) -> i64 {
        let bid = self.book.top_n_sum(Side::Bid, self.top_depth) as i64;
        let ask = self.book.top_n_sum(Side::Ask, self.top_depth) as i64;
        bid - ask
    }

    #[inline]
    pub fn process_level_update(&mut self, side: Side, price: u64, qty: u64, ts_nanos: u64) -> i64 {
        if let Some(idx) = self.order_pool.acquire_index() {
            if let Some(order) = self.order_pool.get_mut(idx) {
                order.side = side;
                order.price = price;
                order.qty = qty;
                order.ts_nanos = ts_nanos;
            }

            let change = self.book.update_level(side, price, qty);
            let delta = self.compute_delta(change);
            self.latest_signal.fetch_add(delta, Ordering::Relaxed);

            self.order_pool.release_index(idx);
            return delta;
        }

        let change = self.book.update_level(side, price, qty);
        let delta = self.compute_delta(change);
        self.latest_signal.fetch_add(delta, Ordering::Relaxed);
        delta
    }

    #[cfg(feature = "std")]
    #[inline]
    pub fn process_packet(&mut self, payload: &[u8]) -> Result<i64, serde_json::Error> {
        let msg: MarketDataMessage<'_> = serde_json::from_slice(payload)?;
        let _symbol = msg.symbol;
        Ok(self.process_level_update(msg.side, msg.price, msg.qty, msg.ts_nanos))
    }

    #[inline]
    fn compute_delta(&self, change: LevelChange) -> i64 {
        let mut delta = 0i64;

        if change.was_top_n || change.is_top_n {
            let diff = change.new_qty as i64 - change.old_qty as i64;
            delta += match change.side {
                Side::Bid => diff,
                Side::Ask => -diff,
            };
        }

        if change.old_best != change.new_best {
            delta += self.best_price_shift_component(change);
        }

        delta
    }

    #[inline]
    fn best_price_shift_component(&self, change: LevelChange) -> i64 {
        match change.side {
            Side::Bid => match (change.old_best, change.new_best) {
                (Some(old), Some(new)) if new > old => change.new_qty as i64,
                (Some(old), Some(new)) if new < old => -(change.old_qty as i64),
                (None, Some(_)) => change.new_qty as i64,
                (Some(_), None) => -(change.old_qty as i64),
                _ => 0,
            },
            Side::Ask => match (change.old_best, change.new_best) {
                (Some(old), Some(new)) if new < old => -(change.new_qty as i64),
                (Some(old), Some(new)) if new > old => change.old_qty as i64,
                (None, Some(_)) => -(change.new_qty as i64),
                (Some(_), None) => change.old_qty as i64,
                _ => 0,
            },
        }
    }
}

impl Default for OfiEngine {
    fn default() -> Self {
        Self::with_capacity(4096)
    }
}
