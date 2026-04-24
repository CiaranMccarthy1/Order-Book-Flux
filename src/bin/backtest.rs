use order_book_flux::engine::OfiEngine;
use order_book_flux::mock::{MockExchange, Action};
use order_book_flux::risk::RiskManager;
use order_book_flux::strategy::MarketMakerStrategy;
use order_book_flux::types::Side;
use std::env;
use std::time::SystemTime;
use std::collections::VecDeque;

struct Lcg {
    state: u64,
}
impl Lcg {
    fn new(seed: u64) -> Self { Self { state: seed } }
    fn next_f64(&mut self) -> f64 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.state >> 11) as f64 / (1u64 << 53) as f64
    }
    fn next_gaussian(&mut self) -> f64 {
        let mut sum = 0.0;
        for _ in 0..12 {
            sum += self.next_f64();
        }
        sum - 6.0
    }
}

struct SimulationResult {
    final_price: f64,
    position: i64,
    equity: f64,
    total_placed: u64,
    total_fills: u64,
    total_rejections: u64,
}

fn run_simulation(seed: u64) -> SimulationResult {
    let mut engine = OfiEngine::new();
    let mut exchange = MockExchange::new();
    let mut strategy = MarketMakerStrategy::new(1, 2); 
    let mut risk = RiskManager::new(50, 500.0); // max inventory 50, max unrealized loss $500

    let mut fair_value: f64 = 50000.0;
    let anchor_price: f64 = 50000.0; 
    let mut rng = Lcg::new(seed); 
    
    // Latency queue: (Execution_Tick, Action)
    let mut latency_queue: VecDeque<(u64, Action)> = VecDeque::new();
    let network_latency_ticks = 100; // Simulated latency
    
    for current_tick in 0..100000u64 {
        let theta = 0.01;  
        let sigma = 0.5;   
        
        let drift = theta * (anchor_price - fair_value);
        let shock = sigma * rng.next_gaussian();
        
        // Toxic Flow Jump Diffusion
        let mut jump = 0.0;
        let mut market_sweep = false;

        if rng.next_f64() < 0.005 {
            jump = rng.next_gaussian() * 15.0; 
            market_sweep = true; // Signals adverse selection
        }

        fair_value += drift + shock + jump;

        let best_bid_u64 = fair_value.floor() as u64 - 1;
        let best_ask_u64 = fair_value.ceil() as u64 + 1;

        let bid_qty = (rng.next_f64() * 50.0) as u64 + 5;
        let ask_qty = (rng.next_f64() * 50.0) as u64 + 5;

        engine.process_level_update(Side::Bid, best_bid_u64, bid_qty);
        engine.process_level_update(Side::Ask, best_ask_u64, ask_qty);

        // Process network latency queue BEFORE market trades
        while let Some(&(exec_tick, _)) = latency_queue.front() {
            if current_tick >= exec_tick {
                let (_, action) = latency_queue.pop_front().unwrap();
                match action {
                    Action::CancelAll => exchange.cancel_all(),
                    Action::PlaceLimit { side, price, qty, post_only, volume_ahead } => {
                        let accepted = exchange.place_limit_order(side, price, qty, post_only, volume_ahead, best_bid_u64, best_ask_u64).is_some();
                        risk.record_placement(accepted, current_tick);
                    }
                }
            } else {
                break;
            }
        }

        // Simulating the Toxic Flow sweeping the book
        if market_sweep {
            if jump > 0.0 {
                // Price jumps up, implies aggressive market buys (hitting asks)
                exchange.on_market_trade(best_ask_u64 + (jump as u64), Side::Bid, 10000); 
            } else if jump < 0.0 {
                // Price jumps down, implies aggressive market sells (hitting bids)
                exchange.on_market_trade(best_bid_u64 - (jump.abs() as u64), Side::Ask, 10000);
            }
        }

        let ofi = engine.latest_signal();
        let bb = engine.best_bid();
        let ba = engine.best_ask();

        // Check Daily Drawdown / Max Unrealized Loss Kill Switch
        let unrealized_pnl = exchange.get_unrealized_pnl(fair_value);
        if !risk.check_unrealized_loss(unrealized_pnl) {
            // "Immediately close all positions at market"
            // For mock simplicity, we just cancel all and stop quoting.
            exchange.cancel_all();
        }

        if risk.is_active(current_tick) {
            let actions = strategy.tick(ofi, bb, ba, exchange.position, &risk);
            for action in actions {
                latency_queue.push_back((current_tick + network_latency_ticks, action));
            }
        }

        // Normal Bid-Ask bounce flow
        let prob_buy = 0.5 + (ofi as f64 * 0.005).clamp(-0.45, 0.45);
        let (trade_price, trade_side) = if rng.next_f64() < prob_buy {
            (best_ask_u64, Side::Bid)
        } else {
            (best_bid_u64, Side::Ask)
        };

        // Small taker size hits the resting book
        let trade_qty = (rng.next_f64() * 10.0) as u64 + 1;
        exchange.on_market_trade(trade_price, trade_side, trade_qty);
    }

    let final_price = fair_value.round();
    SimulationResult {
        final_price,
        position: exchange.position,
        equity: exchange.get_equity(final_price),
        total_placed: exchange.total_placed,
        total_fills: exchange.total_fills,
        total_rejections: exchange.total_rejections,
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut runs = 1;

    for i in 0..args.len() {
        if args[i] == "--runs" && i + 1 < args.len() {
            if let Ok(n) = args[i + 1].parse::<usize>() {
                runs = n;
            }
        }
    }

    println!("Starting Bybit Market Making Backtest...");
    println!("Features: Queue Priority, Network Latency, Toxic Flow, PostOnly Rejections, Bybit Fees.");
    println!("Running {} simulation(s) with Micro-structure models...", runs);

    let mut total_equity = 0.0;
    let mut profitable_runs = 0;
    let mut sum_placed = 0;
    let mut sum_fills = 0;
    let mut sum_rejections = 0;
    let start_seed = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos() as u64;

    for i in 0..runs {
        let result = run_simulation(start_seed + i as u64);
        
        total_equity += result.equity;
        sum_placed += result.total_placed;
        sum_fills += result.total_fills;
        sum_rejections += result.total_rejections;

        if result.equity > 0.0 {
            profitable_runs += 1;
        }

        if runs == 1 {
            println!("Backtest complete.");
            println!("Final market price: ${}", result.final_price);
            println!("Final position: {} units", result.position);
            println!("Total Equity: ${:.2}", result.equity);
        } else if (i + 1) % 10 == 0 || i + 1 == runs {
            println!("Completed {}/{} runs...", i + 1, runs);
        }
    }

    if runs > 1 {
        let avg_equity = total_equity / runs as f64;
        let win_rate = (profitable_runs as f64 / runs as f64) * 100.0;
        let fill_rate = if sum_placed > 0 { (sum_fills as f64 / sum_placed as f64) * 100.0 } else { 0.0 };
        let reject_rate = if sum_placed > 0 { (sum_rejections as f64 / sum_placed as f64) * 100.0 } else { 0.0 };

        println!("\n--- Multi-Run Aggregation ---");
        println!("Total Runs: {}", runs);
        println!("Average Equity (PnL): ${:.2}", avg_equity);
        println!("Win Rate: {:.1}%", win_rate);
        println!("Fill Rate: {:.2}%", fill_rate);
        println!("PostOnly Rejection Rate: {:.2}%", reject_rate);
    }
}
