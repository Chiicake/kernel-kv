//! # Engine Benchmark Harness
//!
//! Purpose: Provide a dependency-free, repeatable benchmark driver for the
//! in-memory engine so baseline throughput and latency can be compared over time.
//!
//! ## Design Principles
//! 1. **Deterministic Workload**: Use a fixed PRNG seed for stable comparisons.
//! 2. **Allocation Control**: Pre-build keys/values to keep setup costs off the hot path.
//! 3. **Zero-Cost Dispatch**: Call the concrete engine directly to avoid dynamic dispatch.
//! 4. **Strategy Pattern Awareness**: The engine implements the `KVEngine` trait,
//!    enabling swappable backends without changing the harness.

use std::env;
use std::hint::black_box;
use std::time::Instant;

use hkv_common::HkvResult;
use hkv_engine::MemoryEngine;

const DEFAULT_KEY_COUNT: usize = 1 << 16;
const DEFAULT_OP_COUNT: usize = 1_000_000;
const DEFAULT_KEY_SIZE: usize = 16;
const DEFAULT_VALUE_SIZE: usize = 128;

struct BenchConfig {
    requested_keys: usize,
    key_count: usize,
    key_mask: usize,
    op_count: usize,
    key_size: usize,
    value_size: usize,
}

impl BenchConfig {
    fn from_args() -> Self {
        let mut args = env::args().skip(1);
        let requested_keys = parse_usize(args.next(), DEFAULT_KEY_COUNT);
        let op_count = parse_usize(args.next(), DEFAULT_OP_COUNT);
        let key_size = parse_usize(args.next(), DEFAULT_KEY_SIZE);
        let value_size = parse_usize(args.next(), DEFAULT_VALUE_SIZE);

        let key_count = normalize_power_of_two(requested_keys);
        let key_mask = key_count - 1;

        BenchConfig {
            requested_keys,
            key_count,
            key_mask,
            op_count,
            key_size,
            value_size,
        }
    }
}

fn parse_usize(value: Option<String>, fallback: usize) -> usize {
    value.and_then(|raw| raw.parse().ok()).unwrap_or(fallback)
}

fn normalize_power_of_two(value: usize) -> usize {
    let value = value.max(1);
    if value.is_power_of_two() {
        value
    } else {
        value.next_power_of_two()
    }
}

/// Tiny deterministic PRNG used to avoid external dependencies.
///
/// XorShift is fast enough for benchmarks and keeps the workload reproducible.
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    #[inline]
    fn next_index(&mut self, mask: usize) -> usize {
        (self.next_u64() as usize) & mask
    }
}

fn write_u64_le(value: u64, buffer: &mut [u8]) {
    let bytes = value.to_le_bytes();
    let copy_len = buffer.len().min(bytes.len());
    buffer[..copy_len].copy_from_slice(&bytes[..copy_len]);
}

fn build_buffers(count: usize, size: usize, seed: u64) -> Vec<Vec<u8>> {
    let mut buffers = Vec::with_capacity(count);
    for i in 0..count {
        let mut buffer = vec![0u8; size];
        write_u64_le(seed ^ (i as u64), &mut buffer);
        buffers.push(buffer);
    }
    buffers
}

fn report(label: &str, ops: usize, elapsed: std::time::Duration) {
    let secs = elapsed.as_secs_f64();
    let ops_per_sec = (ops as f64) / secs;
    let nanos_per_op = (secs * 1e9) / (ops as f64);
    println!(
        "{label}: {ops} ops in {secs:.3}s ({ops_per_sec:.0} ops/s, {nanos_per_op:.1} ns/op)"
    );
}

fn main() {
    if let Err(err) = run() {
        eprintln!("bench_engine failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> HkvResult<()> {
    let config = BenchConfig::from_args();
    let engine = MemoryEngine::new();

    let keys = build_buffers(config.key_count, config.key_size, 0xA5A5_A5A5_A5A5_A5A5);
    let values = build_buffers(config.key_count, config.value_size, 0x5A5A_5A5A_5A5A_5A5A);

    for idx in 0..config.key_count {
        engine.set(keys[idx].clone(), values[idx].clone())?;
    }

    println!(
        "keys: requested={}, actual={}, ops={}, key_size={}, value_size={}",
        config.requested_keys,
        config.key_count,
        config.op_count,
        config.key_size,
        config.value_size
    );

    let mut rng = XorShift64::new(0x1234_5678_9ABC_DEF0);
    let start = Instant::now();
    for _ in 0..config.op_count {
        let idx = rng.next_index(config.key_mask);
        let value = engine.get(&keys[idx])?;
        black_box(value);
    }
    report("GET", config.op_count, start.elapsed());

    let mut rng = XorShift64::new(0x0FED_CBA9_8765_4321);
    let start = Instant::now();
    for _ in 0..config.op_count {
        let idx = rng.next_index(config.key_mask);
        let mut value = values[idx].clone();
        if let Some(first) = value.get_mut(0) {
            *first ^= 0xFF;
        }
        engine.set(keys[idx].clone(), value)?;
    }
    report("SET", config.op_count, start.elapsed());

    Ok(())
}
