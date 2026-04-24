use crate::types::{Price, Quantity, Side};

#[derive(Debug, Clone, Copy)]
pub enum Action {
    PlaceLimit { side: Side, price: Price, qty: Quantity, post_only: bool, volume_ahead: Quantity },
    CancelAll,
}

#[derive(Debug, Clone, Copy)]
pub struct Order {
    pub id: u64,
    pub side: Side,
    pub price: Price,
    pub qty: Quantity,
    pub volume_ahead: Quantity,
}

pub struct MockExchange {
    pub position: i64,      
    pub usdt_balance: f64,  
    pub active_orders: Vec<Order>,
    pub next_order_id: u64,
    pub maker_fee: f64,
    pub taker_fee: f64,
    pub total_placed: u64,
    pub total_rejections: u64,
    pub total_fills: u64,
    pub average_entry_price: f64,
}

impl MockExchange {
    pub fn new() -> Self {
        Self {
            position: 0,
            usdt_balance: 0.0,
            active_orders: Vec::new(),
            next_order_id: 1,
            maker_fee: -0.00015, // PRO3 VIP Maker Rebate (-0.015%)
            taker_fee: 0.0002,   // PRO3 VIP Taker Fee (0.02%)
            total_placed: 0,
            total_rejections: 0,
            total_fills: 0,
            average_entry_price: 0.0,
        }
    }

    pub fn place_limit_order(&mut self, side: Side, price: Price, qty: Quantity, post_only: bool, volume_ahead: Quantity, best_bid: Price, best_ask: Price) -> Option<u64> {
        self.total_placed += 1;
        
        let crosses = match side {
            Side::Bid => price >= best_ask,
            Side::Ask => price <= best_bid,
        };

        if crosses {
            if post_only {
                self.total_rejections += 1;
                return None; 
            } else {
                // TAKER FILL: Order crosses spread and PostOnly is false.
                let fill_qty = qty; 
                let execute_price = match side {
                    Side::Bid => best_ask,
                    Side::Ask => best_bid,
                }; 

                let notional = (execute_price as f64) * (fill_qty as f64);

                // Update Average Entry Price
                if self.position == 0 {
                    self.average_entry_price = execute_price as f64;
                } else if (self.position > 0 && side == Side::Bid) || (self.position < 0 && side == Side::Ask) {
                    let old_cost = self.average_entry_price * self.position.abs() as f64;
                    let new_cost = execute_price as f64 * fill_qty as f64;
                    self.average_entry_price = (old_cost + new_cost) / (self.position.abs() as f64 + fill_qty as f64);
                }

                // Charge Taker Fee
                if side == Side::Bid {
                    self.position += fill_qty as i64;
                    self.usdt_balance -= notional * (1.0 + self.taker_fee); 
                } else {
                    self.position -= fill_qty as i64;
                    self.usdt_balance += notional * (1.0 - self.taker_fee); 
                }

                self.total_fills += 1;
                return None; // Fully filled, nothing rests in book.
            }
        }

        let id = self.next_order_id;
        self.next_order_id += 1;
        self.active_orders.push(Order { id, side, price, qty, volume_ahead });
        Some(id)
    }

    pub fn cancel_all(&mut self) {
        self.active_orders.clear();
    }

    pub fn on_market_trade(&mut self, trade_price: Price, trade_side: Side, trade_qty: Quantity) {
        let mut i = 0;
        let mut remaining_market_qty = trade_qty;
        
        while i < self.active_orders.len() && remaining_market_qty > 0 {
            let order = &mut self.active_orders[i];
            
            // Taker hit the resting order side?
            // A market sell (Side::Ask taker) hits a resting Bid
            let hit_bid = order.side == Side::Bid && trade_side == Side::Ask && trade_price <= order.price;
            // A market buy (Side::Bid taker) hits a resting Ask
            let hit_ask = order.side == Side::Ask && trade_side == Side::Bid && trade_price >= order.price;

            if hit_bid || hit_ask {
                // Volume ahead logic: Taker volume first eats the volume ahead of us in the queue.
                if remaining_market_qty <= order.volume_ahead {
                    order.volume_ahead -= remaining_market_qty;
                    remaining_market_qty = 0;
                } else {
                    remaining_market_qty -= order.volume_ahead;
                    order.volume_ahead = 0;
                    
                    // Now fill our order!
                    let fill_qty = order.qty.min(remaining_market_qty);
                    let notional = (order.price as f64) * (fill_qty as f64);

                    // Update Average Entry Price
                    if self.position == 0 {
                        self.average_entry_price = order.price as f64;
                    } else if (self.position > 0 && order.side == Side::Bid) || (self.position < 0 && order.side == Side::Ask) {
                        let old_cost = self.average_entry_price * self.position.abs() as f64;
                        let new_cost = order.price as f64 * fill_qty as f64;
                        self.average_entry_price = (old_cost + new_cost) / (self.position.abs() as f64 + fill_qty as f64);
                    }

                    if order.side == Side::Bid {
                        self.position += fill_qty as i64;
                        self.usdt_balance -= notional * (1.0 + self.maker_fee);
                    } else {
                        self.position -= fill_qty as i64;
                        self.usdt_balance += notional * (1.0 - self.maker_fee);
                    }
                    
                    order.qty -= fill_qty;
                    remaining_market_qty -= fill_qty;
                    self.total_fills += 1;
                }
            }

            if self.active_orders[i].qty == 0 {
                self.active_orders.remove(i);
            } else {
                i += 1;
            }
        }
    }
    
    pub fn get_equity(&self, mid_price: f64) -> f64 {
        self.usdt_balance + (self.position as f64 * mid_price)
    }

    pub fn get_unrealized_pnl(&self, mid_price: f64) -> f64 {
        if self.position == 0 {
            return 0.0;
        }
        (mid_price - self.average_entry_price) * (self.position as f64)
    }
}
