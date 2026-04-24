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

> **[BENCHMARK NEEDED]** Run `cargo bench --bench tick_to_signal` with CPU frequency scaling disabled (`cpupower frequency-set --governor performance` on Linux) and producer/consumer pinned to distinct physical cores. Report:
> - Median tick-to-signal latency (ns)
> - p99 latency (ns)
> - Throughput (ticks/second)
> - JSON parse path vs. binary schema drop-in, to quantify serialization overhead as a fraction of total latency
> - Baseline comparison: mutex-protected equivalent book implementation

The runtime loop prints approximate nanoseconds-per-tick during live execution, but Criterion results from `benches/tick_to_signal.rs` are the authoritative numbers.

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
