//! UI rendering for the TUI dashboard using ratatui.

use crate::tui::app::App;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Wrap};
use ratatui::Frame;

/// Format a number with thousand separators.
fn format_num(n: u64) -> String {
    n.to_string()
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join(",")
}

/// Render the main UI.
pub fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Length(3), Constraint::Min(10)])
        .split(f.area());

    // Header
    render_header(f, app, chunks[0]);

    // Main content (stats + logs)
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[1]);

    render_stats(f, app, main_chunks[0]);
    render_logs(f, app, main_chunks[1]);
}

/// Render the header block.
fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let header_text = if app.in_gas_input_mode() {
        format!("Enter gas price (Gwei): {}_", app.get_gas_input())
    } else {
        format!("{} | [R] Restart  [G] Gas  [Ctrl+C] Quit", app.title)
    };

    let (border_color, text_color) = if app.in_gas_input_mode() {
        (Color::Yellow, Color::Yellow)
    } else {
        (Color::Cyan, Color::Cyan)
    };

    let header = Paragraph::new(header_text)
        .style(Style::default().fg(text_color).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(border_color)));

    f.render_widget(header, area);
}

/// Render the statistics panel (left column).
fn render_stats(f: &mut Frame, app: &App, area: Rect) {
    let stats = app.stats_snapshot();

    let inner = Block::default()
        .title(" Statistics ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));

    let inner_area = inner.inner(area);
    f.render_widget(inner, area);

    // Split the inner area into sections
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2), // Throughput
            Constraint::Length(2), // Batch Size
            Constraint::Length(2), // Confirmed
            Constraint::Length(2), // Pending
            Constraint::Length(1), // Spacer
            Constraint::Length(2), // Elapsed
            Constraint::Length(2), // ETA
            Constraint::Length(2), // Last Burst
            Constraint::Length(2), // Gas Price
            Constraint::Length(3), // Progress bar
            Constraint::Min(0),    // Remaining space
        ])
        .split(inner_area);

    // Throughput
    let tps_text = format!("{:.1} TPS", stats.current_tps);
    render_stat_row(f, "Throughput:", &tps_text, rows[0], Color::Yellow);

    // Batch size (AIMD-adjusted)
    render_stat_row(f, "Batch Size:", &format_num(stats.current_batch_size), rows[1], Color::White);

    // Burn-all mode: show only confirmed count, hide ratio/progress/ETA
    let is_burn_all = stats.burn_all_mode;
    
    if is_burn_all {
        // Confirmed (no goal)
        let confirmed_text = format_num(stats.confirmed_count);
        render_stat_row(f, "Confirmed:", &confirmed_text, rows[2], Color::Green);
    } else {
        // Confirmed vs goal
        let confirmed_text = format!("{} / {}", format_num(stats.confirmed_count), format_num(stats.total_planned));
        render_stat_row(f, "Confirmed:", &confirmed_text, rows[2], Color::Green);
    }

    // Pending
    let pending_text = format_num(stats.deferred_count);
    let pending_color = if stats.deferred_count == 0 {
        Color::Green
    } else if stats.deferred_count < 100 {
        Color::Yellow
    } else {
        Color::Red
    };
    render_stat_row(f, "Pending:", &pending_text, rows[3], pending_color);

    // Elapsed Time
    let elapsed = format_duration(stats.elapsed_secs as u64);
    render_stat_row(f, "Elapsed:", &elapsed, rows[5], Color::White);

    if !is_burn_all {
        // ETA (only for non-burn-all)
        let eta = format_duration(stats.eta_secs);
        let eta_color = if stats.eta_secs < 60 {
            Color::Green
        } else if stats.eta_secs < 600 {
            Color::Yellow
        } else {
            Color::White
        };
        render_stat_row(f, "ETA:", &eta, rows[6], eta_color);
        
        let burst_text = format!(
            "{} txs in {:.1}s",
            format_num(stats.last_burst_sent), stats.last_burst_time_ms as f64 / 1000.0
        );
        render_stat_row(f, "Last Burst:", &burst_text, rows[7], Color::Cyan);
    }

    // Gas Price
    if stats.gas_price_override > 0 {
        let override_gwei = stats.gas_price_override as f64 / 1_000_000_000.0;
        let gas_text = format!("{:.3} Gwei ⚠ OVERRIDE", override_gwei);
        render_stat_row(f, "Gas Price:", &gas_text, rows[8], Color::Yellow);
    } else {
        let gas_text = format!("{:.3} Gwei", stats.current_gas_gwei as f64 / 1000.0);
        render_stat_row(f, "Current Gas:", &gas_text, rows[8], Color::Magenta);
    }

    if !is_burn_all {
        // Progress bar (only for non-burn-all)
        let progress_bar = Gauge::default()
            .block(Block::default())
            .gauge_style(Style::default().fg(Color::Cyan).bg(Color::Black))
            .ratio(stats.progress_pct / 100.0)
            .label(format!("{:.1}%", stats.progress_pct));
        f.render_widget(progress_bar, rows[9]);
    }
}

/// Render a single statistics row with label and value.
fn render_stat_row(f: &mut Frame, label: &str, value: &str, area: Rect, value_color: Color) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(15), Constraint::Min(20)])
        .split(area);

    let label_widget = Paragraph::new(label).style(Style::default().fg(Color::Gray));
    f.render_widget(label_widget, cols[0]);

    let value_widget = Paragraph::new(value)
        .style(Style::default().fg(value_color).add_modifier(Modifier::BOLD));
    f.render_widget(value_widget, cols[1]);
}

/// Render the logs panel (right column).
fn render_logs(f: &mut Frame, app: &App, area: Rect) {
    let logs = app.get_logs();

    let inner = Block::default()
        .title(" Logs ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));

    let inner_area = inner.inner(area);
    f.render_widget(inner, area);

    // Create log lines
    let log_lines: Vec<Line> = logs
        .iter()
        .map(|log| {
            // Color code based on log content
            let style = if log.contains('❌') || log.contains("error") || log.contains("failed") {
                Style::default().fg(Color::Red)
            } else if log.contains('⚠') || log.contains("warning") || log.contains("deferred") {
                Style::default().fg(Color::Yellow)
            } else if log.contains('✅') || log.contains("success") {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };

            Line::from(vec![Span::styled(log.clone(), style)])
        })
        .collect();

    let log_paragraph = Paragraph::new(log_lines)
        .block(Block::default())
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true });

    f.render_widget(log_paragraph, inner_area);
}

/// Format seconds as HH:MM:SS.
fn format_duration(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    format!("{:02}:{:02}:{:02}", hours, mins, secs)
}

/// Render a final summary after the TUI exits.
pub fn render_summary(app: &App) {
    let stats = app.stats_snapshot();

    println!("\n{}", "=".repeat(60));
    println!("📊 BROADCAST SUMMARY");
    println!("{}", "=".repeat(60));
    println!("  Goal:             {:>12}", format_num(stats.total_planned));
    println!("  Confirmed:        {:>12}", format_num(stats.confirmed_count));
    println!("  Pending:          {:>12}", format_num(stats.deferred_count));
    println!();
    println!(
        "  Elapsed Time:     {}",
        format_duration(stats.elapsed_secs as u64)
    );
    println!("  Average TPS:      {:.1}", stats.current_tps);
    println!("{}", "=".repeat(60));
}
