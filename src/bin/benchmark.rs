#![cfg(feature = "std")]

#[path = "../bench/latency.rs"]
mod latency;

fn main() {
    if let Err(err) = latency::run() {
        eprintln!("Benchmark failed: {}", err);
    }
}
