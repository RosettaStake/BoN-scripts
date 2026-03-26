//! Collect all EGLD from wallets back to a single destination address.

use crate::blockchain::nonce::NonceTracker;
use crate::commands::Command;
use crate::wallet::{RelayedTransaction, WalletManager};
use anyhow::{bail, Result};
use async_trait::async_trait;
use futures::StreamExt;
use multiversx_sdk::gateway::{GetAccountRequest, NetworkConfigRequest};
use multiversx_sdk_http::GatewayHttpProxy;
use num_bigint::BigUint;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TX_POLL_INTERVAL_MS: u64 = 1200;
const TX_POLL_TIMEOUT_SECS: u64 = 60;
const MAX_RETRY_ROUNDS: usize = 10;
const POLL_CONCURRENCY: usize = 20;
/// Gas limit for a simple EGLD transfer (no data).
const TRANSFER_GAS_LIMIT: u64 = 50_000;

/// Collect all EGLD from wallets/ back to a single destination address.
pub struct CollectCommand {
    pub wallets_dir: String,
    pub network_config: crate::network_config::NetworkConfig,
    /// Destination bech32 address to collect funds into.
    pub destination: String,
}

/// Parse the txsHashes response from send-multiple.
fn parse_txs_hashes(body: &serde_json::Value) -> HashMap<usize, String> {
    let mut map = HashMap::new();
    if let Some(hashes) = body["data"]["txsHashes"].as_object() {
        for (k, v) in hashes {
            if let (Ok(idx), Some(hash)) = (k.parse::<usize>(), v.as_str()) {
                if !hash.is_empty() {
                    map.insert(idx, hash.to_string());
                }
            }
        }
    }
    map
}

/// Poll a single tx hash until success/fail/invalid or timeout.
async fn poll_tx_status(client: &reqwest::Client, proxy: &str, hash: &str) -> bool {
    let url = format!("{}/transaction/{}/status", proxy.trim_end_matches('/'), hash);
    let deadline = Instant::now() + Duration::from_secs(TX_POLL_TIMEOUT_SECS);
    loop {
        tokio::time::sleep(Duration::from_millis(TX_POLL_INTERVAL_MS)).await;
        match client.get(&url).send().await {
            Ok(resp) => {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    if let Some(status) = body["data"]["status"].as_str() {
                        match status {
                            "success" => return true,
                            "fail" | "invalid" => return false,
                            _ => {}
                        }
                    }
                }
            }
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            return false;
        }
    }
}

#[async_trait]
impl Command for CollectCommand {
    async fn execute(&self) -> Result<()> {
        let mut wallet_manager = WalletManager::new(&self.wallets_dir);
        wallet_manager.load_wallets()?;

        let proxy_url = self.network_config.proxy.clone();
        let client = reqwest::Client::new();
        let gateway = GatewayHttpProxy::new(proxy_url.clone());
        let config = gateway.http_request(NetworkConfigRequest).await?;

        let all = wallet_manager.get_all_wallets();
        if all.is_empty() {
            bail!("No wallets loaded from {}", self.wallets_dir);
        }

        let dest = multiversx_chain_core::std::Bech32Address::from_bech32_string(
            self.destination.clone(),
        );

        println!("Collecting EGLD from {} wallets → {}", all.len(), self.destination);

        // Fetch balances for all wallets concurrently.
        let proxy_arc = Arc::new(gateway);
        let balances: Vec<(Arc<crate::wallet::WalletEntry>, u128, u64)> =
            futures::stream::iter(all.iter().cloned())
                .map(|w| {
                    let proxy = Arc::clone(&proxy_arc);
                    async move {
                        match proxy.http_request(GetAccountRequest::new(&w.bech32)).await {
                            Ok(acc) => {
                                let balance: u128 = BigUint::from_str(&acc.balance)
                                    .unwrap_or_default()
                                    .to_string()
                                    .parse()
                                    .unwrap_or(0);
                                (w, balance, acc.nonce)
                            }
                            Err(_) => (w, 0u128, 0u64),
                        }
                    }
                })
                .buffer_unordered(POLL_CONCURRENCY)
                .collect()
                .await;

        // Calculate transfer amounts: balance - gas fee.
        let gas_fee = TRANSFER_GAS_LIMIT as u128 * config.min_gas_price as u128;
        let mut pending: Vec<(Arc<crate::wallet::WalletEntry>, u128)> = Vec::new();
        let mut total_to_collect: u128 = 0;
        let mut skipped = 0usize;

        for (w, balance, nonce) in &balances {
            if *balance <= gas_fee {
                skipped += 1;
                continue;
            }
            let send_amount = balance - gas_fee;
            total_to_collect += send_amount;
            // Store the nonce for later use.
            w.nonce.store(*nonce, std::sync::atomic::Ordering::SeqCst);
            pending.push((w.clone(), send_amount));
        }

        println!(
            "  {} wallets with balance (skipping {} empty/dust)",
            pending.len(),
            skipped
        );
        println!(
            "  Total to collect: {:.6} EGLD ({} atomic)",
            total_to_collect as f64 / 1e18,
            total_to_collect
        );
        println!("  Gas fee per tx: {:.6} EGLD", gas_fee as f64 / 1e18);

        if pending.is_empty() {
            println!("Nothing to collect.");
            return Ok(());
        }

        // Send collection txs in retry rounds.
        let mut pending_wallets = pending;

        for round in 0..MAX_RETRY_ROUNDS {
            if pending_wallets.is_empty() {
                break;
            }
            println!(
                "\n[Round {}/{}] Collecting from {} wallet(s)...",
                round + 1,
                MAX_RETRY_ROUNDS,
                pending_wallets.len()
            );

            // Re-fetch nonces on retry rounds.
            if round > 0 {
                let wallets_ref: Vec<Arc<crate::wallet::WalletEntry>> =
                    pending_wallets.iter().map(|(w, _)| w.clone()).collect();
                // Group by shard for nonce sync.
                for shard in 0..3u8 {
                    let shard_wallets: Vec<Arc<crate::wallet::WalletEntry>> = wallets_ref
                        .iter()
                        .filter(|w| w.shard == shard)
                        .cloned()
                        .collect();
                    if !shard_wallets.is_empty() {
                        let px = GatewayHttpProxy::new(self.network_config.shard_url(shard));
                        let _ = NonceTracker::sync_nonces(&px, &shard_wallets).await;
                    }
                }
            }

            // All wallets send 1 tx each — no mempool limit issue (different senders).
            // But we still batch the HTTP send-multiple calls for efficiency.
            const SEND_BATCH: usize = 100;
            let mut next_pending: Vec<(Arc<crate::wallet::WalletEntry>, u128)> = Vec::new();
            let mut succeeded = 0usize;

            for sub_batch in pending_wallets.chunks(SEND_BATCH) {
                let mut txs: Vec<serde_json::Value> = Vec::with_capacity(sub_batch.len());
                for (w, amount) in sub_batch {
                    let mut tx = RelayedTransaction::from_parts(
                        w.get_nonce_then_increment(),
                        *amount,
                        &dest,
                        &w.bech32,
                        config.min_gas_price,
                        TRANSFER_GAS_LIMIT,
                        &config.chain_id,
                        config.min_transaction_version,
                        None,
                    );
                    tx.sign_sender(w);
                    txs.push(serde_json::to_value(&tx).unwrap());
                }

                let url = format!(
                    "{}/transaction/send-multiple",
                    proxy_url.trim_end_matches('/')
                );
                let accepted: Vec<(Arc<crate::wallet::WalletEntry>, u128, String)> =
                    match client.post(&url).json(&txs).send().await {
                        Ok(resp) => {
                            let body: serde_json::Value =
                                resp.json().await.unwrap_or_default();
                            let hashes = parse_txs_hashes(&body);
                            let mut acc = Vec::new();
                            for (local_idx, (w, amount)) in sub_batch.iter().enumerate() {
                                if let Some(hash) = hashes.get(&local_idx) {
                                    acc.push((w.clone(), *amount, hash.clone()));
                                } else {
                                    next_pending.push((w.clone(), *amount));
                                }
                            }
                            println!(
                                "  -> {} accepted, {} rejected.",
                                hashes.len(),
                                sub_batch.len() - hashes.len()
                            );
                            acc
                        }
                        Err(e) => {
                            println!("  -> Sub-batch send failed: {}", e);
                            next_pending.extend(sub_batch.iter().cloned());
                            continue;
                        }
                    };

                // Poll this sub-batch.
                println!("  Polling {} tx(s)...", accepted.len());
                let proxy_url_ref = &proxy_url;
                let client_ref = &client;
                let poll_results: Vec<(Arc<crate::wallet::WalletEntry>, u128, bool)> =
                    futures::stream::iter(accepted.into_iter())
                        .map(|(w, amount, hash)| async move {
                            let ok =
                                poll_tx_status(client_ref, proxy_url_ref, &hash).await;
                            (w, amount, ok)
                        })
                        .buffer_unordered(POLL_CONCURRENCY)
                        .collect()
                        .await;

                for (w, amount, ok) in poll_results {
                    if ok {
                        succeeded += 1;
                    } else {
                        next_pending.push((w, amount));
                    }
                }
            }

            println!(
                "  Round {}: {} succeeded, {} failed/timed-out.",
                round + 1,
                succeeded,
                next_pending.len()
            );

            pending_wallets = next_pending;
        }

        if pending_wallets.is_empty() {
            println!("\nAll wallets collected successfully → {}", self.destination);
        } else {
            println!(
                "\nWarning: {} wallet(s) could not be collected after {} rounds:",
                pending_wallets.len(),
                MAX_RETRY_ROUNDS
            );
            for (w, _) in &pending_wallets {
                println!("  {}", w.bech32.to_bech32_string());
            }
        }

        Ok(())
    }
}
