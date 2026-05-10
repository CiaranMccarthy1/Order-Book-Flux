# Order-Book-Flux

High-performance Order Flow Imbalance (OFI) engine in Rust, designed around low-latency market microstructure processing.

## What this project does

Order-Book-Flux ingests market data updates, maintains a limit order book, and emits OFI deltas and an accumulated signal for the top 5 levels of depth.

Core goals:

- Lock-free producer/consumer ingestion path
- Deterministic price-level updates using ordered maps
- No mutexes on the critical data path
- no_std-compatible core library design where practical

## Architecture

Source layout:

- src/book.rs: Limit order book state and level update mechanics
- src/engine.rs: OFI algorithm and signal accumulation
- src/pool.rs: Preallocated order object pool used by the processing engine
- src/types.rs: Message and domain types, including zero-copy borrowed JSON fields
- src/main.rs: High-speed runtime loop with SPSC queue + CPU pinning
- benches/tick_to_signal.rs: Criterion benchmark for tick-to-signal latency

## Data structures and concurrency model

- Limit Order Book: BTreeMap for bids and asks, with cached best bid and best ask
- Queue: ringbuf lock-free SPSC channel
- Threading: producer thread and processing thread with optional core affinity pinning
- Synchronization: atomics only for signal read/write; no std::sync::Mutex on the hot path

## OFI model

The engine computes delta contributions from level changes and best-price events over top-of-book depth behavior.

Implemented components:

- Size increments/decrements at existing price levels
- Cancellations via quantity reductions to lower size or zero
- Best bid / best ask shifts when price priority changes
- Snapshot helper for top-5 imbalance: sum(bid top 5 qty) - sum(ask top 5 qty)

## Zero-copy deserialization

Incoming JSON packets are parsed from byte slices with borrowed fields for symbol text.

- Message type uses borrowed string slices for symbol
- Parsing operates on &[u8] payloads
- Avoids allocating new strings on the parse path

## no_std compatibility

Library modules are built to support no_std + alloc mode.

- Default feature set enables std runtime pieces
- Core engine/book/pool/types compile with no default features

Check no_std-compatible build:

```powershell
cargo check --no-default-features
```

## Build and run

Standard build:

```powershell
cargo check
```

Run the high-speed loop:

```powershell
cargo run --release
```

The runtime prints:

- processed tick count
- latest OFI signal
- approximate nanoseconds per tick

## Benchmark

Compile benchmark target:

```powershell
cargo bench --bench tick_to_signal --no-run
```

Run benchmark:

```powershell
cargo bench --bench tick_to_signal
```

Criterion reports throughput and timing statistics for the tick-to-signal path.

## Latency benchmark harness

The latency harness runs N independent runs and records per-iteration latency. Each run:

- scrubs caches
- seeds the order book (allocations occur before timing)
- performs a warmup phase (default 10% of iterations)
- captures per-iteration latency on the hot path only

Run the harness:

```powershell
cargo run --release --bin benchmark -- --runs 50 --iterations 1000000 --pin-core 0
```

Key options:

- `--runs N` or env `BENCH_RUNS`
- `--iterations M` or env `BENCH_ITERATIONS`
- `--warmup K`
- `--pin-core ID`
- `--use-rdtsc` (x86_64 only, requires stable CPU MHz)
- `--max-acceptable-ns N` or env `BENCH_MAX_ACCEPTABLE_NS` (discard samples above N ns for clean stats)

Output:

- Raw CSV: `./benchmark_data/<timestamp>/raw.csv`
- Summary JSON: `./benchmark_data/<timestamp>/summary.json`

Example output (good, low-noise run):

```
Runs: 20
Iterations per run: 1000000
Mean: 132.45 ns
Median: 131.92 ns
StdDev: 6.12 ns
Min: 120 ns
Max: 210 ns
P50: 131.92 ns
P90: 139.80 ns
P95: 145.20 ns
P99: 160.10 ns
P999: 190.00 ns
Outliers > 10x median: 0
```

Clean vs outlier analysis:

- Samples above `--max-acceptable-ns` are excluded from the clean distribution
- Outlier distribution stats are printed separately for jitter analysis

Example output (noisy run with jitter):

```
Runs: 20
Iterations per run: 1000000
Mean: 210.30 ns
Median: 135.10 ns
StdDev: 80.50 ns
Min: 120 ns
Max: 3500 ns
P50: 135.10 ns
P90: 190.00 ns
P95: 230.00 ns
P99: 800.00 ns
P999: 2400.00 ns
Outliers > 10x median: 1250
```

Notes:

- CPU frequency locking attempts are best-effort; on Linux this requires sudo/root.
- Thread migration and CPU frequency changes are reported as warnings.

## Notes on performance tuning

- Prefer running with release mode and CPU frequency scaling disabled when benchmarking
- Pin producer and consumer to distinct physical cores when available
- Keep message formats stable to reduce branch variability in parsing
- Replace JSON with a binary schema for lower parsing overhead in production

## License

MIT (see LICENSE)
