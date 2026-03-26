//! Statistics collection for the TUI dashboard.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Shared statistics state for the TUI dashboard.
#[derive(Debug, Default)]
pub struct Stats {
    /// Total number of transactions planned to send.
    pub total_planned: AtomicU64,
    /// Total number of transactions confirmed on-chain (verified via nonce advancement).
    pub confirmed_count: AtomicU64,
    /// Number of transactions currently pending (nonce-window capped backlog).
    pub deferred_count: AtomicU64,
    /// When the broadcast started.
    pub start_time: std::sync::RwLock<Option<Instant>>,
    /// Duration of the most recent burst in milliseconds.
    pub last_burst_time_ms: AtomicU64,
    /// Number of transactions sent in the most recent burst.
    pub last_burst_sent: AtomicU64,
    /// Current gas price (in Gwei for display).
    pub current_gas_gwei: AtomicU64,
    /// Gas price override (in atomic units, 0 means no override).
    pub gas_price_override: AtomicU64,
    /// Gas limit override for S1 (in gas units, 0 means no override).
    pub gas_limit_override: AtomicU64,
    /// Gas limit override for S0/S2 cross-shard (in gas units, 0 means no override).
    pub gas_limit_override_cross: AtomicU64,
    /// Current batch size
    pub current_batch_size: AtomicU64,
    /// True if running in burn-all mode (continuous operation, no fixed target).
    pub burn_all_mode: std::sync::atomic::AtomicBool,
}

impl Stats {
    /// Create a new stats instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new stats instance wrapped in Arc for sharing.
    pub fn new_arc() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Set the total planned transactions.
    pub fn set_total_planned(&self, total: u64) {
        self.total_planned.store(total, Ordering::Relaxed);
    }

    /// Reduce the total planned count (called when pending txs are dropped due to insufficient balance).
    pub fn decrement_planned(&self, count: u64) {
        self.total_planned.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
            Some(cur.saturating_sub(count))
        }).ok();
    }

    /// Mark the broadcast as started.
    pub fn mark_started(&self) {
        let mut start = self.start_time.write().unwrap();
        *start = Some(Instant::now());
    }

    /// Increment the confirmed count.
    pub fn increment_confirmed(&self, count: u64) {
        self.confirmed_count.fetch_add(count, Ordering::Relaxed);
    }

    /// Set the pending count (nonce-window capped backlog).
    pub fn set_deferred(&self, count: u64) {
        self.deferred_count.store(count, Ordering::Relaxed);
    }

    /// Record burst timing.
    pub fn record_burst(&self, sent: usize, elapsed_secs: f64) {
        self.last_burst_sent.store(sent as u64, Ordering::Relaxed);
        self.last_burst_time_ms.store((elapsed_secs * 1000.0) as u64, Ordering::Relaxed);
    }

    /// Set current gas price (stored as Gwei * 1000 for precision).
    pub fn set_gas_price(&self, gas_gwei: u64) {
        self.current_gas_gwei.store(gas_gwei, Ordering::Relaxed);
    }

    /// Set gas price override in atomic units.
    pub fn set_gas_price_override(&self, gas_price: u64) {
        self.gas_price_override.store(gas_price, Ordering::Relaxed);
    }

    /// Get gas price override (0 means no override).
    pub fn get_gas_price_override(&self) -> u64 {
        self.gas_price_override.load(Ordering::Relaxed)
    }

    /// Clear gas price override.
    #[allow(dead_code)]
    pub fn clear_gas_price_override(&self) {
        self.gas_price_override.store(0, Ordering::Relaxed);
    }

    /// Set gas limit override for S1.
    pub fn set_gas_limit_override(&self, gas_limit: u64) {
        self.gas_limit_override.store(gas_limit, Ordering::Relaxed);
    }

    /// Get gas limit override for S1 (0 means no override).
    pub fn get_gas_limit_override(&self) -> u64 {
        self.gas_limit_override.load(Ordering::Relaxed)
    }

    /// Set gas limit override for S0/S2 cross-shard.
    pub fn set_gas_limit_override_cross(&self, gas_limit: u64) {
        self.gas_limit_override_cross.store(gas_limit, Ordering::Relaxed);
    }

    /// Get gas limit override for S0/S2 cross-shard (0 means no override).
    pub fn get_gas_limit_override_cross(&self) -> u64 {
        self.gas_limit_override_cross.load(Ordering::Relaxed)
    }

    /// Get current TPS (confirmed transactions per second).
    pub fn current_tps(&self) -> f64 {
        let confirmed = self.confirmed_count.load(Ordering::Relaxed);
        let elapsed = self.elapsed_secs();
        if elapsed > 0.0 {
            confirmed as f64 / elapsed
        } else {
            0.0
        }
    }

    /// Get elapsed time in seconds.
    pub fn elapsed_secs(&self) -> f64 {
        let start = self.start_time.read().unwrap();
        start.map_or(0.0, |s| s.elapsed().as_secs_f64())
    }

    /// Get elapsed time formatted as HH:MM:SS.
    #[allow(dead_code)]
    pub fn elapsed_formatted(&self) -> String {
        let secs = self.elapsed_secs() as u64;
        format_duration(secs)
    }

    /// Get estimated time to completion in seconds (based on confirmed vs planned).
    pub fn eta_secs(&self) -> u64 {
        let confirmed = self.confirmed_count.load(Ordering::Relaxed);
        let total = self.total_planned.load(Ordering::Relaxed);
        let tps = self.current_tps();

        if tps > 0.0 && confirmed < total {
            let remaining = total.saturating_sub(confirmed);
            (remaining as f64 / tps) as u64
        } else {
            0
        }
    }

    /// Get ETA formatted as HH:MM:SS.
    #[allow(dead_code)]
    pub fn eta_formatted(&self) -> String {
        format_duration(self.eta_secs())
    }

    /// Get progress percentage based on confirmed vs planned (0.0 to 100.0).
    pub fn progress_pct(&self) -> f64 {
        let confirmed = self.confirmed_count.load(Ordering::Relaxed);
        let total = self.total_planned.load(Ordering::Relaxed);

        if total > 0 {
            ((confirmed as f64 / total as f64) * 100.0).min(100.0)
        } else {
            0.0
        }
    }


    /// Set current batch size (AIMD-adjusted).
    pub fn set_batch_size(&self, size: usize) {
        self.current_batch_size.store(size as u64, Ordering::Relaxed);
    }

    /// Get current values snapshot for rendering.
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            total_planned: self.total_planned.load(Ordering::Relaxed),
            confirmed_count: self.confirmed_count.load(Ordering::Relaxed),
            deferred_count: self.deferred_count.load(Ordering::Relaxed),
            elapsed_secs: self.elapsed_secs(),
            current_tps: self.current_tps(),
            eta_secs: self.eta_secs(),
            progress_pct: self.progress_pct(),
            last_burst_time_ms: self.last_burst_time_ms.load(Ordering::Relaxed),
            last_burst_sent: self.last_burst_sent.load(Ordering::Relaxed),
            current_gas_gwei: self.current_gas_gwei.load(Ordering::Relaxed),
            gas_price_override: self.gas_price_override.load(Ordering::Relaxed),
            gas_limit_override: self.gas_limit_override.load(Ordering::Relaxed),
            gas_limit_override_cross: self.gas_limit_override_cross.load(Ordering::Relaxed),
            current_batch_size: self.current_batch_size.load(Ordering::Relaxed),
            burn_all_mode: self.burn_all_mode.load(Ordering::Relaxed),
        }
    }
}

/// A snapshot of stats at a point in time for rendering.
#[derive(Debug, Clone, Copy)]
pub struct StatsSnapshot {
    pub total_planned: u64,
    pub confirmed_count: u64,
    pub deferred_count: u64,
    pub elapsed_secs: f64,
    pub current_tps: f64,
    pub eta_secs: u64,
    pub progress_pct: f64,
    pub last_burst_time_ms: u64,
    pub last_burst_sent: u64,
    pub current_gas_gwei: u64,
    pub gas_price_override: u64,
    pub gas_limit_override: u64,
    pub gas_limit_override_cross: u64,
    pub current_batch_size: u64,
    pub burn_all_mode: bool,
}

/// Format seconds as HH:MM:SS.
#[allow(dead_code)]
fn format_duration(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    format!("{:02}:{:02}:{:02}", hours, mins, secs)
}
