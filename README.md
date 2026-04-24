# Order-Book-Flux

Lock-free Order Flow Imbalance engine in Rust processing L2 market data through a zero-allocation hot path to nanosecond-latency OFI signal emission.

---

## What Makes This Interesting

The engine separates book state mutation from signal accumulation across an SPSC ring buffer, keeping the critical path free of heap allocation, locks, and system calls. OFI delta computation handles the full taxonomy of book events — size increments, cancellations, and best-price shifts — rather than approximating from top-of-book snapshots alone. The core library compiles under `no_std + alloc`, making the design portable to FPGA softcore or kernel-bypass environments without architectural changes.

---

## Architecture

The runtime spawns two threads pinned to distinct physical cores: a producer thread that deserializes incoming JSON packets into typed `Message` values using borrowed `&[u8]` slices to avoid symbol string allocation, and a processing thread that owns the book and engine state exclusively. Communication between them runs through a `ringbuf` SPSC channel, chosen because single-producer single-consumer semantics eliminate the need for any atomic compare-and-swap on the enqueue path — only a head/tail pointer load is required.

The limit order book in `book.rs` uses a `BTreeMap` for each side, with cached best bid and best ask fields updated on every mutation to avoid a tree traversal on the signal read path. The OFI engine in `engine.rs` operates on level deltas — it receives a before/after diff per price level and classifies the event (arrival, cancellation, or aggression proxy via best-price shift) to compute a signed contribution. An accumulated signal is written via atomics, making it readable from an external thread without entering the hot path.

The object pool in `pool.rs` preallocates order structs at startup, removing per-tick heap allocation from the processing loop entirely.

---

## Performance / Results

The following results were captured using the Criterion suite on the `tick_to_signal` hot path. Benchmarks were executed with CPU frequency scaling set to `performance` and threads pinned to isolated physical cores.

### Execution Summary
* **Median Latency:** `360.09 ns`
* **Lower Bound (p95):** `352.42 ns`
* **Upper Bound (p95):** `371.61 ns`
* **Throughput:** ~2.77 million ticks/sec (theoretical maximum)

### Latency Distribution
The engine demonstrates high determinism with a significant performance improvement (~40%) over previous iterations, likely due to optimized cache locality and the removal of remaining heap allocations via `pool.rs`.

| Percentile | Latency |
| :--- | :--- |
| **p50 (Median)** | 360.09 ns |
| **p90** | < 370 ns |
| **p99 (Outliers)** | Observed "high severe" spikes (9% total outliers) |

> [!NOTE]
> **Outlier Analysis:** The 3 "high severe" outliers detected during the 100-measurement sample are characteristic of OS-level interrupts or context switching. Moving from the current Windows-based test environment to a Linux environment with `isolcpus` and a tickless kernel is expected to collapse these outliers and further stabilize the p99.

### Serialization Tax
The current `~360 ns` includes the cost of `serde_json` zero-copy parsing. Preliminary profiling suggests that moving to a binary schema (e.g., Simple Binary Encoding or FlatBuffers) could potentially reduce total latency by an additional 15-20%.

---

## Usage

```rust
// Processing thread: consume ticks from SPSC queue, update book, emit OFI signal
while let Some(msg) = consumer.pop() {
    engine.process(&mut book, msg);
}

// Read accumulated OFI signal from any thread — atomic load, no lock
let signal = engine.signal();

// Top-5 depth imbalance snapshot
let imbalance = book.top5_imbalance(); // sum(bid top-5 qty) - sum(ask top-5 qty)
```

---

## Tech Stack

Rust · `ringbuf` (SPSC) · `BTreeMap` · `Criterion` · `no_std + alloc` · core affinity pinning

---

## Build

```bash
# Debug check
cargo check

# Verified no_std-compatible build
cargo check --no-default-features

# Release binary (required for meaningful latency numbers)
cargo run --release

# Latency benchmark
cargo bench --bench tick_to_signal
```

> Benchmark with CPU frequency scaling disabled and cores pinned to distinct physical cores for reproducible results.

---

## License

MIT (see LICENSE)
