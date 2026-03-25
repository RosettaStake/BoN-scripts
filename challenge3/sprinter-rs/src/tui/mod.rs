//! TUI dashboard for transaction broadcasting.
//!
//! Provides a split-pane terminal UI with:
//! - Left column: Live statistics (TPS, progress, success rate, ETA, etc.)
//! - Right column: Scrollable log output

pub mod app;
pub mod stats;
pub mod ui;

use crate::tui::app::{App, AppLogHandle};
use crate::tui::stats::Stats;
use crate::tui::ui::{render, render_summary};
use anyhow::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Result of a TUI run — either completed normally or restart was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunResult {
    Completed,
    Restart,
}

/// The TUI runner.
pub struct Tui {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    app: App,
}

impl Tui {
    /// Create a new TUI instance.
    /// Returns None if stdout is not a TTY.
    pub fn try_new(title: impl Into<String>) -> Result<Option<Self>> {
        // Check if we're in a terminal
        if !is_tty() {
            return Ok(None);
        }

        let app = App::new(title);

        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        Ok(Some(Self { terminal, app }))
    }

    /// Get the shared stats collector.
    pub fn stats(&self) -> Arc<Stats> {
        Arc::clone(&self.app.stats)
    }

    /// Get a handle for logging to the TUI's log pane.
    pub fn log_handle(&self) -> crate::tui::app::AppLogHandle {
        crate::tui::app::AppLogHandle::from_app(&self.app)
    }

    /// Run the TUI event loop until completion or quit signal.
    /// The provided future runs in a separate task and updates the app state.
    pub async fn run<F>(&mut self, work: F) -> Result<RunResult>
    where
        F: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        // Create a channel for the work task to signal completion
        let (tx, mut rx) = tokio::sync::oneshot::channel();

        // Spawn the work task
        let app_clone = self.app.clone();
        let work_handle = tokio::spawn(async move {
            app_clone.mark_started();
            let result = work.await;
            let _ = tx.send(result);
        });

        let tick_rate = Duration::from_millis(100);
        let mut last_tick = Instant::now();

        // Event loop
        loop {
            // Draw the UI
            self.terminal.draw(|f| render(f, &self.app))?;

            // Check if work is complete
            if let Ok(result) = rx.try_recv() {
                // Work finished
                if let Err(e) = result {
                    self.app.log(format!("Error: {}", e));
                }
                break;
            }

            // Check for quit signal from app
            if self.app.should_quit() {
                break;
            }

            // Handle events with timeout
            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));

            if crossterm::event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    // Handle gas input mode separately
                    if self.app.in_gas_input_mode() {
                        match key.code {
                            KeyCode::Enter => {
                                self.app.apply_gas_price();
                                self.app.exit_gas_input_mode();
                            }
                            KeyCode::Esc => {
                                self.app.exit_gas_input_mode();
                            }
                            KeyCode::Backspace => {
                                self.app.backspace_gas_input();
                            }
                            KeyCode::Char(c) => {
                                // Only allow digits and decimal point
                                if c.is_ascii_digit() || c == '.' {
                                    self.app.add_to_gas_input(c);
                                }
                            }
                            _ => {}
                        }
                    } else {
                        // Normal mode
                        match key.code {
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                self.app.quit();
                                break;
                            }
                            KeyCode::Char('r') | KeyCode::Char('R') => {
                                self.app.log("🔄 Restart requested! Re-generating transactions...".to_string());
                                self.app.request_restart();
                                work_handle.abort();
                                break;
                            }
                            KeyCode::Char('g') | KeyCode::Char('G') => {
                                self.app.enter_gas_input_mode();
                                self.app.log("Enter gas price (Gwei) and press Enter:".to_string());
                            }
                            _ => {}
                        }
                    }
                }
            }

            if last_tick.elapsed() >= tick_rate {
                last_tick = Instant::now();
            }
        }

        let result = if self.app.restart_requested() {
            RunResult::Restart
        } else {
            RunResult::Completed
        };

        Ok(result)
    }

}

impl Drop for Tui {
    fn drop(&mut self) {
        // Restore terminal
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        let _ = self.terminal.show_cursor();

        // Print summary only if not restarting
        if !self.app.restart_requested() {
            render_summary(&self.app);
        }
    }
}

/// Check if stdout is a TTY.
fn is_tty() -> bool {
    atty::is(atty::Stream::Stdout)
}

/// Print a summary of the broadcast results (for use without TUI).
pub fn print_summary(stats: &Stats) {
    let snapshot = stats.snapshot();

    println!("\n{}", "=".repeat(60));
    println!("📊 BROADCAST SUMMARY");
    println!("{}", "=".repeat(60));
    println!("  Goal:             {:>12}", snapshot.total_planned);
    println!("  Confirmed:        {:>12}", snapshot.confirmed_count);
    println!();
    println!(
        "  Elapsed Time:     {}",
        format_duration(snapshot.elapsed_secs as u64)
    );
    println!("  Average TPS:      {:.1}", snapshot.current_tps);
    println!("{}", "=".repeat(60));
}

/// Format seconds as HH:MM:SS.
fn format_duration(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    format!("{:02}:{:02}:{:02}", hours, mins, secs)
}

/// Convenience function to run work with optional TUI.
/// If TUI is available and `no_tui` is false, shows the dashboard; otherwise uses println! output.
pub async fn run_with_optional_tui<F>(
    title: impl Into<String>,
    total_planned: u64,
    no_tui: bool,
    work: impl FnOnce(Arc<Stats>, Option<AppLogHandle>) -> F,
) -> Result<RunResult>
where
    F: std::future::Future<Output = Result<()>> + Send + 'static,
{
    let title = title.into();

    if !no_tui {
        if let Some(mut tui) = Tui::try_new(&title)? {
            // TUI mode
            tui.app.set_total_planned(total_planned);
            let stats = tui.stats();
            let log_handle = tui.log_handle();

            let work_future = work(stats, Some(log_handle));
            let result = tui.run(work_future).await?;
            return Ok(result);
        }
    }

    // Fallback mode (--no-tui or not a TTY) - just run the work
    let stats = Stats::new_arc();
    stats.set_total_planned(total_planned);
    stats.mark_started();

    let work_future = work(Arc::clone(&stats), None);
    work_future.await?;

    print_summary(&stats);

    Ok(RunResult::Completed)
}
