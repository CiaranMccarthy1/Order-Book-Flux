use criterion::{black_box, criterion_group, criterion_main, Criterion};

use order_book_flux::connections::{apply_binance_payload, ExchangeConfig};
use order_book_flux::engine::OfiEngine;

fn bench_tick_to_signal(c: &mut Criterion) {
    let mut engine = OfiEngine::default();
    let mut last_update_id = 0u64;
    let config = ExchangeConfig {
        symbol: "BTCUSDT".to_string(),
        price_scale: 2,
        size_scale: 4,
        url: "wss://stream.binance.com:9443/ws/btcusdt@depth".to_string(),
        rest_url: "https://api.binance.com".to_string(),
        stream: "btcusdt@depth".to_string(),
        origin: "".to_string(),
    };
    let bid_packet = br#"{"e":"depthUpdate","E":1,"s":"BTCUSDT","U":1,"u":2,"b":[["50000.00","0.1000"]],"a":[]}"#;
    let ask_packet = br#"{"e":"depthUpdate","E":2,"s":"BTCUSDT","U":3,"u":4,"b":[],"a":[["50001.00","0.0900"]]}"#;

    c.bench_function("tick_to_signal_ns", |b| {
        b.iter(|| {
            let mut handler = |side, price, qty| {
                engine.process_level_update(side, price, qty);
                true
            };
            let _ = apply_binance_payload(&config, black_box(bid_packet), &mut last_update_id, &mut handler);
            let _ = apply_binance_payload(&config, black_box(ask_packet), &mut last_update_id, &mut handler);
        })
    });
}

criterion_group!(benches, bench_tick_to_signal);
criterion_main!(benches);
