use crate::commands::Command;
use crate::wallet::WalletManager;
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use multiversx_sdk::gateway::GetAccountRequest;
use multiversx_sdk_http::GatewayHttpProxy;
use num_bigint::BigUint;
use std::str::FromStr;
use std::sync::Arc;

const CHECK_CONCURRENCY: usize = 20;

/// Check wallet balances and nonces.
pub struct CheckWalletsCommand {
    pub wallets_dir: String,
    pub network_config: crate::network_config::NetworkConfig,
}

#[async_trait]
impl Command for CheckWalletsCommand {
    async fn execute(&self) -> Result<()> {
        let mut wallet_manager = WalletManager::new(&self.wallets_dir);
        wallet_manager.load_wallets()?;

        let all = wallet_manager.get_all_wallets();
        if all.is_empty() {
            println!("Error: No wallets loaded.");
            return Ok(());
        }

        println!("Querying network for {} wallets...", all.len());
        println!(
            "{:<65} | {:<5} | {:<10} | Balance (EGLD)",
            "Address", "Shard", "Nonce"
        );
        println!("{}", "-".repeat(110));

        let proxy = Arc::new(GatewayHttpProxy::new(self.network_config.proxy.clone()));

        let results: Vec<String> = futures::stream::iter(all.iter().cloned())
            .map(|w| {
                let proxy = Arc::clone(&proxy);
                async move {
                    let bech32 = w.bech32.clone();
                    let shard = w.shard;
                    match proxy.http_request(GetAccountRequest::new(&bech32)).await {
                        Ok(acc) => {
                            let balance = BigUint::from_str(&acc.balance).unwrap_or_default();
                            let divisor = BigUint::from(10u64.pow(18));
                            let whole = &balance / &divisor;
                            let frac = &balance % &divisor;
                            let frac_str = format!("{:0>18}", frac);
                            let frac_6: String = frac_str.chars().take(6).collect();
                            format!(
                                "{:<65} | {:<5} | {:<10} | {}.{}",
                                bech32.to_bech32_string(),
                                shard,
                                acc.nonce,
                                whole,
                                frac_6,
                            )
                        }
                        Err(e) => format!(
                            "{:<65} | Error fetching: {}",
                            bech32.to_bech32_string(),
                            e
                        ),
                    }
                }
            })
            .buffered(CHECK_CONCURRENCY)
            .collect()
            .await;

        for line in results {
            println!("{}", line);
        }
        println!("{}", "-".repeat(110));
        Ok(())
    }
}
