# Order-Book-Flux

Rust order book exploring lock-free concurrency and nanosecond-scale latency measurement. Built to understand how far you can push a single-threaded hot path before the OS gets in the way.

## What It Does

A single-producer, single-consumer pipeline: one thread ingests market data, another mutates a fixed-window, Vec-backed order book and computes order flow imbalance (OFI). The two threads communicate through an SPSC ring buffer. The core library is `no_std + alloc` compatible. The hot path avoids allocations when price levels are pre-seeded. The `std` binary now supports live Binance depth streaming.

The OFI engine classifies book events (arrivals, cancellations, best-price shifts) and accumulates a signed signal on the processing thread.

## Architecture

```
[Binance WebSocket] -> [Producer] --SPSC ringbuf--> [Consumer: book + OFI engine] -> [Signal]
```

- Producer: Binance WebSocket client parses JSON diffs and converts decimal strings into scaled integer prices and sizes.
- SPSC channel: `ringbuf` crate - single head/tail pointer loads, no CAS on enqueue.
- Book: fixed-window price ladder backed by `Vec`, with cached best bid/ask to skip scans on reads.
- Engine: Level-delta OFI model, updates the signal atomically.

## Performance

Measured on Linux, Ryzen 5 5600X, pinned core 0, 50 independent runs x 1M iterations each.

| Metric | Value |
| --- | --- |
| Mean | 15.28 ns |
| P50 | 15.00 ns |
| P95 | 18.00 ns |
| P99 | 23.00 ns |
| Min | 12 ns |
| Max | 26699 ns |

> synthetic tick_to_signal:
>
> ```
> tick_to_signal_ns       
> time: 
> Lower Bound 1.1805 us
> Point Estimate 1.2051 us
> Upper Bound 1.2334 us
> ```
>
> binance_payload_to_engine:
>
> ```
> binance_payload_to_engine
> time: 
> Lower Bound 2.0913 us 
> Point Estimate 2.0950 us
> Upper Bound 2.0994 us
> ```

Tail latency: occasional spikes up to 26.7 us (interrupts, scheduler jitter). These are excluded from clean distribution stats. See benchmark methodology below.

## Usage

```rust
// Processing thread
loop {
    if let Some(update) = consumer.try_pop() {
        engine.process_level_update(update.side, update.price, update.qty);
    } else {
        core::hint::spin_loop();
    }
}

// Read signal on the processing thread
let signal = engine.latest_signal();
let imbalance = engine.top5_snapshot_imbalance();
```

## Build

```bash
cargo check                          # debug
cargo check --no-default-features    # no_std verification
cargo run --release --bin order-book-flux   # live Binance feed
cargo bench --bench tick_to_signal   # Criterion benchmark
cargo bench --bench binance_feed     # Criterion benchmark (sample Binance payloads)
```

## Live Feed Notes

- `cargo run --release` connects to Binance depth via WebSocket and prints stats every 5 seconds.
- Default symbol is `BTCUSDT`. You can change it in `ExchangeConfig::default()` in the `std` binary.
- Default WebSocket URL is `wss://fstream.binance.com/public/ws/btcusdt@depth`.
- Prices and sizes are scaled into integers (default: price scale 2, size scale 8).

## Benchmarking

Custom harness:

```bash
cargo run --release --bin benchmark -- --iterations 1000000
```

Live Binance benchmark:

```bash
cargo run --release --bin benchmark -- --binance --iterations 100000 --warmup 10000
```

What it does:

- Single run with warmup iterations, then timing each update.
- Per-iteration timing: rdtsc with lfence serialization on x86_64.

Example output:

```
Synthetic latency
Mean: 15.28 ns
P50: 15.00 ns
P95: 18.00 ns
P99: 23.00 ns
Min: 12 ns
Max: 26699 ns
```

synthetic tick_to_signal:

```
tick_to_signal_ns       
time: 
Lower Bound 1.1805 us
Point Estimate 1.2051 us
Upper Bound 1.2334 us
```

binance_payload_to_engine:

```
binance_payload_to_engine
time: 
Lower Bound 2.0913 us 
Point Estimate 2.0950 us
Upper Bound 2.0994 us
```

Raw CSV + JSON summary saved to `benchmark_data/<timestamp>/`.

## Why the tail exists

Linux still delivers timer interrupts, scheduler ticks, and network softirqs that pause the thread between rdtsc reads. Even with thread affinity, the kernel still wins occasionally. The 12 ns to 26.7 us outliers are the OS, not your code.

For tighter tail consistency, isolate a core (`isolcpus` + `nohz_full`), pin IRQs away, and disable deep C-states.

## Tech Stack

Rust - ringbuf - Vec-backed price ladder - serde_json - no_std + alloc - rdtsc timing

## License

MIT (see LICENSE)
