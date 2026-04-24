#![cfg(feature = "std")]

use std::thread;
use std::time::Instant;

use core_affinity::CoreId;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;

use order_book_flux::engine::OfiEngine;

fn pin_current_thread(core_id: CoreId) {
    let _ = core_affinity::set_for_current(core_id);
}

fn main() {
    let cores = core_affinity::get_core_ids().unwrap_or_default();
    let processing_core = cores.first().copied();
    let producer_core = cores.get(1).copied().or(processing_core);

    if let Some(core) = processing_core {
        pin_current_thread(core);
    }

    let rb = HeapRb::<Vec<u8>>::new(1 << 14);
    let (mut producer, mut consumer) = rb.split();

    let producer_thread = thread::spawn(move || {
        if let Some(core) = producer_core {
            pin_current_thread(core);
        }

        for i in 0..1_000_000u64 {
            let mut packet = if i % 2 == 0 {
                br#"{"symbol":"XBTUSD","side":"ask","price":50001,"qty":8,"ts_nanos":2}"#.to_vec()
            } else {
                br#"{"symbol":"XBTUSD","side":"bid","price":50000,"qty":10,"ts_nanos":1}"#.to_vec()
            };

            loop {
                match producer.try_push(packet) {
                    Ok(()) => break,
                    Err(returned_packet) => {
                        packet = returned_packet;
                        core::hint::spin_loop();
                    }
                }
            }
        }
    });

    let mut engine = OfiEngine::default();
    let mut processed = 0u64;
    let mut parse_errors = 0u64;
    let start = Instant::now();

    while processed < 1_000_000 {
        if let Some(packet) = consumer.try_pop() {
            match engine.process_packet(&packet) {
                Ok(_) => processed += 1,
                Err(e) => {
                    parse_errors += 1;
                    if parse_errors <= 5 {
                        eprintln!("Parse error #{}: {}", parse_errors, e);
                    }
                }
            }
        } else {
            core::hint::spin_loop();
        }
    }

    let elapsed = start.elapsed();
    let ns_per_tick = (elapsed.as_nanos() as f64) / (processed as f64);

    println!("Processed {} ticks", processed);
    println!("Parse errors: {}", parse_errors);
    println!("Latest OFI signal: {}", engine.latest_signal());
    println!("Top-5 imbalance: {}", engine.top5_snapshot_imbalance());
    println!("Tick-to-signal latency: {:.2} ns/tick", ns_per_tick);

    let _ = producer_thread.join();
}
