use crate::blockchain::nonce::NonceTracker;
use crate::blockchain::transaction::BroadcastConfig;
use crate::commands::Command;
use crate::network_config::NetworkConfig;
use crate::wallet::{RelayedTransaction, WalletManager, WalletQueue};
use anyhow::Result;
use async_trait::async_trait;
use multiversx_chain_core::{std::Bech32Address, types::Address};
use multiversx_sdk::gateway::NetworkConfigRequest;
use multiversx_sdk_http::GatewayHttpProxy;

pub struct DeployContractCommand {
    pub wallets_dir: String,
    pub network_config: NetworkConfig,
    pub shard: u8,
    pub wasm_path: String,
    pub args: Vec<String>,
    pub gas_limit: u64,
    pub gas_price: u64,
    pub no_tui: bool,
    pub verbose: bool,
}

#[async_trait]
impl Command for DeployContractCommand {
    async fn execute(&self) -> Result<()> {
        let client = reqwest::Client::new();
        let proxy = GatewayHttpProxy::new(self.network_config.proxy.clone());

        let mut wallet_manager = WalletManager::new(&self.wallets_dir);
        wallet_manager.load_wallets()?;
        let shard_wallets = wallet_manager.get_wallets_by_shard(self.shard).to_vec();

        if shard_wallets.is_empty() {
            println!("[SHARD {}] Error: No wallets found.", self.shard);
            return Ok(());
        }

        println!("[SHARD {}] Loaded {} wallet(s) for deployment", self.shard, shard_wallets.len());
        println!("WASM Path: {}", self.wasm_path);

        println!("[SHARD {}] Syncing wallet nonces...", self.shard);
        NonceTracker::sync_nonces(&proxy, &shard_wallets).await?;

        let config = proxy.http_request(NetworkConfigRequest).await?;

        // SC deployment zero address (32 zero bytes) — constructed from raw bytes to
        // bypass the strict bech32 checksum validation in multiversx-chain-core 0.22.x.
        let contract_addr = Bech32Address::from(Address::new([0u8; 32]));
        
        // Read WASM file
        let wasm_bytes = std::fs::read(&self.wasm_path)?;
        let data = build_deploy_data(&wasm_bytes, &self.args);

        println!("[SHARD {}] Generating 1 deployment transaction per wallet...", self.shard);

        let mut queues: Vec<WalletQueue> = Vec::with_capacity(shard_wallets.len());
        for sender in &shard_wallets {
            let mut queue = WalletQueue::new(sender.clone());
            let mut tx = RelayedTransaction::from_parts(
                sender.get_nonce_then_increment(),
                0,
                &contract_addr,
                &sender.bech32,
                self.gas_price,
                self.gas_limit,
                &config.chain_id,
                config.min_transaction_version,
                None, // No relayer for deployment
            );
            tx.data = Some(data.clone());
            queue.push(tx, None);
            if let Some((first_tx, first_relayer)) = queue.pending.front() {
                queue.tx_template = Some((first_tx.clone(), first_relayer.clone()));
            }
            queue.target = 1;
            queues.push(queue);
        }

        super::assign_gas_price(&mut queues, self.gas_price);

        let _ = super::broadcast_queues(
            format!("DeployContract @ Shard {}", self.shard),
            format!("SHARD {}", self.shard),
            queues,
            self.network_config.shard_url(self.shard),
            client.clone(),
            BroadcastConfig { batch_size: 99, sleep_time: 0, sign_threads: 0, send_parallelism: 1, verbose: self.verbose, cross_shard: false },
            self.no_tui,
        ).await?;

        Ok(())
    }
}

fn build_deploy_data(wasm_bytes: &[u8], args: &[String]) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let wasm_hex = hex::encode(wasm_bytes);
    let mut parts = vec![
        wasm_hex,
        "0500".to_string(), // VM type
        "0102".to_string(), // Code Metadata: Upgradeable (0x01) + Payable (0x02)
    ];
    parts.extend_from_slice(args);
    let raw = parts.join("@");
    STANDARD.encode(raw.as_bytes())
}
