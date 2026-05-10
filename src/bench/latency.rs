#![cfg(feature = "std")]

use std::env;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use order_book_flux::engine::OfiEngine;
use order_book_flux::types::Side;
use serde_json::json;
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct BenchmarkConfig {
    pub runs: usize,
    pub iterations: usize,
    pub warmup: usize,
    pub pin_core: Option<usize>,
    pub use_rdtsc: bool,
    pub seed_levels: usize,
    pub output_root: PathBuf,
    pub cache_scrub_bytes: usize,
    pub max_acceptable_ns: u64,
}

impl BenchmarkConfig {
    pub fn from_env_and_args() -> Result<Self, String> {
        let mut config = BenchmarkConfig::default();
        let mut args = env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--runs" => {
                    config.runs = parse_usize("--runs", args.next())?;
                }
                "--iterations" => {
                    config.iterations = parse_usize("--iterations", args.next())?;
                }
                "--warmup" => {
                    config.warmup = parse_usize("--warmup", args.next())?;
                }
                "--pin-core" => {
                    config.pin_core = Some(parse_usize("--pin-core", args.next())?);
                }
                "--use-rdtsc" => {
                    config.use_rdtsc = true;
                }
                "--seed-levels" => {
                    config.seed_levels = parse_usize("--seed-levels", args.next())?;
                }
                "--output-dir" => {
                    config.output_root = PathBuf::from(parse_string("--output-dir", args.next())?);
                }
                "--cache-scrub-mb" => {
                    let mb = parse_usize("--cache-scrub-mb", args.next())?;
                    config.cache_scrub_bytes = mb.saturating_mul(1024 * 1024);
                }
                "--max-acceptable-ns" => {
                    config.max_acceptable_ns = parse_u64("--max-acceptable-ns", args.next())?;
                }
                "--help" | "-h" => {
                    return Err(help_text());
                }
                _ => {
                    return Err(format!("Unknown argument: {}\n{}", arg, help_text()));
                }
            }
        }

        if let Ok(runs) = env::var("BENCH_RUNS") {
            config.runs = runs.parse().map_err(|_| "BENCH_RUNS must be a number".to_string())?;
        }
        if let Ok(iterations) = env::var("BENCH_ITERATIONS") {
            config.iterations = iterations
                .parse()
                .map_err(|_| "BENCH_ITERATIONS must be a number".to_string())?;
        }
        if let Ok(max_ns) = env::var("BENCH_MAX_ACCEPTABLE_NS") {
            config.max_acceptable_ns = max_ns
                .parse()
                .map_err(|_| "BENCH_MAX_ACCEPTABLE_NS must be a number".to_string())?;
        }

        if config.warmup == 0 {
            config.warmup = (config.iterations / 10).max(1);
        }

        if config.runs == 0 {
            return Err("runs must be > 0".to_string());
        }
        if config.iterations == 0 {
            return Err("iterations must be > 0".to_string());
        }
        if config.seed_levels == 0 {
            return Err("seed-levels must be > 0".to_string());
        }

        Ok(config)
    }
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            runs: 10,
            iterations: 1_000_000,
            warmup: 0,
            pin_core: None,
            use_rdtsc: false,
            seed_levels: 256,
            output_root: PathBuf::from("benchmark_data"),
            cache_scrub_bytes: 64 * 1024 * 1024,
            max_acceptable_ns: u64::MAX,
        }
    }
}

#[derive(Debug, Copy, Clone)]
struct UpdateOp {
    side: Side,
    price: u64,
    qty: u64,
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = match BenchmarkConfig::from_env_and_args() {
        Ok(cfg) => cfg,
        Err(message) => {
            println!("{}", message);
            return Ok(());
        }
    };

    let output_dir = config.output_root.join(timestamp_dir()?);
    fs::create_dir_all(&output_dir)?;

    let raw_path = output_dir.join("raw.csv");
    let summary_path = output_dir.join("summary.json");

    let mut writer = BufWriter::new(File::create(&raw_path)?);
    writeln!(writer, "run_id,iteration_ns")?;

    let total = config
        .runs
        .checked_mul(config.iterations)
        .ok_or("runs * iterations overflow")?;
    let mut latencies = Vec::with_capacity(total);
    let mut clean_latencies = Vec::with_capacity(total);
    let mut outlier_latencies = Vec::new();
    let mut outliers_over_10x = 0usize;
    let mut warnings = Vec::new();

    #[cfg(target_arch = "x86_64")]
    let requested_rdtsc = config.use_rdtsc;

    #[cfg(not(target_arch = "x86_64"))]
    let requested_rdtsc = {
        if config.use_rdtsc {
            warnings.push("rdtsc not supported on this architecture; using Instant".to_string());
        }
        false
    };

    let rdtsc_hz = if requested_rdtsc {
        rdtsc_frequency_hz().or_else(|| freq_hz_from_mhz(cpu_frequency_mhz()))
    } else {
        None
    };

    let use_rdtsc = requested_rdtsc && rdtsc_hz.is_some();
    if requested_rdtsc && rdtsc_hz.is_none() {
        warnings.push("rdtsc requested but CPU frequency unavailable; falling back to Instant".to_string());
        if cpu_frequency_mhz().is_none() {
            warnings.push("CPU frequency read not available on this platform".to_string());
        }
    }

    if let Some(message) = boost_thread_priority() {
        warnings.push(message);
    }

    if let Some(core) = config.pin_core {
        if let Err(message) = pin_current_thread(core) {
            warnings.push(message);
        }
    }

    if let Some(message) = attempt_lock_cpu_frequency() {
        warnings.push(message);
    }

    for run_id in 0..config.runs {
        if let Some(core) = config.pin_core {
            if let Err(message) = pin_current_thread(core) {
                warnings.push(message);
            }
        }

        scrub_cache(config.cache_scrub_bytes);

        let mut engine = OfiEngine::new();
        seed_book(&mut engine, config.seed_levels);

        let ops = build_ops(config.seed_levels, config.warmup + config.iterations);

        let start_cpu = current_cpu();
        let freq_before = cpu_frequency_mhz();

        for i in 0..config.warmup {
            let op = ops[i];
            engine.process_level_update(op.side, op.price, op.qty);
        }

        if use_rdtsc {
            let freq_hz = rdtsc_hz.unwrap_or(0.0);
            for i in 0..config.iterations {
                let op = ops[config.warmup + i];
                let start = rdtsc_serialized();
                engine.process_level_update(op.side, op.price, op.qty);
                let end = rdtsc_serialized();
                let ns = cycles_to_ns(end.saturating_sub(start), freq_hz);
                record_latency(
                    ns,
                    config.max_acceptable_ns,
                    &mut latencies,
                    &mut clean_latencies,
                    &mut outlier_latencies,
                );
                writeln!(writer, "{},{}", run_id, ns)?;
            }
        } else {
            measure_with_instant(
                &mut engine,
                &ops[config.warmup..],
                run_id,
                config.max_acceptable_ns,
                &mut latencies,
                &mut clean_latencies,
                &mut outlier_latencies,
                &mut writer,
            )?;
        }

        let end_cpu = current_cpu();
        if start_cpu.is_some() && end_cpu.is_some() && start_cpu != end_cpu {
            warnings.push(format!(
                "Run {}: thread migrated from CPU {:?} to {:?}",
                run_id, start_cpu, end_cpu
            ));
        }

        let freq_after = cpu_frequency_mhz();
        if freq_changed(freq_before, freq_after) {
            warnings.push(format!(
                "Run {}: CPU frequency changed from {:?} MHz to {:?} MHz",
                run_id, freq_before, freq_after
            ));
        }
    }

    writer.flush()?;

    let stats_all = compute_stats(&latencies);
    let stats_clean = compute_stats(&clean_latencies);
    let stats_outliers = compute_stats(&outlier_latencies);
    if clean_latencies.is_empty() {
        warnings.push("No clean samples; max_acceptable_ns may be too low".to_string());
    }
    if stats_all.median > 0.0 {
        outliers_over_10x = latencies
            .iter()
            .filter(|value| (**value as f64) > stats_all.median * 10.0)
            .count();
    }

    print_summary(
        &config,
        &stats_clean,
        &stats_outliers,
        latencies.len(),
        clean_latencies.len(),
        outlier_latencies.len(),
        outliers_over_10x,
        &raw_path,
        &summary_path,
        &warnings,
    );

    let summary_json = json!({
        "runs": config.runs,
        "iterations_per_run": config.iterations,
        "max_acceptable_ns": config.max_acceptable_ns,
        "total_samples": latencies.len(),
        "clean_samples": clean_latencies.len(),
        "outlier_samples": outlier_latencies.len(),
        "mean": stats_clean.mean,
        "median": stats_clean.median,
        "stddev": stats_clean.stddev,
        "min": stats_clean.min,
        "max": stats_clean.max,
        "p50": stats_clean.p50,
        "p90": stats_clean.p90,
        "p95": stats_clean.p95,
        "p99": stats_clean.p99,
        "p999": stats_clean.p999,
        "outliers_over_10x_median": outliers_over_10x,
        "outliers": {
            "mean": stats_outliers.mean,
            "median": stats_outliers.median,
            "stddev": stats_outliers.stddev,
            "min": stats_outliers.min,
            "max": stats_outliers.max,
            "p50": stats_outliers.p50,
            "p90": stats_outliers.p90,
            "p95": stats_outliers.p95,
            "p99": stats_outliers.p99,
            "p999": stats_outliers.p999,
        },
        "raw_csv": raw_path.display().to_string(),
        "summary_json": summary_path.display().to_string(),
        "warnings": warnings,
    });

    let mut summary_writer = BufWriter::new(File::create(&summary_path)?);
    writeln!(summary_writer, "{}", serde_json::to_string_pretty(&summary_json)?)?;
    summary_writer.flush()?;

    Ok(())
}

fn help_text() -> String {
    let text = r#"Order-Book-Flux latency benchmark

Options:
  --runs N             Number of independent runs (env: BENCH_RUNS)
  --iterations M       Iterations per run (env: BENCH_ITERATIONS)
  --warmup K           Warmup iterations per run (default: 10% of M)
  --pin-core ID        Pin benchmark thread to core ID
  --use-rdtsc          Use rdtsc timing on x86_64 (requires stable CPU MHz)
  --seed-levels N      Pre-seeded price levels per side
  --cache-scrub-mb N   Cache scrub size in MB per run
    --max-acceptable-ns N  Discard iterations above N ns (env: BENCH_MAX_ACCEPTABLE_NS)
  --output-dir PATH    Output root directory (default: ./benchmark_data)
  -h, --help           Show this help
"#;
    text.to_string()
}

fn parse_usize(flag: &str, value: Option<String>) -> Result<usize, String> {
    let raw = value.ok_or_else(|| format!("{} expects a value", flag))?;
    raw.parse::<usize>()
        .map_err(|_| format!("{} expects a number", flag))
}

fn parse_u64(flag: &str, value: Option<String>) -> Result<u64, String> {
    let raw = value.ok_or_else(|| format!("{} expects a value", flag))?;
    raw.parse::<u64>()
        .map_err(|_| format!("{} expects a number", flag))
}

fn parse_string(flag: &str, value: Option<String>) -> Result<String, String> {
    value.ok_or_else(|| format!("{} expects a value", flag))
}

fn pin_current_thread(core_index: usize) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let group_count = unsafe { GetActiveProcessorGroupCount() } as usize;
        if group_count == 0 {
            return Err("No processor groups available".to_string());
        }

        let mut remaining = core_index;
        let mut target_group: u16 = 0;
        let mut target_number: u8 = 0;
        let mut found = false;

        for group in 0..group_count {
            let count = unsafe { GetActiveProcessorCount(group as u16) } as usize;
            if remaining < count {
                target_group = group as u16;
                target_number = remaining as u8;
                found = true;
                break;
            }
            remaining -= count;
        }

        if !found {
            return Err(format!("Core {} not found", core_index));
        }

        let mask = 1u64 << (target_number as u64);
        let affinity = GROUP_AFFINITY {
            Mask: mask,
            Group: target_group,
            Reserved: [0; 3],
        };

        let current = unsafe { GetCurrentThread() };
        let mut previous = GROUP_AFFINITY {
            Mask: 0,
            Group: 0,
            Reserved: [0; 3],
        };

        let result = unsafe { SetThreadGroupAffinity(current, &affinity, &mut previous) };
        if result == 0 {
            return Err("SetThreadGroupAffinity failed".to_string());
        }

        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let cores = core_affinity::get_core_ids().ok_or("No CPU cores detected")?;
        let core = cores
            .into_iter()
            .find(|core| core.id == core_index)
            .ok_or_else(|| format!("Core {} not found", core_index))?;
        core_affinity::set_for_current(core);
        Ok(())
    }
}

fn scrub_cache(bytes: usize) {
    let mut buffer = vec![0u8; bytes];
    let mut acc = 0u64;
    for byte in buffer.iter_mut() {
        *byte = byte.wrapping_add(1);
        acc = acc.wrapping_add(*byte as u64);
    }
    if acc == u64::MAX {
        println!("cache scrub checksum: {}", acc);
    }
}

fn seed_book(engine: &mut OfiEngine, levels: usize) {
    let base_bid = 100_000u64;
    let base_ask = 100_100u64;
    let qty = 100u64;

    for i in 0..levels {
        engine.process_level_update(Side::Bid, base_bid - i as u64, qty);
        engine.process_level_update(Side::Ask, base_ask + i as u64, qty);
    }
}

fn build_ops(levels: usize, total: usize) -> Vec<UpdateOp> {
    let mut ops = Vec::with_capacity(total);
    let base_bid = 100_000u64;
    let base_ask = 100_100u64;

    for i in 0..total {
        let idx = (i % levels) as u64;
        let qty = 50 + ((i % 10) as u64) * 5;
        if i % 2 == 0 {
            ops.push(UpdateOp {
                side: Side::Bid,
                price: base_bid - idx,
                qty,
            });
        } else {
            ops.push(UpdateOp {
                side: Side::Ask,
                price: base_ask + idx,
                qty,
            });
        }
    }

    ops
}

fn record_latency(
    ns: u64,
    max_acceptable_ns: u64,
    latencies: &mut Vec<u64>,
    clean_latencies: &mut Vec<u64>,
    outlier_latencies: &mut Vec<u64>,
) {
    latencies.push(ns);
    if ns > max_acceptable_ns {
        outlier_latencies.push(ns);
    } else {
        clean_latencies.push(ns);
    }
}

fn measure_with_instant(
    engine: &mut OfiEngine,
    ops: &[UpdateOp],
    run_id: usize,
    max_acceptable_ns: u64,
    latencies: &mut Vec<u64>,
    clean_latencies: &mut Vec<u64>,
    outlier_latencies: &mut Vec<u64>,
    writer: &mut BufWriter<File>,
) -> io::Result<()> {
    for op in ops.iter() {
        let start = Instant::now();
        engine.process_level_update(op.side, op.price, op.qty);
        let elapsed = start.elapsed();
        let ns = elapsed.as_nanos() as u64;
        record_latency(
            ns,
            max_acceptable_ns,
            latencies,
            clean_latencies,
            outlier_latencies,
        );
        writeln!(writer, "{},{}", run_id, ns)?;
    }
    Ok(())
}

fn timestamp_dir() -> Result<String, Box<dyn std::error::Error>> {
    let format = time::format_description::parse("[year]-[month]-[day]_[hour][minute][second]")?;
    let now = OffsetDateTime::now_utc();
    Ok(now.format(&format)?)
}

#[derive(Debug, Clone)]
struct Stats {
    mean: f64,
    median: f64,
    stddev: f64,
    min: u64,
    max: u64,
    p50: f64,
    p90: f64,
    p95: f64,
    p99: f64,
    p999: f64,
}

fn compute_stats(values: &[u64]) -> Stats {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();

    let count = sorted.len() as f64;
    let sum: f64 = sorted.iter().map(|v| *v as f64).sum();
    let mean = if count > 0.0 { sum / count } else { 0.0 };

    let median = percentile(&sorted, 50.0);
    let p50 = median;
    let p90 = percentile(&sorted, 90.0);
    let p95 = percentile(&sorted, 95.0);
    let p99 = percentile(&sorted, 99.0);
    let p999 = percentile(&sorted, 99.9);

    let variance = if count > 0.0 {
        sorted
            .iter()
            .map(|v| {
                let diff = *v as f64 - mean;
                diff * diff
            })
            .sum::<f64>()
            / count
    } else {
        0.0
    };

    Stats {
        mean,
        median,
        stddev: variance.sqrt(),
        min: *sorted.first().unwrap_or(&0),
        max: *sorted.last().unwrap_or(&0),
        p50,
        p90,
        p95,
        p99,
        p999,
    }
}

fn percentile(sorted: &[u64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let clamped = p.clamp(0.0, 100.0);
    let idx = ((clamped / 100.0) * (sorted.len().saturating_sub(1) as f64)).round() as usize;
    sorted[idx] as f64
}

fn print_summary(
    config: &BenchmarkConfig,
    clean_stats: &Stats,
    outlier_stats: &Stats,
    total_samples: usize,
    clean_samples: usize,
    outlier_samples: usize,
    outliers_over_10x: usize,
    raw_path: &Path,
    summary_path: &Path,
    warnings: &[String],
) {
    println!("Runs: {}", config.runs);
    println!("Iterations per run: {}", config.iterations);
    println!("Max acceptable ns: {}", config.max_acceptable_ns);
    println!(
        "Samples: {} total, {} clean, {} outliers",
        total_samples, clean_samples, outlier_samples
    );
    println!("Clean distribution:");
    println!("Mean: {:.2} ns", clean_stats.mean);
    println!("Median: {:.2} ns", clean_stats.median);
    println!("StdDev: {:.2} ns", clean_stats.stddev);
    println!("Min: {} ns", clean_stats.min);
    println!("Max: {} ns", clean_stats.max);
    println!("P50: {:.2} ns", clean_stats.p50);
    println!("P90: {:.2} ns", clean_stats.p90);
    println!("P95: {:.2} ns", clean_stats.p95);
    println!("P99: {:.2} ns", clean_stats.p99);
    println!("P999: {:.2} ns", clean_stats.p999);
    println!("Outliers > 10x median: {}", outliers_over_10x);
    println!("Outlier distribution (> max_acceptable_ns):");
    if outlier_samples == 0 {
        println!("(none)");
    } else {
        println!("Mean: {:.2} ns", outlier_stats.mean);
        println!("Median: {:.2} ns", outlier_stats.median);
        println!("StdDev: {:.2} ns", outlier_stats.stddev);
        println!("Min: {} ns", outlier_stats.min);
        println!("Max: {} ns", outlier_stats.max);
        println!("P50: {:.2} ns", outlier_stats.p50);
        println!("P90: {:.2} ns", outlier_stats.p90);
        println!("P95: {:.2} ns", outlier_stats.p95);
        println!("P99: {:.2} ns", outlier_stats.p99);
        println!("P999: {:.2} ns", outlier_stats.p999);
    }
    println!("Raw data: {}", raw_path.display());
    println!("Summary: {}", summary_path.display());

    if !warnings.is_empty() {
        println!("Warnings:");
        for warning in warnings {
            println!("- {}", warning);
        }
    }
}

fn cpu_frequency_mhz() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        read_cpuinfo_mhz().ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn read_cpuinfo_mhz() -> io::Result<f64> {
    let data = fs::read_to_string("/proc/cpuinfo")?;
    let mut mhz_values = Vec::new();

    for line in data.lines() {
        if let Some(rest) = line.strip_prefix("cpu MHz") {
            if let Some(value) = rest.split(':').nth(1) {
                if let Ok(parsed) = value.trim().parse::<f64>() {
                    mhz_values.push(parsed);
                }
            }
        }
    }

    if mhz_values.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "cpu MHz not found"));
    }

    let sum: f64 = mhz_values.iter().sum();
    Ok(sum / mhz_values.len() as f64)
}

fn freq_changed(before: Option<f64>, after: Option<f64>) -> bool {
    match (before, after) {
        (Some(a), Some(b)) => (a - b).abs() > 1.0,
        _ => false,
    }
}

fn freq_hz_from_mhz(freq_mhz: Option<f64>) -> Option<f64> {
    freq_mhz.map(|mhz| mhz * 1_000_000.0)
}

#[cfg(target_os = "windows")]
fn rdtsc_frequency_hz() -> Option<f64> {
    let mut freq = LARGE_INTEGER { QuadPart: 0 };
    let ok = unsafe { QueryPerformanceFrequency(&mut freq as *mut LARGE_INTEGER) };
    if ok == 0 || freq.QuadPart <= 0 {
        return None;
    }

    let qpc_freq = freq.QuadPart as f64;
    let target_ticks = (qpc_freq * 0.05) as i64;
    let target_ticks = target_ticks.max(1);

    let start_qpc = query_performance_counter()?;
    let start_tsc = rdtsc_serialized();

    loop {
        let now = query_performance_counter()?;
        if now - start_qpc >= target_ticks {
            break;
        }
        core::hint::spin_loop();
    }

    let end_tsc = rdtsc_serialized();
    let end_qpc = query_performance_counter()?;

    let qpc_delta = (end_qpc - start_qpc) as f64;
    if qpc_delta <= 0.0 {
        return None;
    }

    let tsc_delta = end_tsc.saturating_sub(start_tsc) as f64;
    Some(tsc_delta * qpc_freq / qpc_delta)
}

#[cfg(not(target_os = "windows"))]
fn rdtsc_frequency_hz() -> Option<f64> {
    None
}

#[cfg(target_arch = "x86_64")]
fn rdtsc_serialized() -> u64 {
    unsafe {
        std::arch::x86_64::_mm_lfence();
        let value = std::arch::x86_64::_rdtsc();
        std::arch::x86_64::_mm_lfence();
        value
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn rdtsc_serialized() -> u64 {
    0
}

fn cycles_to_ns(cycles: u64, freq_hz: f64) -> u64 {
    if freq_hz == 0.0 {
        return 0;
    }
    let seconds = cycles as f64 / freq_hz;
    (seconds * 1_000_000_000.0).round() as u64
}

fn attempt_lock_cpu_frequency() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let cpu_root = Path::new("/sys/devices/system/cpu");
        if !cpu_root.exists() {
            return Some("CPU frequency controls not available on this system".to_string());
        }

        let mut failed = false;
        let entries = match fs::read_dir(cpu_root) {
            Ok(entries) => entries,
            Err(_) => {
                return Some("Unable to read CPU frequency controls (requires sudo/root)".to_string());
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    failed = true;
                    continue;
                }
            };
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with("cpu") {
                continue;
            }
            let path = entry.path().join("cpufreq").join("scaling_governor");
            if path.exists() {
                if fs::write(&path, "performance").is_err() {
                    failed = true;
                }
            }
        }

        if failed {
            return Some("Unable to set CPU governor to performance (requires sudo/root)".to_string());
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        Some("CPU frequency lock not implemented on this platform".to_string())
    }
}

fn current_cpu() -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        let cpu = unsafe { libc::sched_getcpu() };
        if cpu >= 0 {
            Some(cpu as u32)
        } else {
            None
        }
    }
    #[cfg(target_os = "windows")]
    {
        let mut processor = PROCESSOR_NUMBER {
            Group: 0,
            Number: 0,
            Reserved: 0,
        };

        unsafe {
            GetCurrentProcessorNumberEx(&mut processor as *mut PROCESSOR_NUMBER);
        }

        Some(((processor.Group as u32) << 16) | (processor.Number as u32))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
fn query_performance_counter() -> Option<i64> {
    let mut value = LARGE_INTEGER { QuadPart: 0 };
    let ok = unsafe { QueryPerformanceCounter(&mut value as *mut LARGE_INTEGER) };
    if ok == 0 {
        None
    } else {
        Some(value.QuadPart)
    }
}

fn boost_thread_priority() -> Option<String> {
    #[cfg(target_os = "windows")]
    unsafe {
        let process = GetCurrentProcess();
        if SetPriorityClass(process, HIGH_PRIORITY_CLASS) == 0 {
            return Some("Failed to set process priority class".to_string());
        }

        let thread = GetCurrentThread();
        if SetThreadPriority(thread, THREAD_PRIORITY_HIGHEST) == 0 {
            return Some("Failed to set thread priority".to_string());
        }

        None
    }

    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct GROUP_AFFINITY {
    Mask: u64,
    Group: u16,
    Reserved: [u16; 3],
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct PROCESSOR_NUMBER {
    Group: u16,
    Number: u8,
    Reserved: u8,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct LARGE_INTEGER {
    QuadPart: i64,
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> isize;
    fn GetActiveProcessorGroupCount() -> u16;
    fn GetActiveProcessorCount(group: u16) -> u32;
    fn GetCurrentThread() -> isize;
    fn SetThreadGroupAffinity(
        thread: isize,
        group_affinity: *const GROUP_AFFINITY,
        previous_affinity: *mut GROUP_AFFINITY,
    ) -> i32;
    fn GetCurrentProcessorNumberEx(processor_number: *mut PROCESSOR_NUMBER);
    fn SetPriorityClass(process: isize, priority_class: u32) -> i32;
    fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    fn QueryPerformanceCounter(counter: *mut LARGE_INTEGER) -> i32;
    fn QueryPerformanceFrequency(frequency: *mut LARGE_INTEGER) -> i32;
}

#[cfg(target_os = "windows")]
const HIGH_PRIORITY_CLASS: u32 = 0x00000080;

#[cfg(target_os = "windows")]
const THREAD_PRIORITY_HIGHEST: i32 = 2;
