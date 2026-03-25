use crate::blockchain::transaction::{BroadcastConfig, BroadcastHelper};
use crate::commands::Command;
use crate::network_config::NetworkConfig;
use crate::tui;
use crate::tui::RunResult;
use crate::utils::wait_for_user_confirmation;
use crate::wallet::{WalletManager, WalletQueue};
use anyhow::Result;
use async_trait::async_trait;
use multiversx_sdk_http::GatewayHttpProxy;
use std::collections::HashMap;
use std::sync::Arc;

/// Transfer all shards command (burn-all mode: loops forever, drains wallet balances).
pub struct TransferAllShardsCommand {
    pub wallets_dir: String,
    pub network_config: NetworkConfig,
    pub amount: u128,
    pub relayer: Option<String>,
    pub random_relayer: bool,
    pub batch_size: usize,
    pub sleep_time: u64,
    pub sign_threads: usize,
    pub send_parallelism: usize,
    pub gas_price: u64,
    pub no_tui: bool,
    pub verbose: bool,
}

#[async_trait]
impl Command for TransferAllShardsCommand {
    async fn execute(&self) -> Result<()> {
        let mut first_run = true;
        let client = reqwest::Client::new();

        loop {
            let proxy0 = GatewayHttpProxy::new(self.network_config.shard_url(0));
            let proxy1 = GatewayHttpProxy::new(self.network_config.shard_url(1));
            let proxy2 = GatewayHttpProxy::new(self.network_config.shard_url(2));

            println!("\n{}", "=".repeat(60));
            println!("🔥 INITIATING MULTISHARD BURN-ALL 🔥");
            println!("Mode: burn-all — draining wallet balances across ALL active shards");
            println!("{}\n", "=".repeat(60));

            let mut wallet_manager = WalletManager::new(&self.wallets_dir);
            wallet_manager.load_wallets()?;

            let s0 = wallet_manager.get_wallets_by_shard(0).to_vec();
            let s1 = wallet_manager.get_wallets_by_shard(1).to_vec();
            let s2 = wallet_manager.get_wallets_by_shard(2).to_vec();

            let (r0, r1, r2) = tokio::join!(
                super::generate_shard_txs_burn_all(&proxy0, "SHARD 0", &s0, &s0, 0, self.amount, self.relayer.as_deref(), self.random_relayer, self.gas_price),
                super::generate_shard_txs_burn_all(&proxy1, "SHARD 1", &s1, &s1, 1, self.amount, self.relayer.as_deref(), self.random_relayer, self.gas_price),
                super::generate_shard_txs_burn_all(&proxy2, "SHARD 2", &s2, &s2, 2, self.amount, self.relayer.as_deref(), self.random_relayer, self.gas_price),
            );

            let mut queues_by_shard: HashMap<u8, Vec<WalletQueue>> = HashMap::new();
            for (shard, result) in [(0u8, r0), (1, r1), (2, r2)] {
                match result {
                    Ok(queues) => { queues_by_shard.insert(shard, queues); }
                    Err(e) => { println!("⚠️ [SHARD {}] Failed to generate txs: {} — shard skipped.", shard, e); }
                }
            }

            for queues in queues_by_shard.values_mut() {
                super::assign_gas_price(queues, self.gas_price);
            }

            if first_run {
                wait_for_user_confirmation();
                first_run = false;
            } else {
                println!("🔄 Restarting blast directly...");
            }

            let total_planned = 0u64;
            let title = "TransferAllShards - All Shards (burn-all)".to_string();

            let network_config = self.network_config.clone();
            let batch_size = self.batch_size;
            let sleep_time = self.sleep_time;
            let sign_threads = self.sign_threads;
            let send_parallelism = self.send_parallelism;
            let no_tui = self.no_tui;
            let verbose = self.verbose;
            let client_for_run = client.clone();

            let result = tui::run_with_optional_tui(title, total_planned, no_tui, move |stats, log_handle: Option<tui::app::AppLogHandle>| async move {
                stats.burn_all_mode.store(true, std::sync::atomic::Ordering::Relaxed);
                let client = client_for_run;
                let mut broadcast_handles = Vec::new();

                for (shard, shard_queues) in queues_by_shard {
                    if shard_queues.is_empty() {
                        continue;
                    }
                    let label = format!("SHARD {}", shard);
                    let url = network_config.shard_url(shard);
                    let client = client.clone();
                    let stats_clone = Arc::clone(&stats);
                    let log_handle_clone = log_handle.clone();

                    broadcast_handles.push(tokio::spawn(async move {
                        BroadcastHelper::new(url, client)
                            .broadcast_txs(
                                &label,
                                shard_queues,
                                BroadcastConfig { batch_size, sleep_time, sign_threads, send_parallelism, verbose },
                                Some(stats_clone),
                                log_handle_clone,
                            )
                            .await;
                    }));
                }

                for handle in broadcast_handles {
                    if let Err(e) = handle.await {
                        println!("⚠️ A shard broadcaster thread failed with: {}", e);
                    }
                }

                Ok(())
            })
            .await?;

            if result != RunResult::Restart {
                return Ok(());
            }
        }
    }
}
