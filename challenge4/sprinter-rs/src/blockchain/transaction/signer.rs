use crate::tui::app::AppLogHandle;
use crate::tui::stats::Stats;
use crate::wallet::SignedEntry;
use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};
use std::sync::Arc;
use std::time::Instant;

/// Sign all burst entries, logging the time taken.
pub fn sign_burst_with_wallets(
    entries: &mut Vec<SignedEntry>,
    sign_threads: usize,
    stats: &Option<Arc<Stats>>,
    log_handle: &Option<AppLogHandle>,
) {
    let start = Instant::now();
    let count = entries.len();
    sign_entries(entries, sign_threads);
    let elapsed = start.elapsed();
    super::log_or_print(
        &format!("  Signed {} txs in {:.3}s.", count, elapsed.as_secs_f64()),
        stats,
        log_handle,
    );
}

/// Sign entries in-place using the given number of threads (0 = auto).
/// Uses rayon's global thread pool to avoid per-burst OS thread spawn overhead.
pub fn sign_entries(entries: &mut [SignedEntry], sign_threads: usize) {
    if sign_threads == 1 {
        for (tx, sender, relayer) in entries.iter_mut() {
            tx.sign_both(sender, relayer.as_deref());
        }
        return;
    }

    let pool = if sign_threads == 0 {
        rayon::ThreadPoolBuilder::new().build()
    } else {
        rayon::ThreadPoolBuilder::new().num_threads(sign_threads).build()
    };

    match pool {
        Ok(pool) => {
            pool.install(|| {
                entries.par_iter_mut().for_each(|(tx, sender, relayer)| {
                    tx.sign_both(sender, relayer.as_deref());
                });
            });
        }
        Err(_) => {
            // Fallback to single-threaded if pool creation fails
            for (tx, sender, relayer) in entries.iter_mut() {
                tx.sign_both(sender, relayer.as_deref());
            }
        }
    }
}
