use crate::tui::app::AppLogHandle;
use crate::tui::stats::Stats;
use crate::wallet::{RelayedTransaction, SignedEntry, WalletQueue};
use anyhow::{anyhow, Result};
use futures::future::join_all;
use serde_json::Value;
use std::sync::Arc;

use super::broadcast::BroadcastHelper;
use super::log_or_print;

const HTTP_RETRIES: usize = 3;

impl BroadcastHelper {
    /// Send a batch of relayed transactions to the network.
    /// Returns `(num_accepted, hashes_vec)`.
    pub(super) async fn send_batch_relayed(
        &self,
        txs: &[&RelayedTransaction],
    ) -> Result<(usize, Vec<String>)> {
        let url = format!("{}/transaction/send-multiple", self.network_url);

        let resp = self.client.post(&url).json(txs).send().await?;
        let body: Value = resp.json().await?;

        if let Some(data) = body.get("data") {
            let num_sent = data
                .get("txsSent")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let hashes = extract_indexed_strings(data, "txsHashes", txs.len());
            Ok((num_sent, hashes))
        } else {
            let err = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            Err(anyhow!("send-multiple failed: {}", err))
        }
    }

    /// Send signed entries in parallel slices, writing rejected indices into `rejected_out`.
    pub(super) async fn send_all_with_wallets(
        &self,
        label: &str,
        entries: &[SignedEntry],
        rejected_out: &mut Vec<usize>,
        sent_count: &mut usize,
        total_count: &mut usize,
        send_parallelism: usize,
        verbose: bool,
        stats: &Option<Arc<Stats>>,
        log_handle: &Option<AppLogHandle>,
    ) {
        rejected_out.clear();
        let txs: Vec<&RelayedTransaction> = entries.iter().map(|(tx, _, _)| tx).collect();
        let mut offset = 0;
        let mut slice_ranges: Vec<(usize, usize)> = Vec::with_capacity(send_parallelism);

        while offset < txs.len() {
            slice_ranges.clear();
            while slice_ranges.len() < send_parallelism && offset < txs.len() {
                let end = (offset + 1000).min(txs.len());
                slice_ranges.push((offset, end));
                offset = end;
            }

            let send_futs: Vec<_> = slice_ranges
                .iter()
                .map(|&(start, end)| {
                    let slice = &txs[start..end];
                    async move { self.send_batch_relayed(slice).await }
                })
                .collect();
            let results = join_all(send_futs).await;

            for (result, &(start, end)) in results.into_iter().zip(slice_ranges.iter()) {
                let slice_len = end - start;
                let (accepted, hashes) = match result {
                    Ok(r) => r,
                    Err(e) => match self
                        .retry_batch(&txs[start..end], label, stats, log_handle, e, slice_len)
                        .await
                    {
                        Some(r) => r,
                        None => {
                            // Treat total HTTP failure as rejection so txs are re-queued
                            // via the pushback cascade rather than dropped permanently.
                            rejected_out.extend(start..end);
                            log_or_print(
                                &format!(
                                    "[{}] ❌ HTTP failed after {} retries — {} txs re-queued.",
                                    label, HTTP_RETRIES, slice_len
                                ),
                                stats,
                                log_handle,
                            );
                            continue;
                        }
                    },
                };
                *sent_count += accepted;
                *total_count += accepted;
                if accepted < slice_len {
                    log_rejection_reasons(label, entries, &hashes, start, stats, log_handle);
                    let before = rejected_out.len();
                    collect_rejected_indices(&hashes, start, rejected_out);
                    log_or_print(
                        &format!(
                            "[{}] ⚠️ {}/{} accepted. {} rejected.",
                            label,
                            accepted,
                            slice_len,
                            rejected_out.len() - before
                        ),
                        stats,
                        log_handle,
                    );
                }
                if verbose {
                    log_tx_hashes(label, entries, &hashes, start, stats, log_handle);
                }
            }
        }
    }

    /// Retry a failed batch up to HTTP_RETRIES times. Returns None if all retries fail.
    async fn retry_batch(
        &self,
        txs: &[&RelayedTransaction],
        label: &str,
        stats: &Option<Arc<Stats>>,
        log_handle: &Option<AppLogHandle>,
        first_err: anyhow::Error,
        slice_len: usize,
    ) -> Option<(usize, Vec<String>)> {
        log_or_print(
            &format!(
                "[{}] ❌ HTTP error: {}. Retrying up to {}x...",
                label, first_err, HTTP_RETRIES
            ),
            stats,
            log_handle,
        );
        for attempt in 1..=HTTP_RETRIES {
            tokio::time::sleep(std::time::Duration::from_millis(200 * attempt as u64)).await;
            match self.send_batch_relayed(txs).await {
                Ok(result) => return Some(result),
                Err(e) => {
                    if attempt == HTTP_RETRIES {
                        log_or_print(
                            &format!(
                                "[{}] ❌ HTTP error after {} retries: {}. Dropping {} txs.",
                                label, HTTP_RETRIES, e, slice_len
                            ),
                            stats,
                            log_handle,
                        );
                    } else {
                        log_or_print(
                            &format!(
                                "[{}] ❌ HTTP retry {}/{} failed: {}.",
                                label, attempt, HTTP_RETRIES, e
                            ),
                            stats,
                            log_handle,
                        );
                    }
                }
            }
        }
        None
    }
}

impl BroadcastHelper {
    /// Fire-and-forget send for prepare-style one-shot operations.
    ///
    /// Flattens all pending txs from `queues`, signs them, and sends them in a
    /// single HTTP batch. No monitoring loop, no nonce re-sync, no per-burst log.
    /// Prints one summary line: `  [label] ✓ N/N sent.` (or a rejection warning).
    ///
    /// Returns `(accepted, total)`.
    pub async fn send_once(&self, label: &str, queues: Vec<WalletQueue>) -> (usize, usize) {
        let mut entries: Vec<SignedEntry> = queues
            .into_iter()
            .flat_map(|q| {
                let sender = q.sender.clone();
                q.pending
                    .into_iter()
                    .map(move |(tx, relayer)| (tx, sender.clone(), relayer))
            })
            .collect();

        let total = entries.len();
        if total == 0 {
            return (0, 0);
        }

        super::signer::sign_entries(&mut entries, 0);

        let txs: Vec<&RelayedTransaction> = entries.iter().map(|(tx, _, _)| tx).collect();
        let mut accepted = 0;
        let mut rejected = 0;

        for chunk in txs.chunks(1000) {
            match self.send_batch_relayed(chunk).await {
                Ok((n, hashes)) => {
                    accepted += n;
                    rejected += hashes.iter().filter(|h| h.is_empty()).count();
                }
                Err(e) => {
                    println!("  [{}] ⚠️ HTTP error: {}", label, e);
                    rejected += chunk.len();
                }
            }
        }

        if rejected > 0 {
            println!("  [{}] ⚠️ {}/{} accepted, {} rejected.", label, accepted, total, rejected);
        } else {
            println!("  [{}] ✓ {}/{} sent.", label, accepted, total);
        }

        (accepted, total)
    }
}

/// Extract a positional string array from a JSON map keyed by string indices.
fn extract_indexed_strings(data: &Value, key: &str, count: usize) -> Vec<String> {
    let map = match data.get(key).and_then(|v| v.as_object()) {
        Some(m) => m,
        None => return vec![String::new(); count],
    };
    (0..count)
        .map(|i| {
            map.get(&i.to_string())
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

/// Push indices of rejected transactions (empty hash) into `out`.
fn collect_rejected_indices(hashes: &[String], start: usize, out: &mut Vec<usize>) {
    out.extend(
        hashes
            .iter()
            .enumerate()
            .filter(|(_, h)| h.is_empty())
            .map(|(i, _)| start + i),
    );
}

/// Log per-tx rejection details for debugging.
fn log_rejection_reasons(
    label: &str,
    entries: &[SignedEntry],
    hashes: &[String],
    start: usize,
    stats: &Option<Arc<Stats>>,
    log_handle: &Option<AppLogHandle>,
) {
    for (i, hash) in hashes.iter().enumerate() {
        if hash.is_empty() {
            if let Some((tx, _, _)) = entries.get(start + i) {
                let bech32 = tx.sender.to_bech32_string();
                let abbrev = format!("{}…{}", &bech32[..12], &bech32[bech32.len() - 6..]);
                log_or_print(
                    &format!(
                        "[{}] ❌ REJECTED: sender={} nonce={} gas_price={}",
                        label, abbrev, tx.nonce, tx.gas_price,
                    ),
                    stats,
                    log_handle,
                );
            }
        }
    }
}

/// Log per-tx hashes (verbose mode).
fn log_tx_hashes(
    label: &str,
    entries: &[SignedEntry],
    hashes: &[String],
    start: usize,
    stats: &Option<Arc<Stats>>,
    log_handle: &Option<AppLogHandle>,
) {
    for (i, hash) in hashes.iter().enumerate() {
        if !hash.is_empty() {
            if let Some((tx, _, _)) = entries.get(start + i) {
                log_or_print(
                    &format!(
                        "[{}] TX_HASH: sender={} receiver={} nonce={} hash={}",
                        label, tx.sender, tx.receiver, tx.nonce, hash
                    ),
                    stats,
                    log_handle,
                );
            }
        }
    }
}
