# Order-Book-Flux

A high-performance Binance order book with Order Flow Imbalance (OFI) signal computation.

**Key improvement:** Switched from sliding window (Vec) to `BTreeMap` for unbounded price levels. Eliminates recentering `.fill(0)` clears and data loss risk.

## Build

```bash
cargo build --lib                           # Library only (no_std)
cargo build --release --features std        # With Binance feed
```

## Usage

### Live Binance Stream
```bash
cargo run --release --features std
```

Output shows 5-second snapshots:
- **Update#**: Number of updates processed
- **OFI Signal**: Cumulative order flow imbalance
- **Signal Δ**: Change since last report
- **Top-5 Imb**: Bid-ask imbalance in top 5 levels
- **Spread**: Mid-market spread
- **Best Bid/Ask**: Current best prices

### In Code
```rust
use order_book_flux::engine::OfiEngine;
use order_book_flux::types::Side;

let mut engine = OfiEngine::new();
let delta = engine.process_level_update(Side::Bid, 50000, 100);
let imb = engine.top5_snapshot_imbalance();
```

## Architecture

| Module | Purpose |
|--------|---------|
| `book.rs` | BTreeMap order book (bids/asks) |
| `engine.rs` | OFI signal computation |
| `connections.rs` | Binance WebSocket + REST |
| `types.rs` | Price, Quantity, Side types |
| `main.rs` | Live dashboard |

## Testing

```bash
cargo test --lib book       # Order book tests
cargo test --lib engine     # OFI signal tests
cargo test                  # Full suite (47 tests)
```

All tests pass. Includes regression test: `no_data_loss_across_price_ranges`.

## Benchmarking

```bash
cargo bench --bench tick_to_signal    # Single update: ~1.7 µs
cargo bench --bench binance_feed         # Typical payload: ~2.1 µs per level
```

No recentering spikes. Smooth O(log N) latency profile.

## Performance

| Metric | Value |
|--------|-------|
| Per-update latency | 1–2 µs |
| Typical book size | 5k bids + 5k asks |
| Memory per level | 8 bytes |
| Throughput | 10k+ updates/sec |



## Configuration

Edit `ExchangeConfig::default()` in `src/connections.rs`:
- Default WebSocket: `wss://stream.binance.com:9443/ws/btcusdt@depth`
- Default REST: `https://api.binance.com`
- Price scale: 2 (cents), Size scale: 8 (satoshis)


## License

MIT