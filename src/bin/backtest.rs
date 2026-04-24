use order_book_flux::engine::OfiEngine;
use order_book_flux::mock::MockExchange;
use order_book_flux::strategy::MarketMakerStrategy;
use order_book_flux::types::Side;
use std::env;
use std::time::SystemTime;

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
}

fn run_simulation(seed: u64) -> SimulationResult {
    let mut engine = OfiEngine::new();
    let mut exchange = MockExchange::new();
    let mut strategy = MarketMakerStrategy::new(2, 1); 

    let mut fair_value: f64 = 50000.0;
    let anchor_price: f64 = 50000.0; // Mean reversion anchor
    let mut rng = Lcg::new(seed); 
    
    // We'll feed 100000 ticks
    for _ in 0..100000 {
        // 1. Ornstein-Uhlenbeck process for fair value (mean-reverting)
        let theta = 0.01;  
        let sigma = 0.5;   
        
        let drift = theta * (anchor_price - fair_value);
        let shock = sigma * rng.next_gaussian();
        
        // Jump diffusion: 0.5% chance of a large heavy-tail jump
        let jump = if rng.next_f64() < 0.005 {
            rng.next_gaussian() * 15.0 
        } else {
            0.0
        };

        fair_value += drift + shock + jump;

        let best_bid_u64 = fair_value.floor() as u64 - 1;
        let best_ask_u64 = fair_value.ceil() as u64 + 1;

        let bid_qty = (rng.next_f64() * 50.0) as u64 + 5;
        let ask_qty = (rng.next_f64() * 50.0) as u64 + 5;

        engine.process_level_update(Side::Bid, best_bid_u64, bid_qty);
        engine.process_level_update(Side::Ask, best_ask_u64, ask_qty);

        let ofi = engine.latest_signal();

        strategy.tick(
            &mut exchange,
            ofi,
            Some(best_bid_u64),
            Some(best_ask_u64),
        );

        let prob_buy = 0.5 + (ofi as f64 * 0.005).clamp(-0.45, 0.45);
        
        let trade_price = if rng.next_f64() < prob_buy {
            best_ask_u64
        } else {
            best_bid_u64
        };

        exchange.on_market_trade(trade_price);
    }

    let final_price = fair_value.round();
    SimulationResult {
        final_price,
        position: exchange.position,
        equity: exchange.get_equity(final_price),
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
    println!("Running {} simulation(s) with Micro-structure models...", runs);

    let mut total_equity = 0.0;
    let mut profitable_runs = 0;
    let start_seed = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos() as u64;

    for i in 0..runs {
        // Offset the seed so each run is unique
        let result = run_simulation(start_seed + i as u64);
        
        total_equity += result.equity;
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
        println!("\n--- Multi-Run Aggregation ---");
        println!("Total Runs: {}", runs);
        println!("Average Equity (PnL): ${:.2}", avg_equity);
        println!("Win Rate: {:.1}%", win_rate);
    }
}
