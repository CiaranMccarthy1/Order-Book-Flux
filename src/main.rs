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

fn main() {
    let cores = core_affinity::get_core_ids().unwrap_or_default();
    let processing_core = cores.first().copied();
    let producer_core = cores.get(1).copied().or(processing_core);

    if let Some(core) = processing_core {
        pin_current_thread(core);
    }

    let rb = HeapRb::<LevelUpdate>::new(1 << 14);
    let (mut producer, mut consumer) = rb.split();

    let _producer_thread = thread::spawn(move || {
        if let Some(core) = producer_core {
            pin_current_thread(core);
        }

        let config = ExchangeConfig::default();
        if let Err(err) = stream_binance_depth_with_handler(config, |side, price, qty| {
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
        }) {
            eprintln!("Binance stream error: {}", err);
        }
    });

    let mut engine = OfiEngine::default();
    let mut processed = 0u64;
    let mut last_report = Instant::now();

    loop {
        if let Some(update) = consumer.try_pop() {
            engine.process_level_update(update.side, update.price, update.qty);
            processed += 1;
        } else {
            core::hint::spin_loop();
        }

        if last_report.elapsed() >= Duration::from_secs(5) {
            println!("Processed {} updates", processed);
            println!("Latest OFI signal: {}", engine.latest_signal());
            println!("Top-5 imbalance: {}", engine.top5_snapshot_imbalance());
            last_report = Instant::now();
        }
    }
}
