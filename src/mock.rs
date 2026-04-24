use crate::types::{Price, Quantity, Side};

#[derive(Debug, Clone, Copy)]
pub struct Order {
    pub id: u64,
    pub side: Side,
    pub price: Price,
    pub qty: Quantity,
}

pub struct MockExchange {
    pub position: i64,      // Positive = long, Negative = short
    pub usdt_balance: f64,  // Realized PnL
    pub active_orders: Vec<Order>,
    pub next_order_id: u64,
}

impl MockExchange {
    pub fn new() -> Self {
        Self {
            position: 0,
            usdt_balance: 0.0,
            active_orders: Vec::new(),
            next_order_id: 1,
        }
    }

    pub fn place_limit_order(&mut self, side: Side, price: Price, qty: Quantity) -> u64 {
        let id = self.next_order_id;
        self.next_order_id += 1;
        self.active_orders.push(Order { id, side, price, qty });
        id
    }

    pub fn cancel_all(&mut self) {
        self.active_orders.clear();
    }

    // Call this when the market trades to simulate fills
    pub fn on_market_trade(&mut self, trade_price: Price) {
        let mut i = 0;
        while i < self.active_orders.len() {
            let order = &self.active_orders[i];
            let filled = match order.side {
                Side::Bid => trade_price <= order.price, // Market sold down to our bid
                Side::Ask => trade_price >= order.price, // Market bought up to our ask
            };

            if filled {
                // Execute trade
                let qty_i64 = order.qty as i64;
                match order.side {
                    Side::Bid => {
                        self.position += qty_i64;
                        self.usdt_balance -= (order.price as f64) * (order.qty as f64);
                    }
                    Side::Ask => {
                        self.position -= qty_i64;
                        self.usdt_balance += (order.price as f64) * (order.qty as f64);
                    }
                }
                self.active_orders.remove(i);
            } else {
                i += 1;
            }
        }
    }
    
    // Evaluate total equity given the current market mid price
    pub fn get_equity(&self, mid_price: f64) -> f64 {
        self.usdt_balance + (self.position as f64 * mid_price)
    }
}
