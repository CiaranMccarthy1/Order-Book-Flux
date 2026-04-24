use crate::types::{Price, Quantity, Side};
use crate::mock::Action;

pub struct MarketMakerStrategy {
    pub quote_distance: Price,
    pub order_qty: Quantity,

    pub ofi_ewma: f64,
    pub ofi_var: f64,
    pub alpha: f64,

    pub ticks_since_last_quote: u64,
    pub current_ttl: u64,

    pub price_ema: f64,
    pub price_ema_alpha: f64,
}

impl MarketMakerStrategy {
    pub fn new(quote_distance: Price, order_qty: Quantity) -> Self {
        Self {
            quote_distance,
            order_qty,
            ofi_ewma: 0.0,
            ofi_var: 1.0, 
            alpha: 0.01,  
            ticks_since_last_quote: 0,
            current_ttl: 0,
            price_ema: 0.0,
            price_ema_alpha: 0.001,
        }
    }

    // Instead of acting directly on exchange, emit Actions.
    pub fn tick(&mut self, current_ofi: i64, best_bid: Option<(Price, Quantity)>, best_ask: Option<(Price, Quantity)>, position: i64, risk: &crate::risk::RiskManager) -> Vec<Action> {
        let mut actions = Vec::new();
        let ofi_f = current_ofi as f64;
        
        let diff = ofi_f - self.ofi_ewma;
        self.ofi_ewma += self.alpha * diff;
        self.ofi_var = (1.0 - self.alpha) * (self.ofi_var + self.alpha * diff * diff);
        let std_dev = self.ofi_var.sqrt().max(0.0001);
        
        let z_score = (ofi_f - self.ofi_ewma) / std_dev;
        let abs_z = z_score.abs();

        self.ticks_since_last_quote += 1;

        if self.ticks_since_last_quote >= self.current_ttl {
            if let (Some((bb_price, bb_qty)), Some((ba_price, ba_qty))) = (best_bid, best_ask) {
                let mid = (bb_price + ba_price) / 2;
                let mid_f = mid as f64;

                // Update Price EMA
                if self.price_ema == 0.0 {
                    self.price_ema = mid_f;
                } else {
                    self.price_ema += self.price_ema_alpha * (mid_f - self.price_ema);
                }

                // Dynamic Threshold: Scale base 1.5 threshold with volatility proxy (std_dev)
                let dynamic_threshold = (1.5 * (1.0 + std_dev * 0.02)).clamp(1.5, 3.0);
                
                // EMA Trend Filter
                let trend_is_up = mid_f > self.price_ema + 0.5;
                let trend_is_down = mid_f < self.price_ema - 0.5;

                actions.push(Action::CancelAll);

                self.current_ttl = (100.0 / (1.0 + abs_z)).max(1.0).round() as u64;
                self.ticks_since_last_quote = 0;

                let inventory_skew = (position as f64 * 0.1).round() as i64;
                
                let predictive_skew = if z_score > dynamic_threshold { 1 } else if z_score < -dynamic_threshold { -1 } else { 0 };
                
                let bid_price = (mid as i64 - self.quote_distance as i64 - inventory_skew + predictive_skew).max(1) as u64;
                let ask_price = (mid as i64 + self.quote_distance as i64 - inventory_skew + predictive_skew).max(1) as u64;

                let bid_vol_ahead = if bid_price >= bb_price { bb_qty } else { bb_qty * 2 };
                let ask_vol_ahead = if ask_price <= ba_price { ba_qty } else { ba_qty * 2 };

                // If signal is exceptionally strong, disable PostOnly to guarantee execution as a Taker.
                let mut post_only_bid = true;
                let mut post_only_ask = true;
                if z_score > 2.5 {
                    post_only_bid = false; // Pay taker fee to capture strong breakout
                } else if z_score < -2.5 {
                    post_only_ask = false; // Pay taker fee to capture strong breakdown
                }

                // Only quote if RiskManager allows it, and avoid fighting a strong trend
                let can_bid = risk.can_quote(position, Side::Bid) && risk.check_fat_finger(mid, bid_price) && !trend_is_down;
                let can_ask = risk.can_quote(position, Side::Ask) && risk.check_fat_finger(mid, ask_price) && !trend_is_up;

                if z_score > dynamic_threshold {
                    if can_bid { actions.push(Action::PlaceLimit { side: Side::Bid, price: bid_price, qty: self.order_qty, post_only: post_only_bid, volume_ahead: bid_vol_ahead }); }
                } else if z_score < -dynamic_threshold {
                    if can_ask { actions.push(Action::PlaceLimit { side: Side::Ask, price: ask_price, qty: self.order_qty, post_only: post_only_ask, volume_ahead: ask_vol_ahead }); }
                } else {
                    if can_bid { actions.push(Action::PlaceLimit { side: Side::Bid, price: bid_price, qty: self.order_qty, post_only: post_only_bid, volume_ahead: bid_vol_ahead }); }
                    if can_ask { actions.push(Action::PlaceLimit { side: Side::Ask, price: ask_price, qty: self.order_qty, post_only: post_only_ask, volume_ahead: ask_vol_ahead }); }
                }
            }
        }

        actions
    }
}
