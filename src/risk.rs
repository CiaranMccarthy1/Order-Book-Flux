use crate::types::{Price, Side};

pub struct RiskManager {
    pub max_inventory: i64,
    pub max_unrealized_loss: f64,
    pub max_price_deviation_pct: f64,
    
    pub recent_attempts: Vec<bool>,
    pub max_rejection_rate: f64,
    pub pause_until_tick: u64,
    
    pub is_killed: bool,
}

impl RiskManager {
    pub fn new(max_inventory: i64, max_unrealized_loss: f64) -> Self {
        Self {
            max_inventory,
            max_unrealized_loss,
            max_price_deviation_pct: 0.01,
            recent_attempts: Vec::with_capacity(100),
            max_rejection_rate: 0.8, // Pause if >80% rejected
            pause_until_tick: 0,
            is_killed: false,
        }
    }

    pub fn can_quote(&self, position: i64, side: Side) -> bool {
        match side {
            Side::Bid => position < self.max_inventory,
            Side::Ask => position > -self.max_inventory,
        }
    }

    pub fn check_fat_finger(&self, mid_price: Price, order_price: Price) -> bool {
        let deviation = (order_price as f64 - mid_price as f64).abs() / (mid_price as f64);
        deviation <= self.max_price_deviation_pct
    }

    pub fn check_unrealized_loss(&mut self, unrealized_pnl: f64) -> bool {
        if unrealized_pnl < -self.max_unrealized_loss {
            self.is_killed = true;
            false
        } else {
            true
        }
    }

    pub fn record_placement(&mut self, accepted: bool, current_tick: u64) {
        if self.recent_attempts.len() >= 100 {
            self.recent_attempts.remove(0);
        }
        self.recent_attempts.push(accepted);

        if self.recent_attempts.len() == 100 {
            let rejections = self.recent_attempts.iter().filter(|&&x| !x).count();
            let rejection_rate = rejections as f64 / 100.0;
            if rejection_rate >= self.max_rejection_rate {
                // Pause for 60 seconds (simulated as 60,000 ticks)
                self.pause_until_tick = current_tick + 60_000;
                self.recent_attempts.clear();
            }
        }
    }

    pub fn is_active(&self, current_tick: u64) -> bool {
        !self.is_killed && current_tick >= self.pause_until_tick
    }
}
