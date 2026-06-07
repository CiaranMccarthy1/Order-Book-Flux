#![cfg(feature = "std")]

use std::thread;
use std::time::{Duration, Instant};

use core_affinity::CoreId;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;

use order_book_flux::connections::{stream_binance_depth_with_handler, ExchangeConfig};
use order_book_flux::engine::OfiEngine;
use order_book_flux::types::Side;

fn pin_current_thread(core_id: CoreId) {
    let _ = core_affinity::set_for_current(core_id);
}

#[derive(Copy, Clone, Debug)]
struct LevelUpdate {
    side: Side,
    price: u64,
    qty: u64,
}

fn pow10_u64(scale: u32) -> u64 {
    let mut v = 1u64;
    for _ in 0..scale {
        v = v.checked_mul(10).unwrap();
    }
    v
}

fn format_with_commas_u64(mut n: u64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    while n > 0 {
        let chunk = (n % 1000) as u16;
        parts.push(format!("{:03}", chunk));
        n /= 1000;
    }
    let mut s = parts.pop().unwrap();
    while let Some(p) = parts.pop() {
        s.push_str(",");
        s.push_str(&p);
    }
    // remove leading zeros from the highest chunk
    if s.starts_with('0') {
        // this can only happen if the highest chunk had leading zeros; strip them
        let first_comma = s.find(',');
        if let Some(idx) = first_comma {
            let (head, rest) = s.split_at(idx);
            let head_trimmed = head.trim_start_matches('0');
            s = if head_trimmed.is_empty() { format!("0{}", rest) } else { format!("{}{}", head_trimmed, rest) };
        }
    }
    s
}

fn format_with_commas_i64(n: i64) -> String {
    if n < 0 {
        format!("-{}", format_with_commas_u64((-n) as u64))
    } else {
        format_with_commas_u64(n as u64)
    }
}

fn format_price(price: u64, scale: u32) -> String {
    let sf = pow10_u64(scale);
    let int = price / sf;
    let frac = price % sf;
    if frac == 0 {
        format_with_commas_u64(int)
    } else {
        let mut frac_str = format!("{:0width$}", frac, width = scale as usize);
        // trim trailing zeros
        while frac_str.ends_with('0') {
            frac_str.pop();
        }
        format!("{}.{}", format_with_commas_u64(int), frac_str)
    }
}

fn format_currency(price: u64, scale: u32) -> String {
    let sf = pow10_u64(scale);
    let int = price / sf;
    let frac = price % sf;
    let frac_str = format!("{:0width$}", frac, width = scale as usize);
    format!("${}.{}", format_with_commas_u64(int), frac_str)
}

fn main() {
    let cores = core_affinity::get_core_ids().unwrap_or_default();
    let processing_core = cores.first().copied();
    let producer_core = cores.get(1).copied().or(processing_core);

    if let Some(core) = processing_core {
        pin_current_thread(core);
    }

    let rb = HeapRb::<LevelUpdate>::new(1 << 14);
    let (mut producer, mut consumer) = rb.split();

    // Create exchange config here so the main thread can use the price_scale for formatting
    let config = ExchangeConfig::default();
    let config_for_thread = config.clone();

    let _producer_thread = thread::spawn(move || {
        if let Some(core) = producer_core {
            pin_current_thread(core);
        }

        if let Err(err) = stream_binance_depth_with_handler(config_for_thread, |side, price, qty| {
            let mut update = LevelUpdate { side, price, qty };
            loop {
                match producer.try_push(update) {
                    Ok(()) => break,
                    Err(returned_update) => {
                        update = returned_update;
                        core::hint::spin_loop();
                    }
                }
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

    // Column widths
    const W_UPDATE: usize = 9;
    const W_OFI: usize = 15;
    const W_DELTA: usize = 13;
    const W_TOP5: usize = 13;
    const W_SPREAD: usize = 9;
    const W_BEST: usize = 22;

    loop {
        if let Some(update) = consumer.try_pop() {
            engine.process_level_update(update.side, update.price, update.qty);
            processed += 1;
        } else {
            core::hint::spin_loop();
        }

        if last_report.elapsed() >= Duration::from_secs(5) {
            if !printed_header {
                println!(
                    "{:>W_UPDATE$} │ {:>W_OFI$} │ {:>W_DELTA$} │ {:>W_TOP5$} │ {:>W_SPREAD$} │ {:>W_BEST$}",
                    "Update#",
                    "OFI Signal",
                    "Signal Δ",
                    "Top-5 Imb",
                    "Spread",
                    "Best Bid/Ask"
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

            let ofi_str = format_with_commas_i64(current_signal);
            let delta_str = if delta >= 0 {
                format!("+{}", format_with_commas_i64(delta))
            } else {
                format_with_commas_i64(delta)
            };

            let top5_str = if top5 >= 0 {
                format!("+{}", format_with_commas_i64(top5))
            } else {
                format_with_commas_i64(top5)
            };

            let (bb_str, ba_str, spread_str) = match (engine.best_bid(), engine.best_ask()) {
                (Some((bb_price, _)), Some((ba_price, _))) => {
                    let bb = format_price(bb_price, config.price_scale);
                    let ba = format_price(ba_price, config.price_scale);
                    let spread = if ba_price >= bb_price { ba_price - bb_price } else { bb_price - ba_price };
                    (bb, ba, format_currency(spread, config.price_scale))
                }
                (Some((bb_price, _)), None) => (format_price(bb_price, config.price_scale), "-".to_string(), "-".to_string()),
                (None, Some((ba_price, _))) => ("-".to_string(), format_price(ba_price, config.price_scale), "-".to_string()),
                _ => ("-".to_string(), "-".to_string(), "-".to_string()),
            };

            let best_pair = format!("{} / {}", bb_str, ba_str);

            println!(
                "{:>W_UPDATE$} │ {:>W_OFI$} │ {:>W_DELTA$} │ {:>W_TOP5$} │ {:>W_SPREAD$} │ {:>W_BEST$}",
                processed,
                ofi_str,
                delta_str,
                top5_str,
                spread_str,
                best_pair
            );

            last_report = Instant::now();
        }
    }
}
