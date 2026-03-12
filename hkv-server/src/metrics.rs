//! # Server Metrics
//!
//! Provide lightweight counters and a latency histogram to compute
//! QPS, error rate, and tail latency for the user-space server.
//!
//! ## Design Principles
//! 1. **Accumulator Pattern**: Use atomic counters to aggregate events cheaply.
//! 2. **Fixed Buckets**: Keep histogram buckets in a contiguous array for cache locality.
//! 3. **Zero-Cost Access**: Expose snapshots as plain structs without heap work.
//! 4. **FFI-Free**: Pure Rust types keep the hot path safe and portable.
//!
//! ## Notes
//! - Metrics are intentionally decoupled from the request path to keep the
//!   server fast; wiring and sampling policy are left to the caller.
//! - Bucket boundaries are expressed in microseconds and can be tuned later.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Default latency bucket boundaries in microseconds.
///
/// These are coarse on purpose to keep bucket scans short (performance-first).
pub const DEFAULT_LATENCY_BUCKETS_US: [u64; 12] =
    [1, 2, 5, 10, 20, 50, 100, 200, 500, 1_000, 2_000, 5_000];

/// Snapshot of all server metrics at a point in time.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    /// Total number of requests observed.
    pub requests_total: u64,
    /// Total number of error responses observed.
    pub errors_total: u64,
    /// Current in-flight requests.
    pub inflight: u64,
    /// Time since the metrics instance was created.
    pub uptime: Duration,
    /// Latency histogram snapshot.
    pub latency: LatencySnapshot,
}

/// Snapshot of the latency histogram.
#[derive(Debug, Clone)]
pub struct LatencySnapshot {
    /// Bucket boundaries in microseconds.
    pub bounds_us: Vec<u64>,
    /// Bucket counts, including the overflow bucket at the end.
    pub buckets: Vec<u64>,
    /// Total number of samples.
    pub samples: u64,
    /// Sum of latencies in microseconds.
    pub sum_us: u64,
    /// Maximum observed latency in microseconds.
    pub max_us: u64,
}

/// Thread-safe metrics aggregator for the server.
///
/// The struct is intentionally small and uses `AtomicU64` so record calls are
/// zero-allocation and cheap. `Ordering::Relaxed` is sufficient because we do
/// not require cross-field ordering, only eventual consistency.
pub struct Metrics {
    requests_total: AtomicU64,
    errors_total: AtomicU64,
    inflight: AtomicU64,
    latency: LatencyHistogram,
    started_at: Instant,
}

impl Metrics {
    /// Creates a new metrics aggregator with the default latency buckets.
    pub fn new() -> Self {
        Metrics {
            requests_total: AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
            inflight: AtomicU64::new(0),
            latency: LatencyHistogram::new(DEFAULT_LATENCY_BUCKETS_US.to_vec()),
            started_at: Instant::now(),
        }
    }

    /// Creates a new metrics aggregator with custom latency bucket boundaries.
    ///
    /// The boundaries must be sorted ascending and represent microseconds.
    ///
    /// **Input**: `bounds_us` (ascending microsecond thresholds).
    /// **Output**: a `Metrics` instance configured with those buckets.
    pub fn with_latency_buckets(bounds_us: Vec<u64>) -> Self {
        Metrics {
            requests_total: AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
            inflight: AtomicU64::new(0),
            latency: LatencyHistogram::new(bounds_us),
            started_at: Instant::now(),
        }
    }

    /// Records the start of a request.
    ///
    /// Call this when a request is accepted to increment totals and in-flight.
    pub fn record_request_start(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        self.inflight.fetch_add(1, Ordering::Relaxed);
    }

    /// Records the end of a request.
    ///
    /// Call this on completion to decrement in-flight and capture latency.
    pub fn record_request_end(&self, latency: Duration) {
        self.inflight.fetch_sub(1, Ordering::Relaxed);
        self.latency.record(latency);
    }

    /// Records an error response.
    pub fn record_error(&self) {
        self.errors_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns a snapshot of all counters and histogram buckets.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            errors_total: self.errors_total.load(Ordering::Relaxed),
            inflight: self.inflight.load(Ordering::Relaxed),
            uptime: self.started_at.elapsed(),
            latency: self.latency.snapshot(),
        }
    }
}

impl MetricsSnapshot {
    /// Returns the average queries per second since the metrics instance started.
    pub fn qps(&self) -> f64 {
        let uptime_secs = self.uptime.as_secs_f64();
        if uptime_secs <= f64::EPSILON {
            self.requests_total as f64
        } else {
            self.requests_total as f64 / uptime_secs
        }
    }

    /// Returns the fraction of requests that produced an error response.
    pub fn error_rate(&self) -> f64 {
        if self.requests_total == 0 {
            0.0
        } else {
            self.errors_total as f64 / self.requests_total as f64
        }
    }
}

/// Fixed-bucket latency histogram.
///
/// Uses a linear scan to pick buckets; this is O(buckets) but the list is small
/// and stays hot in cache. If you need faster bucket selection, replace with a
/// binary search or precomputed lookup table.
pub struct LatencyHistogram {
    bounds_us: Vec<u64>,
    buckets: Vec<AtomicU64>,
    sum_us: AtomicU64,
    samples: AtomicU64,
    max_us: AtomicU64,
}

impl LatencyHistogram {
    /// Creates a histogram with explicit bucket boundaries (microseconds).
    pub fn new(bounds_us: Vec<u64>) -> Self {
        let mut buckets = Vec::with_capacity(bounds_us.len() + 1);
        for _ in 0..=bounds_us.len() {
            buckets.push(AtomicU64::new(0));
        }

        LatencyHistogram {
            bounds_us,
            buckets,
            sum_us: AtomicU64::new(0),
            samples: AtomicU64::new(0),
            max_us: AtomicU64::new(0),
        }
    }

    /// Records a latency measurement into the histogram.
    ///
    /// Caller passes `Duration` to avoid unit ambiguity.
    pub fn record(&self, latency: Duration) {
        let micros = latency.as_micros() as u64;
        self.samples.fetch_add(1, Ordering::Relaxed);
        self.sum_us.fetch_add(micros, Ordering::Relaxed);
        update_max(&self.max_us, micros);

        let mut bucket_idx = self.bounds_us.len();
        for (i, &bound) in self.bounds_us.iter().enumerate() {
            if micros <= bound {
                bucket_idx = i;
                break;
            }
        }
        self.buckets[bucket_idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Returns a point-in-time snapshot of the histogram.
    pub fn snapshot(&self) -> LatencySnapshot {
        let buckets: Vec<u64> = self
            .buckets
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .collect();

        LatencySnapshot {
            bounds_us: self.bounds_us.clone(),
            buckets,
            samples: self.samples.load(Ordering::Relaxed),
            sum_us: self.sum_us.load(Ordering::Relaxed),
            max_us: self.max_us.load(Ordering::Relaxed),
        }
    }
}

impl LatencySnapshot {
    /// Returns the arithmetic mean latency in microseconds.
    pub fn average_us(&self) -> Option<f64> {
        if self.samples == 0 {
            None
        } else {
            Some(self.sum_us as f64 / self.samples as f64)
        }
    }

    /// Returns the histogram upper bound that satisfies the requested percentile.
    pub fn percentile_us(&self, percentile: f64) -> Option<u64> {
        if self.samples == 0 || !(0.0..=100.0).contains(&percentile) || percentile == 0.0 {
            return None;
        }

        let rank = ((self.samples as f64) * (percentile / 100.0)).ceil() as u64;
        let target = rank.max(1);
        let mut cumulative = 0u64;

        for (idx, count) in self.buckets.iter().copied().enumerate() {
            cumulative = cumulative.saturating_add(count);
            if cumulative < target {
                continue;
            }

            return if idx < self.bounds_us.len() {
                Some(self.bounds_us[idx])
            } else {
                Some(self.max_us)
            };
        }

        Some(self.max_us)
    }
}

fn update_max(max: &AtomicU64, value: u64) {
    let mut current = max.load(Ordering::Relaxed);
    while value > current {
        match max.compare_exchange(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_computes_percentiles_average_and_error_rate() {
        let metrics = Metrics::with_latency_buckets(vec![10, 20, 50, 100]);

        metrics.record_request_start();
        metrics.record_request_end(Duration::from_micros(9));
        metrics.record_request_start();
        metrics.record_request_end(Duration::from_micros(12));
        metrics.record_request_start();
        metrics.record_request_end(Duration::from_micros(80));
        metrics.record_error();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.requests_total, 3);
        assert_eq!(snapshot.errors_total, 1);
        assert_eq!(snapshot.inflight, 0);
        assert_eq!(snapshot.latency.samples, 3);
        assert_eq!(snapshot.latency.percentile_us(50.0), Some(20));
        assert_eq!(snapshot.latency.percentile_us(90.0), Some(100));
        assert_eq!(snapshot.latency.max_us, 80);
        assert_eq!(snapshot.latency.average_us(), Some(33.666_666_666_666_664));
        assert!((snapshot.error_rate() - (1.0 / 3.0)).abs() < 1e-12);
        assert!(snapshot.qps() >= 0.0);
    }

    #[test]
    fn percentile_returns_none_without_samples() {
        let histogram = LatencyHistogram::new(vec![10, 20, 50]);
        let snapshot = histogram.snapshot();
        assert_eq!(snapshot.average_us(), None);
        assert_eq!(snapshot.percentile_us(50.0), None);
    }
}
