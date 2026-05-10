# Order-Book-Flux

Rust order book exploring lock-free concurrency and nanosecond-scale latency measurement. Built to understand how far you can push a single-threaded hot path before the OS gets in the way.

## What It Does

A single-producer, single-consumer pipeline: one thread ingests market data, another mutates a BTreeMap-backed order book and computes order flow imbalance (OFI). The two threads communicate through an SPSC ring buffer. The core library is `no_std + alloc` compatible. The hot path avoids allocations when price levels are pre-seeded.

The OFI engine classifies book events (arrivals, cancellations, best-price shifts) and accumulates a signed signal readable via atomic load from any thread.

## Architecture

```
[Network/JSON] -> [Producer] --SPSC ringbuf--> [Consumer: book + OFI engine] -> [Signal]
```

- Producer: Deserializes JSON into `&[u8]`-borrowed message structs, avoiding per-symbol string allocation.
- SPSC channel: `ringbuf` crate - single head/tail pointer loads, no CAS on enqueue.
- Book: `BTreeMap` per side with cached best bid/ask to skip tree traversal on reads.
- Engine: Level-delta OFI model, updates the signal atomically.

## Performance

Measured on Windows 11, Ryzen 5 5600X, pinned core 0, 50 independent runs x 1M iterations each.

| Metric | Value |
| --- | --- |
| Median | 47 ns |
| P90 | 59 ns |
| P95 | 62 ns |
| P99 | 72 ns |
| P999 | 109 ns |

Tail latency: 0.001% of samples exceed 5 us (OS interrupts, scheduler jitter). These are excluded from clean distribution stats. See benchmark methodology below.

> [!NOTE]
> These are single-threaded, synthetic measurements - best case, no network, no serialization, no contention. Real market data adds parse time, cache pressure, and branch mispredicts. The 47 ns is an upper bound on what is possible.

## Usage

```rust
// Processing thread
while let Some(packet) = consumer.pop() {
    engine.process_packet(&packet)?;
}

// Read signal from anywhere - atomic load, no lock
let signal = engine.latest_signal();
let imbalance = engine.top5_snapshot_imbalance();
```

## Build

```bash
cargo check                          # debug
cargo check --no-default-features    # no_std verification
cargo run --release                  # release binary
cargo bench --bench tick_to_signal   # Criterion benchmark
```

## Benchmarking

Custom harness for statistical rigor:

```powershell
cargo run --release --bin benchmark -- --runs 50 --iterations 1000000 --pin-core 0 --max-acceptable-ns 5000 --use-rdtsc
```

What it does:

- N independent runs: each scrubs caches, re-seeds the book, warms up separately.
- Per-iteration timing: rdtsc with lfence serialization on x86_64, falls back to Instant.
- Outlier rejection: samples above `--max-acceptable-ns` excluded from clean distribution.
- Migration detection: warns if the thread moves cores mid-run.
- Frequency monitoring: warns if CPU MHz changes (when available).

Example output (clean run):

```
Runs: 50
Iterations per run: 1000000
Max acceptable ns: 5000
Samples: 50000000 total, 49999403 clean, 597 outliers
Clean distribution:
Mean: 48.34 ns
Median: 47.00 ns
StdDev: 21.50 ns
P50: 47.00 ns
P90: 59.00 ns
P95: 62.00 ns
P99: 72.00 ns
P999: 109.00 ns
Outliers > 10x median: 597
Outlier distribution (> max_acceptable_ns):
Mean: 32492.00 ns
Median: 14458.00 ns
Max: 383589 ns
```

Raw CSV + JSON summary saved to `benchmark_data/<timestamp>/`.

## Why the tail exists

Windows delivers DPCs, timer interrupts, and scheduler ticks that pause the thread between rdtsc reads. Even with HIGH_PRIORITY_CLASS and thread affinity, the kernel still wins occasionally. The 5-383 us outliers are the OS, not your code.

For sub-us tail consistency, move to Linux with `isolcpus` + `nohz_full` + disabled C-states.

## What I Learned

- Measurement is harder than the code. Building the benchmark harness took longer than the engine.
- Windows is a bad HFT platform. The 0.001% tail is acceptable for learning, but production latency work needs bare metal Linux.
- `BTreeMap` is surprisingly fast. For 256 price levels, cache-resident traversal is about 50 ns. For deeper books or hotter paths, a flat array or radix tree likely wins.
- Serialization dominates in real pipelines. The microbenchmark excludes JSON parsing; moving to SBE or FlatBuffers would reduce end-to-end latency significantly.

## Tech Stack

Rust - ringbuf - BTreeMap - serde_json - no_std + alloc - rdtsc timing

## License

MIT (see LICENSE)
