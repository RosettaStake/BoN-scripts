//! Application state for the TUI dashboard.

use crate::blockchain::MIN_GAS_PRICE;
use crate::tui::stats::{Stats, StatsSnapshot};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The TUI application state.
pub struct App {
    /// Shared statistics collector.
    pub stats: Arc<Stats>,
    /// Ring buffer of log messages for the log pane.
    pub logs: Arc<Mutex<VecDeque<String>>>,
    /// Maximum number of log lines to keep.
    max_log_lines: usize,
    /// Whether the app should quit.
    pub should_quit: AtomicBool,
    /// Whether a restart was requested (press 'r').
    pub restart_requested: AtomicBool,
    /// Title to display in the header.
    pub title: String,
    /// Current gas price input buffer (when in gas input mode).
    pub gas_input_buffer: Arc<Mutex<String>>,
    /// Whether we're currently in gas price input mode.
    pub in_gas_input_mode: AtomicBool,
    /// Current gas limit input buffer (when in gas limit input mode).
    pub gas_limit_input_buffer: Arc<Mutex<String>>,
    /// Whether we're currently in gas limit input mode (S1).
    pub in_gas_limit_input_mode: AtomicBool,
    /// Current cross-shard gas limit input buffer.
    pub gas_limit_cross_input_buffer: Arc<Mutex<String>>,
    /// Whether we're currently in cross-shard gas limit input mode.
    pub in_gas_limit_cross_input_mode: AtomicBool,
}

impl App {
    /// Create a new app instance with the given title.
    pub fn new(title: impl Into<String>) -> Self {
        let logs = Arc::new(Mutex::new(VecDeque::with_capacity(100)));

        Self {
            stats: Stats::new_arc(),
            logs,
            max_log_lines: 100,
            should_quit: AtomicBool::new(false),
            restart_requested: AtomicBool::new(false),
            title: title.into(),
            gas_input_buffer: Arc::new(Mutex::new(String::new())),
            in_gas_input_mode: AtomicBool::new(false),
            gas_limit_input_buffer: Arc::new(Mutex::new(String::new())),
            in_gas_limit_input_mode: AtomicBool::new(false),
            gas_limit_cross_input_buffer: Arc::new(Mutex::new(String::new())),
            in_gas_limit_cross_input_mode: AtomicBool::new(false),
        }
    }

    /// Create a new app with an existing stats collector.
    #[allow(dead_code)]
    pub fn with_stats(title: impl Into<String>, stats: Arc<Stats>) -> Self {
        let logs = Arc::new(Mutex::new(VecDeque::with_capacity(100)));

        Self {
            stats,
            logs,
            max_log_lines: 100,
            should_quit: AtomicBool::new(false),
            restart_requested: AtomicBool::new(false),
            title: title.into(),
            gas_input_buffer: Arc::new(Mutex::new(String::new())),
            in_gas_input_mode: AtomicBool::new(false),
            gas_limit_input_buffer: Arc::new(Mutex::new(String::new())),
            in_gas_limit_input_mode: AtomicBool::new(false),
            gas_limit_cross_input_buffer: Arc::new(Mutex::new(String::new())),
            in_gas_limit_cross_input_mode: AtomicBool::new(false),
        }
    }

    /// Get a snapshot of the current stats.
    pub fn stats_snapshot(&self) -> StatsSnapshot {
        self.stats.snapshot()
    }

    /// Add a log message to the ring buffer.
    pub fn log(&self, message: impl Into<String>) {
        let mut logs = self.logs.lock().unwrap();
        let message = message.into();

        // Add timestamp prefix
        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
        let formatted = format!("[{}] {}", timestamp, message);

        if logs.len() >= self.max_log_lines {
            logs.pop_front();
        }
        logs.push_back(formatted);
    }

    /// Get the current log lines.
    pub fn get_logs(&self) -> Vec<String> {
        let logs = self.logs.lock().unwrap();
        logs.iter().cloned().collect()
    }

    /// Signal the app to quit.
    pub fn quit(&self) {
        self.should_quit.store(true, Ordering::Relaxed);
    }

    /// Check if the app should quit.
    pub fn should_quit(&self) -> bool {
        self.should_quit.load(Ordering::Relaxed)
    }

    /// Signal a restart.
    pub fn request_restart(&self) {
        self.restart_requested.store(true, Ordering::Relaxed);
        self.should_quit.store(true, Ordering::Relaxed);
    }

    /// Check if a restart was requested.
    pub fn restart_requested(&self) -> bool {
        self.restart_requested.load(Ordering::Relaxed)
    }

    /// Set the total planned transactions.
    pub fn set_total_planned(&self, total: u64) {
        self.stats.set_total_planned(total);
    }

    /// Mark the broadcast as started.
    pub fn mark_started(&self) {
        self.stats.mark_started();
    }

    /// Enter gas price input mode.
    pub fn enter_gas_input_mode(&self) {
        self.in_gas_input_mode.store(true, Ordering::Relaxed);
        let mut buffer = self.gas_input_buffer.lock().unwrap();
        buffer.clear();
    }

    /// Exit gas price input mode.
    pub fn exit_gas_input_mode(&self) {
        self.in_gas_input_mode.store(false, Ordering::Relaxed);
    }

    /// Check if we're in gas input mode.
    pub fn in_gas_input_mode(&self) -> bool {
        self.in_gas_input_mode.load(Ordering::Relaxed)
    }

    /// Add a character to the gas input buffer.
    pub fn add_to_gas_input(&self, c: char) {
        let mut buffer = self.gas_input_buffer.lock().unwrap();
        buffer.push(c);
    }

    /// Remove the last character from the gas input buffer.
    pub fn backspace_gas_input(&self) {
        let mut buffer = self.gas_input_buffer.lock().unwrap();
        buffer.pop();
    }

    /// Get the current gas input buffer content.
    pub fn get_gas_input(&self) -> String {
        let buffer = self.gas_input_buffer.lock().unwrap();
        buffer.clone()
    }

    /// Clear the gas input buffer.
    #[allow(dead_code)]
    pub fn clear_gas_input(&self) {
        let mut buffer = self.gas_input_buffer.lock().unwrap();
        buffer.clear();
    }

    /// Apply the gas price from input buffer (converts Gwei to atomic units).
    pub fn apply_gas_price(&self) {
        let buffer = self.gas_input_buffer.lock().unwrap();
        if let Ok(gwei) = buffer.parse::<f64>() {
            if gwei > 0.0 {
                let atomic = ((gwei * 1_000_000_000.0) as u64).max(MIN_GAS_PRICE);
                self.stats.set_gas_price_override(atomic);
                self.log(format!("Gas price set to {:.3} Gwei", atomic as f64 / 1_000_000_000.0));
            }
        }
    }

    /// Enter gas limit input mode.
    pub fn enter_gas_limit_input_mode(&self) {
        self.in_gas_limit_input_mode.store(true, Ordering::Relaxed);
        let mut buffer = self.gas_limit_input_buffer.lock().unwrap();
        buffer.clear();
    }

    /// Exit gas limit input mode.
    pub fn exit_gas_limit_input_mode(&self) {
        self.in_gas_limit_input_mode.store(false, Ordering::Relaxed);
    }

    /// Check if we're in gas limit input mode.
    pub fn in_gas_limit_input_mode(&self) -> bool {
        self.in_gas_limit_input_mode.load(Ordering::Relaxed)
    }

    /// Add a character to the gas limit input buffer.
    pub fn add_to_gas_limit_input(&self, c: char) {
        let mut buffer = self.gas_limit_input_buffer.lock().unwrap();
        buffer.push(c);
    }

    /// Remove the last character from the gas limit input buffer.
    pub fn backspace_gas_limit_input(&self) {
        let mut buffer = self.gas_limit_input_buffer.lock().unwrap();
        buffer.pop();
    }

    /// Get the current gas limit input buffer content.
    pub fn get_gas_limit_input(&self) -> String {
        let buffer = self.gas_limit_input_buffer.lock().unwrap();
        buffer.clone()
    }

    /// Apply the gas limit from input buffer (interprets value as millions of gas units).
    pub fn apply_gas_limit(&self) {
        let buffer = self.gas_limit_input_buffer.lock().unwrap();
        if let Ok(millions) = buffer.parse::<f64>() {
            if millions > 0.0 {
                let gas = (millions * 1_000_000.0) as u64;
                self.stats.set_gas_limit_override(gas);
                self.log(format!("S1 gas limit set to {}M ({})", millions, gas));
            }
        }
    }

    // ── Cross-shard gas limit (J key) ─────────────────────────────────

    pub fn enter_gas_limit_cross_input_mode(&self) {
        self.in_gas_limit_cross_input_mode.store(true, Ordering::Relaxed);
        let mut buffer = self.gas_limit_cross_input_buffer.lock().unwrap();
        buffer.clear();
    }

    pub fn exit_gas_limit_cross_input_mode(&self) {
        self.in_gas_limit_cross_input_mode.store(false, Ordering::Relaxed);
    }

    pub fn in_gas_limit_cross_input_mode(&self) -> bool {
        self.in_gas_limit_cross_input_mode.load(Ordering::Relaxed)
    }

    pub fn add_to_gas_limit_cross_input(&self, c: char) {
        let mut buffer = self.gas_limit_cross_input_buffer.lock().unwrap();
        buffer.push(c);
    }

    pub fn backspace_gas_limit_cross_input(&self) {
        let mut buffer = self.gas_limit_cross_input_buffer.lock().unwrap();
        buffer.pop();
    }

    pub fn get_gas_limit_cross_input(&self) -> String {
        let buffer = self.gas_limit_cross_input_buffer.lock().unwrap();
        buffer.clone()
    }

    pub fn apply_gas_limit_cross(&self) {
        let buffer = self.gas_limit_cross_input_buffer.lock().unwrap();
        if let Ok(millions) = buffer.parse::<f64>() {
            if millions > 0.0 {
                let gas = (millions * 1_000_000.0) as u64;
                self.stats.set_gas_limit_override_cross(gas);
                self.log(format!("S0/S2 gas limit set to {}M ({})", millions, gas));
            }
        }
    }
}

impl Clone for App {
    fn clone(&self) -> Self {
        Self {
            stats: Arc::clone(&self.stats),
            logs: Arc::clone(&self.logs),
            max_log_lines: self.max_log_lines,
            should_quit: AtomicBool::new(self.should_quit.load(Ordering::Relaxed)),
            restart_requested: AtomicBool::new(self.restart_requested.load(Ordering::Relaxed)),
            title: self.title.clone(),
            gas_input_buffer: Arc::clone(&self.gas_input_buffer),
            in_gas_input_mode: AtomicBool::new(self.in_gas_input_mode.load(Ordering::Relaxed)),
            gas_limit_input_buffer: Arc::clone(&self.gas_limit_input_buffer),
            in_gas_limit_input_mode: AtomicBool::new(self.in_gas_limit_input_mode.load(Ordering::Relaxed)),
            gas_limit_cross_input_buffer: Arc::clone(&self.gas_limit_cross_input_buffer),
            in_gas_limit_cross_input_mode: AtomicBool::new(self.in_gas_limit_cross_input_mode.load(Ordering::Relaxed)),
        }
    }
}

/// A thread-safe handle to the app for logging from other threads.
#[derive(Clone)]
#[allow(dead_code)]
pub struct AppLogHandle {
    logs: Arc<Mutex<VecDeque<String>>>,
    max_log_lines: usize,
}

#[allow(dead_code)]
impl AppLogHandle {
    /// Create a new log handle from an app.
    pub fn from_app(app: &App) -> Self {
        Self {
            logs: Arc::clone(&app.logs),
            max_log_lines: app.max_log_lines,
        }
    }

    /// Log a message.
    pub fn log(&self, message: impl Into<String>) {
        let mut logs = self.logs.lock().unwrap();
        let message = message.into();

        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
        let formatted = format!("[{}] {}", timestamp, message);

        if logs.len() >= self.max_log_lines {
            logs.pop_front();
        }
        logs.push_back(formatted);
    }
}
