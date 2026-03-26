use crate::blockchain::nonce::NonceTracker;
use crate::blockchain::transaction::BroadcastConfig;
use crate::commands::Command;
use crate::network_config::NetworkConfig;
use crate::tui::RunResult;
use crate::utils::wait_for_user_confirmation;
use crate::wallet::{RelayedTransaction, WalletManager, WalletQueue};
use anyhow::Result;
use async_trait::async_trait;
use multiversx_chain_core::std::Bech32Address;
use multiversx_sdk::gateway::NetworkConfigRequest;
use multiversx_sdk_http::GatewayHttpProxy;
use rand::prelude::SliceRandom;

pub struct CallContractCommand {
    pub wallets_dir: String,
    pub network_config: NetworkConfig,
    pub shard: u8,
    pub contract: String,
    pub function: String,
    /// Raw hex-encoded arguments, comma-separated (e.g. "token_hex,amount_hex")
    pub args: Vec<String>,
    /// Optional ESDT token identifier to attach as ESDTTransfer
    pub token: Option<String>,
    /// Amount for ESDTTransfer (ignored if token is None)
    pub token_amount: u128,
    pub gas_limit: u64,
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

#[async_trait]
impl Command for CallContractCommand {
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

            println!("[SHARD {}] Loaded {} wallet(s)", self.shard, shard_wallets.len());
            println!("Contract: {}", self.contract);
            println!("Function: {}", self.function);

            let (relayer_account, sender_to_eligible_relayers) =
                super::build_relayer_config(self.relayer.as_deref(), self.random_relayer, &shard_wallets, self.shard)?;

            println!("[SHARD {}] Syncing wallet nonces...", self.shard);
            NonceTracker::sync_nonces(&proxy, &shard_wallets).await?;

            let config = proxy.http_request(NetworkConfigRequest).await?;

            let contract_addr = Bech32Address::from_bech32_string(self.contract.clone());

            let data = build_data(self.token.as_deref(), self.token_amount, &self.function, &self.args);

            let total_txs = shard_wallets.len() * self.total_txs_per_wallet;
            println!("[SHARD {}] Pre-generating {} transactions...", self.shard, total_txs);

            let mut queues: Vec<WalletQueue> = {
                let mut rng = rand::thread_rng();
                let mut queues: Vec<WalletQueue> = Vec::with_capacity(shard_wallets.len());
                for sender in &shard_wallets {
                    let mut queue = WalletQueue::new(sender.clone());
                    for _ in 0..self.total_txs_per_wallet {
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
                            self.gas_limit,
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
                    queue.target = self.total_txs_per_wallet;
                    queues.push(queue);
                }
                queues
            };

            super::assign_gas_price(&mut queues, self.gas_price);

            if first_run {
                wait_for_user_confirmation();
                first_run = false;
            } else {
                println!("🔄 Restarting blast directly...");
            }

            let result = super::broadcast_queues(
                format!("CallContract - {} @ Shard {}", self.function, self.shard),
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

/// Build the transaction data field (base64-encoded, as required by the MultiversX gateway API).
/// With token: `ESDTTransfer@<token_hex>@<amount_hex>@<function_hex>@<args...>`
/// Without token: `<function_hex>@<args...>`
fn build_data(token: Option<&str>, token_amount: u128, function: &str, args: &[String]) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let fn_hex = hex::encode(function);
    let raw = if let Some(tok) = token {
        let tok_hex = hex::encode(tok);
        let amt_hex = super::swap_dex::hex_encode_u128(token_amount);
        let mut parts = vec![
            "ESDTTransfer".to_string(),
            tok_hex,
            amt_hex,
            fn_hex,
        ];
        parts.extend_from_slice(args);
        parts.join("@")
    } else {
        let mut parts = vec![fn_hex];
        parts.extend_from_slice(args);
        parts.join("@")
    };
    STANDARD.encode(raw.as_bytes())
}
