use crate::types::{Price, Quantity, Side};
use crate::mock::MockExchange;

pub struct MarketMakerStrategy {
    pub quote_distance: Price,
    pub order_qty: Quantity,

    // EWMA state for OFI Z-score
    pub ofi_ewma: f64,
    pub ofi_var: f64,
    pub alpha: f64,

    pub ticks_since_last_quote: u64,
    pub current_ttl: u64,
}

impl MarketMakerStrategy {
    pub fn new(quote_distance: Price, order_qty: Quantity) -> Self {
        Self {
            quote_distance,
            order_qty,
            ofi_ewma: 0.0,
            ofi_var: 1.0, // prevent div by zero
            alpha: 0.01,  // EWMA decay factor
            ticks_since_last_quote: 0,
            current_ttl: 0,
        }
    }

    pub fn tick(&mut self, exchange: &mut MockExchange, current_ofi: i64, best_bid: Option<Price>, best_ask: Option<Price>) {
        let ofi_f = current_ofi as f64;
        
        // Update EWMA
        let diff = ofi_f - self.ofi_ewma;
        self.ofi_ewma += self.alpha * diff;
        self.ofi_var = (1.0 - self.alpha) * (self.ofi_var + self.alpha * diff * diff);
        let std_dev = self.ofi_var.sqrt().max(0.0001);
        
        let z_score = (ofi_f - self.ofi_ewma) / std_dev;
        let abs_z = z_score.abs();

        self.ticks_since_last_quote += 1;

        // Only cancel and replace orders if TTL has expired
        if self.ticks_since_last_quote >= self.current_ttl {
            if let (Some(bb), Some(ba)) = (best_bid, best_ask) {
                let mid = (bb + ba) / 2;
                
                exchange.cancel_all();

                // Base TTL is 100 ticks. High volatility (high Z-score) reduces it rapidly
                // so we cancel fast and don't get run over by toxic flow.
                self.current_ttl = (100.0 / (1.0 + abs_z)).max(1.0).round() as u64;
                self.ticks_since_last_quote = 0;

                // Quote based on Z-score
                if z_score > 2.0 {
                    // Strong buying pressure: lean bids
                    exchange.place_limit_order(Side::Bid, mid - self.quote_distance / 2, self.order_qty);
                } else if z_score < -2.0 {
                    // Strong selling pressure: lean asks
                    exchange.place_limit_order(Side::Ask, mid + self.quote_distance / 2, self.order_qty);
                } else {
                    // Neutral: symmetrical quoting
                    exchange.place_limit_order(Side::Bid, mid - self.quote_distance, self.order_qty);
                    exchange.place_limit_order(Side::Ask, mid + self.quote_distance, self.order_qty);
                }
            }
        }
    }
}
