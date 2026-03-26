use crate::wallet::WalletEntry;
use anyhow::{anyhow, Result};
use futures::StreamExt;
use multiversx_sdk::gateway::GetAccountRequest;
use multiversx_sdk_http::GatewayHttpProxy;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

/// Maximum concurrent nonce-fetch requests to the proxy.
const NONCE_SYNC_CONCURRENCY: usize = 20;
/// Number of retry attempts per wallet on transient errors (e.g. 429).
const NONCE_SYNC_RETRIES: usize = 3;
/// Delay between retries.
const NONCE_SYNC_RETRY_DELAY: Duration = Duration::from_secs(2);

/// NonceTracker handles syncing nonces from the network.
pub struct NonceTracker;

impl NonceTracker {
    /// Sync nonces for a list of wallets from the network.
    /// Requests are rate-limited to `NONCE_SYNC_CONCURRENCY` in-flight at a time,
    /// with up to `NONCE_SYNC_RETRIES` retries per wallet on transient errors.
    /// Sync nonces for all wallets and return their current balances (aEGLD) in wallet order.
    pub async fn sync_nonces(
        proxy: &GatewayHttpProxy,
        wallets: &[Arc<WalletEntry>],
    ) -> Result<Vec<u128>> {
        let results: Vec<Result<u128>> = futures::stream::iter(wallets.iter().cloned())
            .map(|w| async move {
                let bech32 = w.bech32.to_bech32_string();
                let mut last_err = anyhow!("no attempts made");
                for attempt in 0..=NONCE_SYNC_RETRIES {
                    if attempt > 0 {
                        tokio::time::sleep(NONCE_SYNC_RETRY_DELAY).await;
                    }
                    match proxy.http_request(GetAccountRequest::new(&w.bech32)).await {
                        Ok(acc) => {
                            w.nonce.store(acc.nonce, Ordering::SeqCst);
                            let balance = acc.balance.parse::<u128>().unwrap_or(0);
                            return Ok(balance);
                        }
                        Err(e) => last_err = anyhow!("{}: {}", bech32, e),
                    }
                }
                Err(last_err)
            })
            .buffered(NONCE_SYNC_CONCURRENCY)
            .collect()
            .await;

        let mut balances = Vec::with_capacity(wallets.len());
        for r in results {
            balances.push(r?);
        }
        Ok(balances)
    }
}
