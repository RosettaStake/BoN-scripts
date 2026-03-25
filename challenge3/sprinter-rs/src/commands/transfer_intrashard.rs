use crate::blockchain::transaction::BroadcastConfig;
use crate::commands::Command;
use crate::network_config::NetworkConfig;
use crate::tui::RunResult;
use crate::utils::wait_for_user_confirmation;
use crate::wallet::WalletManager;
use anyhow::Result;
use async_trait::async_trait;
use multiversx_sdk_http::GatewayHttpProxy;

/// Transfer intrashard command.
pub struct TransferIntrashardCommand {
    pub wallets_dir: String,
    pub network_config: NetworkConfig,
    pub shard: u8,
    pub amount: u128,
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
    pub ping_pong: bool,
}

#[async_trait]
impl Command for TransferIntrashardCommand {
    async fn execute(&self) -> Result<()> {
        if self.shard > 2 {
            return Ok(());
        }

        let mut first_run = true;
        let client = reqwest::Client::new();

        loop {
            let proxy = GatewayHttpProxy::new(self.network_config.proxy.clone());

            let mut wallet_manager = WalletManager::new(&self.wallets_dir);
            wallet_manager.load_wallets()?;
            let shard_wallets = wallet_manager.get_wallets_by_shard(self.shard).to_vec();

            let mut queues = super::generate_shard_txs(
                &proxy,
                &format!("SHARD {}", self.shard),
                &shard_wallets,
                &shard_wallets,
                self.shard,
                self.amount,
                self.total_txs_per_wallet,
                self.relayer.as_deref(),
                self.random_relayer,
                self.gas_price,
                self.ping_pong,
            )
            .await?;

            if queues.is_empty() {
                return Ok(());
            }

            super::assign_gas_price(&mut queues, self.gas_price);

            if first_run {
                wait_for_user_confirmation();
                first_run = false;
            } else {
                println!("🔄 Restarting blast directly...");
            }

            let result = super::broadcast_queues(
                format!("TransferIntrashard - Shard {}", self.shard),
                format!("SHARD {}", self.shard),
                queues,
                self.network_config.shard_url(self.shard),
                client.clone(),
                BroadcastConfig { batch_size: self.batch_size, sleep_time: self.sleep_time, sign_threads: self.sign_threads, send_parallelism: self.send_parallelism, verbose: self.verbose },
                self.no_tui,
            ).await?;

            if result != RunResult::Restart {
                return Ok(());
            }
        }
    }
}
