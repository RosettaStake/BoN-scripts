mod broadcast;
mod sender;
mod signer;

pub use broadcast::{BroadcastConfig, BroadcastHelper};

use crate::tui::app::AppLogHandle;
use crate::tui::stats::Stats;
use std::io::{BufWriter, Write};
use std::sync::{Arc, Mutex, OnceLock};

struct LogFile {
    writer: Mutex<BufWriter<std::fs::File>>,
    file_only: bool,
}

static LOG_FILE: OnceLock<LogFile> = OnceLock::new();

/// Open `path` for writing.
/// If `file_only` is true, console/TUI output is suppressed — only the file receives logs.
pub fn init_log_file(path: &str, file_only: bool) -> anyhow::Result<()> {
    let f = std::fs::File::create(path)?;
    LOG_FILE.set(LogFile {
        writer: Mutex::new(BufWriter::new(f)),
        file_only,
    }).ok();
    if !file_only {
        println!("Mirroring log output to: {}", path);
    } else {
        println!("Logging to file only: {}", path);
    }
    Ok(())
}

/// Log to the TUI log pane, or fall back to stdout. Also mirrors to log file if set.
pub(crate) fn log_or_print(
    message: &str,
    _stats: &Option<Arc<Stats>>,
    log_handle: &Option<AppLogHandle>,
) {
    let file_only = LOG_FILE.get().map(|lf| lf.file_only).unwrap_or(false);

    if !file_only {
        if let Some(ref handle) = log_handle {
            handle.log(message);
        } else {
            println!("{}", message);
        }
    }

    if let Some(lf) = LOG_FILE.get() {
        if let Ok(mut f) = lf.writer.lock() {
            let _ = writeln!(f, "{}", message);
            let _ = f.flush();
        }
    }
}
