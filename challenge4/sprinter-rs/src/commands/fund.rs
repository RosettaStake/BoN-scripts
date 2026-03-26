use crate::commands::Command;
use crate::wallet::WalletManager;
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use multiversx_sdk::data::transaction::Transaction;
use multiversx_sdk::gateway::{GetAccountRequest, NetworkConfigRequest};
use multiversx_sdk::wallet::Wallet;
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

/// Fund wallets from a whale wallet.
pub struct FundCommand {
    pub wallets_dir: String,
    pub network_config: crate::network_config::NetworkConfig,
    pub whale: String,
    pub amount: Option<u128>,
}

/// Parse the txsHashes response from send-multiple.
/// Returns a map from positional index (as returned by gateway) to tx hash.
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
/// Returns true if the tx succeeded, false otherwise.
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
impl Command for FundCommand {
    async fn execute(&self) -> Result<()> {
        let mut wallet_manager = WalletManager::new(&self.wallets_dir);
        wallet_manager.load_wallets()?;

        let proxy_url = self.network_config.proxy.clone();
        let client = reqwest::Client::new();
        let gateway: GatewayHttpProxy = GatewayHttpProxy::new(proxy_url.clone());

        let all = wallet_manager.get_all_wallets();
        if all.is_empty() {
            bail!("No wallets loaded.");
        }

        let whale = Wallet::from_pem_file(&self.whale)
            .context("Failed to load whale wallet")?;
        let whale_addr = whale.to_address();
        let whale_bech32 = whale_addr.to_bech32("erd");
        println!("Whale wallet: {}", whale_bech32.to_bech32_string());

        let whale_account = gateway
            .http_request(GetAccountRequest::new(&whale_bech32))
            .await?;
        let whale_balance = BigUint::from_str(&whale_account.balance).unwrap_or_default();
        let whale_balance_u128: u128 = whale_balance.to_string().parse().unwrap_or(0);
        println!(
            "Whale balance: {:.4} EGLD ({} atomic units)",
            whale_balance_u128 as f64 / 1e18,
            whale_balance_u128
        );

        let num_wallets = all.len() as u128;
        let fee_reserve = num_wallets * 50_000 * 1_000_000_000;

        let total_to_distribute = if let Some(amt) = self.amount {
            if amt + fee_reserve > whale_balance_u128 {
                bail!("Specified amount plus fees exceeds whale balance.");
            }
            println!(
                "Distributing specified amount: {:.4} EGLD",
                amt as f64 / 1e18
            );
            amt
        } else {
            let ttd = whale_balance_u128.saturating_sub(fee_reserve);
            if ttd == 0 {
                bail!("Insufficient balance for distribution.");
            }
            println!(
                "Distributing all available balance (minus fee reserve): {:.4} EGLD",
                ttd as f64 / 1e18
            );
            ttd
        };

        let amount_per_wallet = total_to_distribute / all.len() as u128;
        println!(
            "Amount per wallet: {:.6} EGLD ({} atomic units)",
            amount_per_wallet as f64 / 1e18,
            amount_per_wallet
        );
        println!("Target wallets: {}", all.len());

        let config = gateway.http_request(NetworkConfigRequest).await?;

        // pending_wallets tracks which wallet entries still need funding
        let mut pending_wallets: Vec<Arc<crate::wallet::WalletEntry>> = all.into_iter().collect();

        for round in 0..MAX_RETRY_ROUNDS {
            if pending_wallets.is_empty() {
                break;
            }
            println!(
                "\n[Round {}/{}] Funding {} wallet(s)...",
                round + 1,
                MAX_RETRY_ROUNDS,
                pending_wallets.len()
            );

            // Fetch fresh nonce once per retry round.
            let whale_account = gateway
                .http_request(GetAccountRequest::new(&whale_bech32))
                .await?;
            let mut nonce = whale_account.nonce;

            // Send all pending wallets in sub-batches of MEMPOOL_BATCH (mempool limit per
            // sender). Each sub-batch is sent, polled to confirmation, then the next
            // sub-batch is sent. This avoids wasted signing and HTTP calls.
            const MEMPOOL_BATCH: usize = 100;
            let mut next_pending: Vec<Arc<crate::wallet::WalletEntry>> = Vec::new();
            let mut succeeded = 0usize;

            for sub_batch in pending_wallets.chunks(MEMPOOL_BATCH) {
                // Build and sign this sub-batch
                let mut txs: Vec<Transaction> = Vec::with_capacity(sub_batch.len());
                for w in sub_batch {
                    let mut tx = Transaction {
                        nonce,
                        value: amount_per_wallet.to_string(),
                        receiver: w.bech32.clone(),
                        sender: whale_bech32.clone(),
                        gas_price: config.min_gas_price,
                        gas_limit: 50_000,
                        data: None,
                        signature: None,
                        chain_id: config.chain_id.clone(),
                        version: config.min_transaction_version,
                        options: 0,
                    };
                    let sig = whale.sign_tx(&tx);
                    tx.signature = Some(hex::encode(sig));
                    txs.push(tx);
                    nonce += 1;
                }

                // Send sub-batch
                println!("  Sending sub-batch of {} txs...", txs.len());
                let url = format!("{}/transaction/send-multiple", proxy_url.trim_end_matches('/'));
                let accepted: Vec<(Arc<crate::wallet::WalletEntry>, String)> =
                    match client.post(&url).json(&txs).send().await {
                        Ok(resp) => {
                            let body: serde_json::Value = resp.json().await.unwrap_or_default();
                            let hashes = parse_txs_hashes(&body);
                            let mut acc = Vec::new();
                            for (local_idx, w) in sub_batch.iter().enumerate() {
                                if let Some(hash) = hashes.get(&local_idx) {
                                    acc.push((w.clone(), hash.clone()));
                                } else {
                                    next_pending.push(w.clone());
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

                // Poll this sub-batch before sending the next one
                println!("  Polling {} tx(s)...", accepted.len());
                let proxy_url_ref = &proxy_url;
                let client_ref = &client;
                let poll_results: Vec<(Arc<crate::wallet::WalletEntry>, bool)> =
                    futures::stream::iter(accepted.into_iter())
                        .map(|(w, hash)| async move {
                            let ok = poll_tx_status(client_ref, proxy_url_ref, &hash).await;
                            (w, ok)
                        })
                        .buffer_unordered(POLL_CONCURRENCY)
                        .collect()
                        .await;

                for (w, ok) in poll_results {
                    if ok {
                        succeeded += 1;
                    } else {
                        next_pending.push(w);
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
            println!("\nAll wallets funded successfully.");
        } else {
            println!(
                "\nWarning: {} wallet(s) could not be funded after {} rounds:",
                pending_wallets.len(),
                MAX_RETRY_ROUNDS
            );
            for w in &pending_wallets {
                println!("  {}", w.bech32.to_bech32_string());
            }
        }
        println!("{}", "=".repeat(60));
        Ok(())
    }
}
