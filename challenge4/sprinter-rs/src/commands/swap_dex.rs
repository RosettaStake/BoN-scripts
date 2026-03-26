use crate::blockchain::nonce::NonceTracker;
use crate::blockchain::transaction::BroadcastConfig;
use crate::commands::Command;
use crate::network_config::NetworkConfig;
use crate::tui::RunResult;
use crate::utils::wait_for_user_confirmation;
use crate::wallet::{RelayedTransaction, WalletManager, WalletQueue};
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use multiversx_chain_core::std::Bech32Address;
use multiversx_sdk::gateway::NetworkConfigRequest;
use multiversx_sdk_http::GatewayHttpProxy;
use rand::prelude::SliceRandom;

pub struct SwapDexCommand {
    pub wallets_dir: String,
    pub network_config: NetworkConfig,
    pub shard: u8,
    pub contract: String,
    pub token_in: String,
    pub amount_in: u128,
    pub token_out: String,
    pub amount_out_min: u128,
    /// If true, fetch each wallet's token_in balance and swap all of it (1 tx per wallet)
    pub swap_all: bool,
    pub relayer: Option<String>,
    pub random_relayer: bool,
    pub total_txs_per_wallet: usize,
    pub batch_size: usize,
    pub sleep_time: u64,
    pub sign_threads: usize,
    pub send_parallelism: usize,
    pub gas_price: u64,
    pub no_tui: bool,
    pub verbose: bool,
}

const GAS_LIMIT: u64 = 15_000_000;

#[async_trait]
impl Command for SwapDexCommand {
    async fn execute(&self) -> Result<()> {
        let mut first_run = true;
        let client = reqwest::Client::new();

        loop {
            let proxy = GatewayHttpProxy::new(self.network_config.proxy.clone());

            let mut wallet_manager = WalletManager::new(&self.wallets_dir);
            wallet_manager.load_wallets()?;
            let shard_wallets = wallet_manager.get_wallets_by_shard(self.shard).to_vec();

            if shard_wallets.is_empty() {
                println!("[SHARD {}] Error: No wallets found.", self.shard);
                return Ok(());
            }

            println!("Executing swap from {} wallet(s) in Shard {}", shard_wallets.len(), self.shard);
            println!("Contract: {}", self.contract);
            let amt_str = if self.swap_all { "MAX".to_string() } else { self.amount_in.to_string() };
            println!("Input: {} of {}", amt_str, self.token_in);
            println!("Minimum Output: {} of {}", self.amount_out_min, self.token_out);

            let (relayer_account, sender_to_eligible_relayers) =
                super::build_relayer_config(self.relayer.as_deref(), self.random_relayer, &shard_wallets, self.shard)?;

            println!("Syncing wallet nonces...");
            NonceTracker::sync_nonces(&proxy, &shard_wallets).await?;

            let config = proxy.http_request(NetworkConfigRequest).await?;
            let contract_addr = Bech32Address::from_bech32_string(self.contract.clone());

            // Fetch per-wallet balances if swap_all
            let wallet_amounts: Vec<u128> = if self.swap_all {
                let proxy_url = self.network_config.proxy.clone();
                let token_in = self.token_in.clone();
                let wallet_bech32s: Vec<String> = shard_wallets.iter()
                    .map(|w| w.bech32.to_bech32_string())
                    .collect();
                futures::stream::iter(wallet_bech32s.into_iter())
                    .map(|bech32| {
                        let client = client.clone();
                        let proxy_url = proxy_url.clone();
                        let token_in = token_in.clone();
                        async move { fetch_esdt_balance(&client, &proxy_url, &bech32, &token_in).await }
                    })
                    .buffer_unordered(20)
                    .collect()
                    .await
            } else {
                vec![self.amount_in; shard_wallets.len()]
            };

            let txs_per_wallet = if self.swap_all { 1 } else { self.total_txs_per_wallet };

            let queues: Vec<WalletQueue> = {
                let mut rng = rand::thread_rng();
                let mut queues = Vec::with_capacity(shard_wallets.len());

                for (sender, &amount) in shard_wallets.iter().zip(wallet_amounts.iter()) {
                    if self.swap_all && amount == 0 {
                        println!("  Skipping {}: 0 balance for {}", sender.bech32, self.token_in);
                        continue;
                    }

                    let data = build_swap_data(&self.token_in, amount, &self.token_out, self.amount_out_min);
                    let mut queue = WalletQueue::new(sender.clone());

                    for _ in 0..txs_per_wallet {
                        let relayer = if let Some(ref rel) = relayer_account {
                            Some(rel.clone())
                        } else if self.random_relayer {
                            let eligible = sender_to_eligible_relayers.get(&sender.public_key_hex()).unwrap();
                            Some(eligible.choose(&mut rng).unwrap().clone())
                        } else {
                            None
                        };

                        let mut tx = RelayedTransaction::from_parts(
                            sender.get_nonce_then_increment(),
                            0,
                            &contract_addr,
                            &sender.bech32,
                            self.gas_price,
                            GAS_LIMIT,
                            &config.chain_id,
                            config.min_transaction_version,
                            relayer.as_deref().map(|r| &r.bech32),
                        );
                        tx.data = Some(data.clone());
                        queue.push(tx, relayer);
                    }
                    if let Some((first_tx, first_relayer)) = queue.pending.front() {
                        queue.tx_template = Some((first_tx.clone(), first_relayer.clone()));
                    }
                    queue.target = txs_per_wallet;
                    queues.push(queue);
                }
                queues
            };

            if queues.is_empty() {
                println!("No transactions to send.");
                return Ok(());
            }

            let mut queues = queues;
            super::assign_gas_price(&mut queues, self.gas_price);

            if first_run {
                wait_for_user_confirmation();
                first_run = false;
            } else {
                println!("🔄 Restarting blast directly...");
            }

            let result = super::broadcast_queues(
                format!("SwapDex - Shard {}", self.shard),
                format!("SHARD {}", self.shard),
                queues,
                self.network_config.shard_url(self.shard),
                client.clone(),
                BroadcastConfig { batch_size: self.batch_size, sleep_time: self.sleep_time, sign_threads: self.sign_threads, send_parallelism: self.send_parallelism, verbose: self.verbose, cross_shard: false },
                self.no_tui,
            ).await?;

            if result != RunResult::Restart {
                return Ok(());
            }
        }
    }
}

/// `ESDTTransfer@<token_in_hex>@<amount_hex>@swapTokensFixedInput_hex@<token_out_hex>@<amount_out_min_hex>`
/// Returns the data field base64-encoded, as required by the MultiversX gateway API and signing format.
fn build_swap_data(token_in: &str, amount_in: u128, token_out: &str, amount_out_min: u128) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let raw = [
        "ESDTTransfer".to_string(),
        hex::encode(token_in),
        hex_encode_u128(amount_in),
        hex::encode("swapTokensFixedInput"),
        hex::encode(token_out),
        hex_encode_u128(amount_out_min),
    ]
    .join("@");
    STANDARD.encode(raw.as_bytes())
}

/// Hex-encode a u128 as minimal big-endian bytes (always even-length, as MultiversX requires).
pub fn hex_encode_u128(v: u128) -> String {
    if v == 0 {
        return "00".to_string();
    }
    let bytes = v.to_be_bytes();
    let start = bytes.iter().position(|&b| b != 0).unwrap();
    hex::encode(&bytes[start..])
}

async fn fetch_esdt_balance(client: &reqwest::Client, network: &str, addr: &str, token: &str) -> u128 {
    let url = format!("{}/address/{}/esdt/{}", network, addr, token);
    let Ok(resp) = client.get(&url).send().await else { return 0 };
    let Ok(body) = resp.json::<serde_json::Value>().await else { return 0 };
    body.pointer("/data/tokenData/balance")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u128>().ok())
        .unwrap_or(0)
}
