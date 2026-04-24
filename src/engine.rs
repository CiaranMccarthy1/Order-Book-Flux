#[cfg(feature = "std")]
use serde_json;
#[cfg(feature = "simd-json")]
use simd_json;

use crate::book::{LevelChange, LimitOrderBook};
#[cfg(feature = "std")]
use crate::types::MarketDataMessage;
use crate::types::Side;

pub struct OfiEngine {
    book: LimitOrderBook,
    top_depth: usize,
    latest_signal: i64,
}

impl OfiEngine {
    pub fn new() -> Self {
        Self {
            book: LimitOrderBook::new(),
            top_depth: 5,
            latest_signal: 0,
        }
    }

    pub fn with_depth(top_depth: usize) -> Self {
        Self {
            book: LimitOrderBook::new(),
            top_depth,
            latest_signal: 0,
        }
    }

    #[inline]
    pub fn latest_signal(&self) -> i64 {
        self.latest_signal
    }

    #[inline]
    pub fn top5_snapshot_imbalance(&self) -> i64 {
        let bid = self.book.top_n_sum(Side::Bid, self.top_depth) as i64;
        let ask = self.book.top_n_sum(Side::Ask, self.top_depth) as i64;
        bid - ask
    }

    #[inline]
    pub fn best_bid(&self) -> Option<(crate::types::Price, crate::types::Quantity)> {
        self.book.best_bid()
    }

    #[inline]
    pub fn best_ask(&self) -> Option<(crate::types::Price, crate::types::Quantity)> {
        self.book.best_ask()
    }

    #[inline]
    pub fn process_level_update(&mut self, side: Side, price: u64, qty: u64) -> i64 {
        let change = self.book.update_level(side, price, qty);
        let delta = self.compute_delta(change);
        self.latest_signal += delta;
        delta
    }

    /// Deserialise and ingest one raw WebSocket frame.
    /// Uses simd-json when the feature is enabled (AVX2/NEON path),
    /// falls back to serde_json otherwise.
    #[cfg(feature = "std")]
    #[inline]
    pub fn process_packet(&mut self, payload: &[u8]) -> Result<i64, serde_json::Error> {
        // simd-json requires a mutable buffer (it writes in-place during parsing).
        #[cfg(feature = "simd-json")]
        {
            let mut buf = payload.to_vec();
            let msg: MarketDataMessage<'_> = simd_json::from_slice(&mut buf)
                .map_err(|e| serde_json::Error::io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;
            return Ok(self.process_level_update(msg.side, msg.price, msg.qty));
        }
        #[cfg(not(feature = "simd-json"))]
        {
            let msg: MarketDataMessage<'_> = serde_json::from_slice(payload)?;
            Ok(self.process_level_update(msg.side, msg.price, msg.qty))
        }
    }

    #[inline]
    fn compute_delta(&self, change: LevelChange) -> i64 {
        let mut delta = 0i64;

        if change.old_best != change.new_best {
            // Best-price shift is the dominant signal — captures the full
            // directional impact so we skip the quantity-diff branch to
            // avoid double-counting.
            delta += self.best_price_shift_component(change);
        } else if change.was_top_n || change.is_top_n {
            // No best-price move: normal quantity change within the top-N.
            let diff = change.new_qty as i64 - change.old_qty as i64;
            delta += match change.side {
                Side::Bid => diff,
                Side::Ask => -diff,
            };
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
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_engine_signal_is_zero() {
        let engine = OfiEngine::new();
        assert_eq!(engine.latest_signal(), 0);
    }

    #[test]
    fn first_bid_produces_positive_delta() {
        let mut engine = OfiEngine::new();
        let delta = engine.process_level_update(Side::Bid, 100, 10);
        // First bid into empty book: best shifts from None -> Some(100)
        assert!(delta > 0, "first bid should produce positive delta, got {}", delta);
    }

    #[test]
    fn first_ask_produces_negative_delta() {
        let mut engine = OfiEngine::new();
        let delta = engine.process_level_update(Side::Ask, 101, 10);
        // First ask into empty book: best shifts from None -> Some(101)
        assert!(delta < 0, "first ask should produce negative delta, got {}", delta);
    }

    #[test]
    fn bid_qty_increase_without_best_shift() {
        let mut engine = OfiEngine::new();
        engine.process_level_update(Side::Bid, 100, 10);
        // Increase qty at same level — no best change
        let delta = engine.process_level_update(Side::Bid, 100, 15);
        assert_eq!(delta, 5, "bid qty increase of 5 should produce delta +5");
    }

    #[test]
    fn ask_qty_increase_without_best_shift() {
        let mut engine = OfiEngine::new();
        engine.process_level_update(Side::Ask, 200, 10);
        // Increase qty at same level — no best change
        let delta = engine.process_level_update(Side::Ask, 200, 18);
        assert_eq!(delta, -8, "ask qty increase of 8 should produce delta -8");
    }

    #[test]
    fn bid_qty_decrease_without_best_shift() {
        let mut engine = OfiEngine::new();
        engine.process_level_update(Side::Bid, 100, 10);
        let delta = engine.process_level_update(Side::Bid, 100, 3);
        assert_eq!(delta, -7, "bid qty decrease of 7 should produce delta -7");
    }

    #[test]
    fn signal_accumulates_across_updates() {
        let mut engine = OfiEngine::new();
        let d1 = engine.process_level_update(Side::Bid, 100, 10);
        let d2 = engine.process_level_update(Side::Ask, 200, 5);
        assert_eq!(engine.latest_signal(), d1 + d2);
    }

    #[test]
    fn best_bid_shift_up_produces_positive_delta() {
        let mut engine = OfiEngine::new();
        engine.process_level_update(Side::Bid, 100, 10);
        // New higher bid
        let delta = engine.process_level_update(Side::Bid, 110, 20);
        assert!(delta > 0, "best bid shifting up should produce positive delta, got {}", delta);
        assert_eq!(delta, 20); // new_qty for the new best
    }

    #[test]
    fn best_bid_shift_down_produces_negative_delta() {
        let mut engine = OfiEngine::new();
        engine.process_level_update(Side::Bid, 100, 10);
        engine.process_level_update(Side::Bid, 110, 20);
        // Remove the best bid — best shifts down
        let delta = engine.process_level_update(Side::Bid, 110, 0);
        assert!(delta < 0, "best bid shifting down should produce negative delta, got {}", delta);
    }

    #[test]
    fn no_double_counting_on_best_shift() {
        // This is the core regression test for the double-counting bug.
        // When the best bid is removed, only the best-price-shift component
        // should fire, NOT both the quantity-diff and the shift.
        let mut engine = OfiEngine::new();
        engine.process_level_update(Side::Bid, 100, 10);
        engine.process_level_update(Side::Bid, 110, 20);

        let delta = engine.process_level_update(Side::Bid, 110, 0);
        // Should be -(old_qty) = -20 from best_price_shift_component only.
        // Before the fix it was -40 (double-counted).
        assert_eq!(delta, -20);
    }

    #[test]
    fn snapshot_imbalance() {
        let mut engine = OfiEngine::new();
        engine.process_level_update(Side::Bid, 100, 30);
        engine.process_level_update(Side::Ask, 200, 10);
        assert_eq!(engine.top5_snapshot_imbalance(), 20); // 30 - 10
    }

    #[test]
    fn configurable_depth() {
        let mut engine = OfiEngine::with_depth(2);
        engine.process_level_update(Side::Bid, 100, 10);
        engine.process_level_update(Side::Bid, 101, 10);
        engine.process_level_update(Side::Bid, 102, 10);
        // Only top 2 should count: 102 + 101 = 20
        assert_eq!(engine.top5_snapshot_imbalance(), 20);
    }

    #[cfg(feature = "std")]
    #[test]
    fn process_packet_valid_json() {
        let mut engine = OfiEngine::new();
        let packet = br#"{"symbol":"XBTUSD","side":"bid","price":50000,"qty":10,"ts_nanos":1}"#;
        let result = engine.process_packet(packet);
        assert!(result.is_ok());
    }

    #[cfg(feature = "std")]
    #[test]
    fn process_packet_invalid_json() {
        let mut engine = OfiEngine::new();
        let result = engine.process_packet(b"not json");
        assert!(result.is_err());
    }
}
