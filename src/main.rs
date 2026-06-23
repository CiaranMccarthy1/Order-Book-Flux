#![cfg(feature = "std")]

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use order_book_flux::connections::{stream_binance_depth_with_handler, ExchangeConfig};
use order_book_flux::engine::OfiEngine;
use order_book_flux::types::Side;


#[derive(Copy, Clone, Debug)]
struct LevelUpdate {
    side: Side,
    price: u64,
    qty: u64,
}


fn format_price(price: u64, scale: u32) -> String {
    let sf = 10u64.checked_pow(scale).unwrap();
    let int = price / sf;
    let frac = price % sf;
    if frac == 0 {
        format!("{}", int)
    } else {
        let mut frac_str = format!("{:0width$}", frac, width = scale as usize);
        while frac_str.ends_with('0') {
            frac_str.pop();
        }
        format!("{}.{}", int, frac_str)
    }
}

fn main() {
  
    let (producer, consumer) = mpsc::sync_channel::<LevelUpdate>(1 << 14);

    let config = ExchangeConfig::default();
    let config_for_thread = config.clone();

    let _producer_thread = thread::spawn(move || {
        if let Err(err) = stream_binance_depth_with_handler(config_for_thread, |side, price, qty| {
            let update = LevelUpdate { side, price, qty };
            if producer.send(update).is_err() {
                return false;
            }
            true
        }) {
            eprintln!("Binance stream error: {}", err);
        }
    });

    let mut engine = OfiEngine::default();
    let mut processed = 0u64;
    let mut last_report = Instant::now();
    let mut last_signal = engine.latest_signal();
    let mut printed_header = false;

    const W_UPDATE: usize = 9;
    const W_OFI: usize = 15;
    const W_DELTA: usize = 13;
    const W_TOP5: usize = 13;
    const W_SPREAD: usize = 9;
    const W_BEST: usize = 22;

    loop {
        if let Ok(update) = consumer.try_recv() {
            engine.process_level_update(update.side, update.price, update.qty);
            processed += 1;
        } else {
            core::hint::spin_loop();
        }

        if last_report.elapsed() >= Duration::from_secs(5) {
            if !printed_header {
                println!(
                    "{:>W_UPDATE$} │ {:>W_OFI$} │ {:>W_DELTA$} │ {:>W_TOP5$} │ {:>W_SPREAD$} │ {:>W_BEST$}",
                    "Update#", "OFI Signal", "Signal Δ", "Top-5 Imb", "Spread", "Best Bid/Ask"
                );
                println!(
                    "{}┼{}┼{}┼{}┼{}┼{}",
                    "─".repeat(W_UPDATE),
                    "─".repeat(W_OFI),
                    "─".repeat(W_DELTA),
                    "─".repeat(W_TOP5),
                    "─".repeat(W_SPREAD),
                    "─".repeat(W_BEST)
                );
                printed_header = true;
            }

            let current_signal = engine.latest_signal();
            let delta = current_signal - last_signal;
            last_signal = current_signal;
            let top5 = engine.top5_snapshot_imbalance();

            let ofi_str = format!("{}", current_signal);
            let delta_str = if delta >= 0 { format!("+{}", delta) } else { format!("{}", delta) };
            let top5_str = if top5 >= 0 { format!("+{}", top5) } else { format!("{}", top5) };

            let (bb_str, ba_str, spread_str) = match (engine.best_bid(), engine.best_ask()) {
                (Some((bb_price, _)), Some((ba_price, _))) => {
                    let bb = format_price(bb_price, config.price_scale);
                    let ba = format_price(ba_price, config.price_scale);
                    let spread = if ba_price >= bb_price { ba_price - bb_price } else { bb_price - ba_price };
                    (bb, ba, format!("${}", format_price(spread, config.price_scale)))
                }
                (Some((bb_price, _)), None) => (format_price(bb_price, config.price_scale), "-".to_string(), "-".to_string()),
                (None, Some((ba_price, _))) => ("-".to_string(), format_price(ba_price, config.price_scale), "-".to_string()),
                _ => ("-".to_string(), "-".to_string(), "-".to_string()),
            };

            let best_pair = format!("{} / {}", bb_str, ba_str);

            println!(
                "{:>W_UPDATE$} │ {:>W_OFI$} │ {:>W_DELTA$} │ {:>W_TOP5$} │ {:>W_SPREAD$} │ {:>W_BEST$}",
                processed, ofi_str, delta_str, top5_str, spread_str, best_pair
            );

            last_report = Instant::now();
        }
    }
}
