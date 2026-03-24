use criterion::{black_box, criterion_group, criterion_main, Criterion};

use order_book_flux::engine::OfiEngine;

fn bench_tick_to_signal(c: &mut Criterion) {
    let mut engine = OfiEngine::default();
    let bid_packet = br#"{"symbol":"XBTUSD","side":"bid","price":50000,"qty":10,"ts_nanos":100}"#;
    let ask_packet = br#"{"symbol":"XBTUSD","side":"ask","price":50001,"qty":9,"ts_nanos":101}"#;

    c.bench_function("tick_to_signal_ns", |b| {
        b.iter(|| {
            let _ = engine.process_packet(black_box(bid_packet));
            let _ = engine.process_packet(black_box(ask_packet));
        })
    });
}

criterion_group!(benches, bench_tick_to_signal);
criterion_main!(benches);
