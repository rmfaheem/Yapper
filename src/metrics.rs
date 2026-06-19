use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Shared, thread-safe counters updated by flood workers and read by the TUI
/// dashboard / CLI reporter.
#[derive(Debug)]
pub struct Metrics {
    ops: AtomicU64,
    errors: AtomicU64,
    bytes: AtomicU64,
    active: AtomicBool,
    /// Recent operation latencies in microseconds, drained on each snapshot.
    latencies_us: Mutex<Vec<u32>>,
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics {
            ops: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            active: AtomicBool::new(false),
            latencies_us: Mutex::new(Vec::new()),
        }
    }
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Metrics::default())
    }

    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Relaxed);
    }

    /// Record one successful operation that took `latency_us` microseconds and
    /// transferred `bytes` bytes.
    pub fn record_op(&self, latency_us: u32, bytes: u64) {
        self.ops.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        if let Ok(mut lat) = self.latencies_us.lock() {
            // Cap the buffer so a long-running flood can't grow it unbounded
            // between snapshots.
            if lat.len() < 100_000 {
                lat.push(latency_us);
            }
        }
    }

    pub fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn total_ops(&self) -> u64 {
        self.ops.load(Ordering::Relaxed)
    }

    pub fn total_errors(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }

    pub fn total_bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// Drain the latency buffer and return `(p50_ms, p99_ms)` over the drained
    /// window. Returns `(0.0, 0.0)` when no samples are pending.
    pub fn drain_latency_percentiles(&self) -> (f64, f64) {
        let mut samples = match self.latencies_us.lock() {
            Ok(mut lat) => std::mem::take(&mut *lat),
            Err(_) => return (0.0, 0.0),
        };
        if samples.is_empty() {
            return (0.0, 0.0);
        }
        samples.sort_unstable();
        let pick = |q: f64| -> f64 {
            let idx = ((samples.len() as f64 - 1.0) * q).round() as usize;
            samples[idx] as f64 / 1000.0
        };
        (pick(0.50), pick(0.99))
    }

    /// Reset all counters (used when starting a fresh flood from the TUI).
    pub fn reset(&self) {
        self.ops.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
        self.bytes.store(0, Ordering::Relaxed);
        if let Ok(mut lat) = self.latencies_us.lock() {
            lat.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_ops_bytes_and_errors() {
        let m = Metrics::new();
        m.record_op(1000, 50);
        m.record_op(2000, 50);
        m.record_error();
        assert_eq!(m.total_ops(), 2);
        assert_eq!(m.total_bytes(), 100);
        assert_eq!(m.total_errors(), 1);
    }

    #[test]
    fn percentiles_then_drain_to_zero() {
        let m = Metrics::new();
        // 10..=50 microseconds.
        for v in [10u32, 20, 30, 40, 50] {
            m.record_op(v, 0);
        }
        let (p50, p99) = m.drain_latency_percentiles();
        // p50 -> index round(4*0.5)=2 -> 30us -> 0.03ms; p99 -> index 4 -> 50us -> 0.05ms.
        assert!((p50 - 0.030).abs() < 1e-9, "p50 was {p50}");
        assert!((p99 - 0.050).abs() < 1e-9, "p99 was {p99}");

        // The buffer was drained, so the next call sees no samples.
        let (p50b, p99b) = m.drain_latency_percentiles();
        assert!(p50b.abs() < 1e-12 && p99b.abs() < 1e-12);
    }

    #[test]
    fn reset_clears_all_counters() {
        let m = Metrics::new();
        m.record_op(5, 5);
        m.record_error();
        m.reset();
        assert_eq!(m.total_ops(), 0);
        assert_eq!(m.total_errors(), 0);
        assert_eq!(m.total_bytes(), 0);
        let (p50, p99) = m.drain_latency_percentiles();
        assert!(p50.abs() < 1e-12 && p99.abs() < 1e-12);
    }
}
