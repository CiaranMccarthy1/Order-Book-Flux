use criterion::{black_box, criterion_group, criterion_main, Criterion};
use order_book_flux::connections::{apply_binance_payload, ExchangeConfig};
use order_book_flux::engine::OfiEngine;

fn bench_binance_payload(c: &mut Criterion) {
    let config = ExchangeConfig {
        symbol: "BTCUSDT".to_string(),
        price_scale: 2,
        size_scale: 4,
        url: "wss://stream.binance.com:9443/ws/btcusdt@depth".to_string(),
        rest_url: "https://api.binance.com".to_string(),
        stream: "btcusdt@depth".to_string(),
        origin: "".to_string(),
    };

    let diff_one = br#"{"e":"depthUpdate","E":1,"s":"BTCUSDT","U":1,"u":2,"b":[["50000.00","0.1000"]],"a":[["50001.00","0.2000"]]}"#;
    let diff_two = br#"{"e":"depthUpdate","E":2,"s":"BTCUSDT","U":3,"u":4,"b":[["50000.00","0.1500"]],"a":[["50001.00","0"]]}"#;
    let payloads: [&[u8]; 2] = [diff_one, diff_two];

    c.bench_function("binance_payload_to_engine", |b| {
        let mut engine = OfiEngine::default();
        let mut last_update_id = 0u64;
        b.iter(|| {
            for payload in payloads {
                let mut handler = |side, price, qty| {
                    engine.process_level_update(side, price, qty);
                    true
                };
                apply_binance_payload(&config, payload, &mut last_update_id, &mut handler).unwrap();
            }
            black_box(engine.latest_signal());
        });
    });
}

criterion_group!(benches, bench_binance_payload);
criterion_main!(benches);
