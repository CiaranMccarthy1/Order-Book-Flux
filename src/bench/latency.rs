#![cfg(feature = "std")]

use std::env;
use std::cell::Cell;

#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{_mm_lfence, _rdtsc};

// 4.7 GHz
const CPU_FREQ_HZ: f64 = 4_700_000_000.0;

use order_book_flux::connections::{apply_binance_payload, ExchangeConfig};
use order_book_flux::engine::OfiEngine;
use order_book_flux::types::Side;
use serde::Deserialize;
use tungstenite::{connect, Message};
use url::Url;

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub use_binance: bool,
    pub iterations: u64,
    pub warmup: u64,
}

impl RunConfig {
    fn from_args() -> Result<Self, String> {
        let mut config = RunConfig {
            use_binance: false,
            iterations: 100_000,
            warmup: 10_000,
        };

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--binance" => config.use_binance = true,
                "--iterations" => {
                    config.iterations = parse_u64("--iterations", args.next())?;
                }
                "--warmup" => {
                    config.warmup = parse_u64("--warmup", args.next())?;
                }
                "--help" | "-h" => return Err(help_text()),
                _ => return Err(format!("Unknown argument: {}\n{}", arg, help_text())),
            }
        }

        if config.iterations == 0 {
            return Err("iterations must be > 0".to_string());
        }
        if config.warmup > config.iterations {
            config.warmup = config.iterations;
        }

        Ok(config)
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = match RunConfig::from_args() {
        Ok(cfg) => cfg,
        Err(message) => {
            println!("{}", message);
            return Ok(());
        }
    };

    if config.use_binance {
        run_binance(&config)
    } else {
        run_synthetic(&config)
    }
}

#[derive(Debug, Deserialize)]
struct BinanceSnapshot {
    #[serde(rename = "lastUpdateId")]
    last_update_id: u64,
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

fn run_binance(config: &RunConfig) -> Result<(), Box<dyn std::error::Error>> {
    let exchange = ExchangeConfig::default();
    let snapshot = fetch_snapshot(&exchange.symbol)?;

    let mut engine = OfiEngine::new();
    let mut last_update_id = snapshot.last_update_id;

    for level in snapshot.bids {
        let price = parse_decimal_to_u64(&level[0], exchange.price_scale)?;
        let qty = parse_decimal_to_u64(&level[1], exchange.size_scale)?;
        engine.process_level_update(Side::Bid, price, qty);
    }

    for level in snapshot.asks {
        let price = parse_decimal_to_u64(&level[0], exchange.price_scale)?;
        let qty = parse_decimal_to_u64(&level[1], exchange.size_scale)?;
        engine.process_level_update(Side::Ask, price, qty);
    }

    let latencies = stream_binance_latencies(
        &exchange,
        &mut engine,
        &mut last_update_id,
        config.warmup,
        config.iterations,
    )?;

    print_stats("Binance", &latencies);
    Ok(())
}

fn run_synthetic(config: &RunConfig) -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = OfiEngine::new();
    let mut rng = Lcg::new(0x1234_5678_9abc_def0);

    for _ in 0..config.warmup {
        let (side, price, qty) = random_update(&mut rng);
        engine.process_level_update(side, price, qty);
    }

    let mut latencies_cycles = Vec::with_capacity(config.iterations as usize);
    for _ in 0..config.iterations {
        let (side, price, qty) = random_update(&mut rng);
        let start_cycles = unsafe {
            _mm_lfence();
            _rdtsc()
        };
        engine.process_level_update(side, price, qty);
        let end_cycles = unsafe {
            _mm_lfence();
            _rdtsc()
        };
        latencies_cycles.push(end_cycles - start_cycles);
    }

    let latencies_ns: Vec<u64> = latencies_cycles
        .into_iter()
        .map(|cycles| ((cycles as f64 / CPU_FREQ_HZ) * 1_000_000_000.0) as u64)
        .collect();

    print_stats("Synthetic", &latencies_ns);
    Ok(())
}

fn fetch_snapshot(symbol: &str) -> Result<BinanceSnapshot, Box<dyn std::error::Error>> {
    let url = format!(
        "https://api.binance.com/api/v3/depth?symbol={}&limit=1000",
        symbol
    );
    let client = reqwest::blocking::Client::builder()
        .user_agent("order-book-flux")
        .build()?;
    let response = client.get(url).send()?.error_for_status()?;
    let snapshot = response.json::<BinanceSnapshot>()?;
    Ok(snapshot)
}

fn stream_binance_latencies(
    config: &ExchangeConfig,
    engine: &mut OfiEngine,
    last_update_id: &mut u64,
    warmup: u64,
    iterations: u64,
) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
    let url = Url::parse(&config.url)?;
    let (mut socket, _response) = connect(url)?;

    let mut latencies_cycles = Vec::with_capacity(iterations as usize);
    let mut seen = 0u64;
    let done = Cell::new(false);

    let mut handler = |side: Side, price: u64, qty: u64| {
        if done.get() {
            return false;
        }

        let start_cycles = unsafe {
            _mm_lfence();
            _rdtsc()
        };
        engine.process_level_update(side, price, qty);
        let end_cycles = unsafe {
            _mm_lfence();
            _rdtsc()
        };

        if seen >= warmup {
            latencies_cycles.push(end_cycles - start_cycles);
            if latencies_cycles.len() as u64 >= iterations {
                done.set(true);
                return false;
            }
        }

        seen = seen.saturating_add(1);
        true
    };

    while !done.get() {
        match socket.read_message()? {
            Message::Text(text) => {
                apply_binance_payload(config, text.as_bytes(), last_update_id, &mut handler)?;
            }
            Message::Binary(bytes) => {
                apply_binance_payload(config, &bytes, last_update_id, &mut handler)?;
            }
            Message::Ping(payload) => {
                socket.send(Message::Pong(payload))?;
            }
            Message::Pong(_) => {}
            Message::Close(_) => break,
            _ => {}
        }
    }

    let latencies_ns: Vec<u64> = latencies_cycles
        .into_iter()
        .map(|cycles| ((cycles as f64 / CPU_FREQ_HZ) * 1_000_000_000.0) as u64)
        .collect();

    Ok(latencies_ns)
}

fn random_update(rng: &mut Lcg) -> (Side, u64, u64) {
    let side = if rng.next_u64() & 1 == 0 { Side::Bid } else { Side::Ask };
    let price = 50_000 + (rng.next_u64() % 200);
    let qty = 1 + (rng.next_u64() % 50);
    (side, price, qty)
}

fn print_stats(label: &str, latencies: &[u64]) {
    if latencies.is_empty() {
        println!("{}: no samples", label);
        return;
    }

    let filtered: Vec<u64> = latencies.iter().copied().filter(|v| *v > 0).collect();
    if filtered.is_empty() {
        println!("{}: all samples were zero", label);
        return;
    }

    let stats = compute_stats(&filtered);
    println!("{} latency", label);
    println!("Mean: {:.2} ns", stats.mean);
    println!("P50: {:.2} ns", stats.p50);
    println!("P95: {:.2} ns", stats.p95);
    println!("P99: {:.2} ns", stats.p99);
    println!("Min: {} ns", stats.min);
    println!("Max: {} ns", stats.max);
}

#[derive(Debug, Clone)]
struct Stats {
    mean: f64,
    p50: f64,
    p95: f64,
    p99: f64,
    min: u64,
    max: u64,
}

fn compute_stats(values: &[u64]) -> Stats {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();

    let count = sorted.len() as f64;
    let sum: f64 = sorted.iter().map(|v| *v as f64).sum();
    let mean = if count > 0.0 { sum / count } else { 0.0 };

    let min = *sorted.first().unwrap_or(&0);
    let max = *sorted.last().unwrap_or(&0);

    Stats {
        mean,
        p50: percentile(&sorted, 50.0),
        p95: percentile(&sorted, 95.0),
        p99: percentile(&sorted, 99.0),
        min,
        max,
    }
}

fn percentile(sorted: &[u64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (sorted.len().saturating_sub(1) as f64)).round() as usize;
    sorted[idx] as f64
}

fn parse_u64(flag: &str, value: Option<String>) -> Result<u64, String> {
    let raw = value.ok_or_else(|| format!("{} expects a value", flag))?;
    raw.parse::<u64>()
        .map_err(|_| format!("{} expects a number", flag))
}

fn help_text() -> String {
    let text = r#"Order-Book-Flux latency benchmark

Options:
    --binance          Use Binance WebSocket benchmark
  --iterations N     Number of timed iterations (default: 100000)
  --warmup N         Warmup iterations (default: 10000)
  -h, --help         Show this help
"#;
    text.to_string()
}

#[derive(Debug)]
enum ParseDecimalError {
    InvalidFormat,
    Overflow,
}

impl std::fmt::Display for ParseDecimalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseDecimalError::InvalidFormat => write!(f, "invalid format"),
            ParseDecimalError::Overflow => write!(f, "overflow"),
        }
    }
}

impl std::error::Error for ParseDecimalError {}

fn parse_decimal_to_u64(input: &str, scale: u32) -> Result<u64, ParseDecimalError> {
    let mut int_part = 0u64;
    let mut frac_part = 0u64;
    let mut frac_digits = 0u32;
    let mut seen_dot = false;
    let mut saw_digit = false;

    for byte in input.bytes() {
        match byte {
            b'0'..=b'9' => {
                let digit = (byte - b'0') as u64;
                saw_digit = true;

                if seen_dot {
                    if frac_digits < scale {
                        frac_part = frac_part
                            .checked_mul(10)
                            .and_then(|v| v.checked_add(digit))
                            .ok_or(ParseDecimalError::Overflow)?;
                        frac_digits += 1;
                    }
                } else {
                    int_part = int_part
                        .checked_mul(10)
                        .and_then(|v| v.checked_add(digit))
                        .ok_or(ParseDecimalError::Overflow)?;
                }
            }
            b'.' => {
                if seen_dot {
                    return Err(ParseDecimalError::InvalidFormat);
                }
                seen_dot = true;
            }
            _ => return Err(ParseDecimalError::InvalidFormat),
        }
    }

    if !saw_digit {
        return Err(ParseDecimalError::InvalidFormat);
    }

    if frac_digits < scale {
        let padding = scale - frac_digits;
        let pad = pow10(padding)?;
        frac_part = frac_part.checked_mul(pad).ok_or(ParseDecimalError::Overflow)?;
    }

    let scale_factor = pow10(scale)?;
    let scaled_int = int_part
        .checked_mul(scale_factor)
        .ok_or(ParseDecimalError::Overflow)?;

    scaled_int
        .checked_add(frac_part)
        .ok_or(ParseDecimalError::Overflow)
}

fn pow10(scale: u32) -> Result<u64, ParseDecimalError> {
    let mut value = 1u64;
    for _ in 0..scale {
        value = value.checked_mul(10).ok_or(ParseDecimalError::Overflow)?;
    }
    Ok(value)
}

struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }
}