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
use std::sync::Arc;

/// All 6 cross-shard pairs for 3 shards.
const CROSS_SHARD_PAIRS: [(u8, u8); 6] = [
    (0, 1),
    (0, 2),
    (1, 0),
    (1, 2),
    (2, 0),
    (2, 1),
];

/// Transfer all cross-shards command (burn-all mode: loops forever, drains wallet balances).
pub struct TransferAllCrossShardsCommand {
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
impl Command for TransferAllCrossShardsCommand {
    async fn execute(&self) -> Result<()> {
        let mut first_run = true;
        let client = reqwest::Client::new();

        loop {

            println!("\n{}", "=".repeat(60));
            println!("🔥 INITIATING ALL CROSS-SHARD BURN-ALL 🔥");
            println!("Mode: burn-all — draining wallet balances across all 6 cross-shard pairs");
            println!("{}\n", "=".repeat(60));

            let mut wallet_manager = WalletManager::new(&self.wallets_dir);
            wallet_manager.load_wallets()?;

            let s0 = wallet_manager.get_wallets_by_shard(0).to_vec();
            let s1 = wallet_manager.get_wallets_by_shard(1).to_vec();
            let s2 = wallet_manager.get_wallets_by_shard(2).to_vec();

            let shards = [s0, s1, s2];

            // For each source shard: sync nonces ONCE, then generate burn-all txs for both
            // destination shards. Budget is split evenly: affordable_per_dst = balance / cost / 2.
            // Spawn one task per source shard for parallelism.
            let mut shard_handles = Vec::new();
            for src_shard in 0u8..3 {
                let src_wallets = shards[src_shard as usize].clone();
                let shards_clone = shards.clone();
                let proxy_url = self.network_config.shard_url(src_shard);
                let relayer = self.relayer.clone();
                let amount = self.amount;
                let random_relayer = self.random_relayer;
                let gas_price = self.gas_price;

                // Destination shards for this source.
                let dsts: Vec<u8> = CROSS_SHARD_PAIRS
                    .iter()
                    .filter(|(s, _)| *s == src_shard)
                    .map(|(_, d)| *d)
                    .collect();

                shard_handles.push(tokio::spawn(async move {
                    let proxy = GatewayHttpProxy::new(proxy_url.clone());

                    if src_wallets.is_empty() {
                        return Ok::<Vec<WalletQueue>, anyhow::Error>(Vec::new());
                    }

                    println!("[S{src_shard}] Syncing wallet nonces and balances...");
                    let balances = crate::blockchain::nonce::NonceTracker::sync_nonces(&proxy, &src_wallets).await?;

                    let config = proxy
                        .http_request(multiversx_sdk::gateway::NetworkConfigRequest)
                        .await?;

                    let (relayer_account, sender_to_eligible_relayers) =
                        super::build_relayer_config(
                            relayer.as_deref(),
                            random_relayer,
                            &src_wallets,
                            src_shard,
                        )?;

                    const GAS_LIMIT: u64 = 50_000;
                    let cost_per_tx = amount + GAS_LIMIT as u128 * gas_price as u128;

                    // Generate burn-all txs for each destination sequentially.
                    // Nonces auto-increment across calls since wallets share the atomic counter.
                    let mut queues_per_dst: Vec<Vec<WalletQueue>> = Vec::new();
                    for &dst in &dsts {
                        let dst_wallets = &shards_clone[dst as usize];
                        let label = format!("S{src_shard}->S{dst}");

                        // Split budget: each dst gets half the affordable txs.
                        let split_balances: Vec<u128> = balances
                            .iter()
                            .map(|&b| if cost_per_tx > 0 { (b / cost_per_tx / 2) * cost_per_tx } else { 0 })
                            .collect();

                        let queues = super::generate_burn_all_txs(
                            &config,
                            &label,
                            &src_wallets,
                            dst_wallets,
                            &split_balances,
                            amount,
                            relayer_account.as_ref(),
                            random_relayer,
                            &sender_to_eligible_relayers,
                            gas_price,
                        );
                        queues_per_dst.push(queues);
                    }

                    // Merge: interleave pending txs from the two destination queues into a
                    // single queue per wallet. Re-number nonces after interleaving.
                    let mut queues_a = queues_per_dst.remove(0);
                    let mut queues_b = queues_per_dst.remove(0);
                    let mut merged: Vec<WalletQueue> = Vec::with_capacity(queues_a.len());

                    for (qa, qb) in queues_a.iter_mut().zip(queues_b.iter_mut()) {
                        let mut mq = WalletQueue::new(qa.sender.clone());
                        mq.egld_balance = qa.egld_balance;
                        mq.burn_all = true;
                        mq.target = usize::MAX;

                        // Interleave: one from A, one from B, repeat.
                        loop {
                            match (qa.pending.pop_front(), qb.pending.pop_front()) {
                                (Some(a), Some(b)) => {
                                    mq.pending.push_back(a);
                                    mq.pending.push_back(b);
                                }
                                (Some(a), None) => mq.pending.push_back(a),
                                (None, Some(b)) => mq.pending.push_back(b),
                                (None, None) => break,
                            }
                        }

                        // Re-number nonces after interleaving.
                        if let Some((first_tx, _)) = mq.pending.front() {
                            let base_nonce = first_tx.nonce;
                            for (i, (tx, _)) in mq.pending.iter_mut().enumerate() {
                                tx.nonce = base_nonce + i as u64;
                                tx.clear_signatures();
                            }
                        }

                        // Set both templates so refill alternates between dst A and dst B,
                        // keeping both destination shards funded and all 6 pairs active.
                        mq.tx_template = qa.tx_template.take()
                            .or_else(|| mq.pending.front().map(|(tx, rel)| (tx.clone(), rel.clone())));
                        mq.tx_template_b = qb.tx_template.take()
                            .or_else(|| mq.pending.get(1).map(|(tx, rel)| (tx.clone(), rel.clone())));

                        merged.push(mq);
                    }

                    println!("[S{src_shard}->cross] Merged {} wallets, {} initial txs.",
                        merged.len(),
                        merged.iter().map(|q| q.pending.len()).sum::<usize>());

                    Ok::<Vec<WalletQueue>, anyhow::Error>(merged)
                }));
            }

            let mut merged_by_src: [Vec<WalletQueue>; 3] = [Vec::new(), Vec::new(), Vec::new()];
            for (src_shard, handle) in shard_handles.into_iter().enumerate() {
                match handle.await? {
                    Ok(queues) => merged_by_src[src_shard] = queues,
                    Err(e) => println!("⚠️ [S{src_shard}] Failed to generate txs: {e}"),
                }
            }

            for queues in merged_by_src.iter_mut() {
                super::assign_gas_price(queues, self.gas_price);
            }

            if first_run {
                wait_for_user_confirmation();
                first_run = false;
            } else {
                println!("🔄 Restarting blast directly...");
            }

            let total_planned = 0u64;
            let title = "TransferAllCrossShards - All 6 Pairs (burn-all)".to_string();

            let network_config = self.network_config.clone();
            let batch_size = self.batch_size;
            let sleep_time = self.sleep_time;
            let sign_threads = self.sign_threads;
            let send_parallelism = self.send_parallelism;
            let no_tui = self.no_tui;
            let verbose = self.verbose;
            let client_for_run = client.clone();

            let run_result = tui::run_with_optional_tui(
                title,
                total_planned,
                no_tui,
                move |stats, log_handle: Option<tui::app::AppLogHandle>| async move {
                    stats.burn_all_mode.store(true, std::sync::atomic::Ordering::Relaxed);
                    let client = client_for_run;
                    let mut broadcast_handles = Vec::new();

                    // Broadcast one task per source shard (3 tasks total).
                    for (src_shard, queues) in merged_by_src.into_iter().enumerate() {
                        if queues.is_empty() {
                            continue;
                        }
                        let label = format!("S{src_shard}->cross");
                        let url = network_config.shard_url(src_shard as u8);
                        let client = client.clone();
                        let stats_clone = Arc::clone(&stats);
                        let log_handle_clone = log_handle.clone();

                        broadcast_handles.push(tokio::spawn(async move {
                            BroadcastHelper::new(url, client)
                                .broadcast_txs(
                                    &label,
                                    queues.into_iter().collect(),
                                    BroadcastConfig { batch_size, sleep_time, sign_threads, send_parallelism, verbose },
                                    Some(stats_clone),
                                    log_handle_clone,
                                )
                                .await;
                        }));
                    }

                    for handle in broadcast_handles {
                        if let Err(e) = handle.await {
                            println!("⚠️ A pair broadcaster thread failed with: {}", e);
                        }
                    }

                    Ok(())
                },
            )
            .await?;

            if run_result != RunResult::Restart {
                return Ok(());
            }
        }
    }
}
