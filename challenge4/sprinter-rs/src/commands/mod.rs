//! Commands module for CLI command implementations.

use anyhow::{bail, Result};
use async_trait::async_trait;
use rand::prelude::SliceRandom;
use std::collections::HashMap;

use crate::blockchain::transaction::{BroadcastConfig, BroadcastHelper};
use crate::tui::{self, RunResult};
use crate::wallet::{find_relayer_account, RelayedTransaction, WalletEntry, WalletQueue};
use std::sync::Arc;

pub mod call_contract;
pub mod challenge4;
pub mod check_wallets;
pub mod collect;
pub mod create_wallets;
pub mod deploy_contract;
pub mod fund;
pub mod swap_dex;
pub mod transfer_all_cross_shards;
pub mod transfer_all_shards;
pub mod transfer_cross_shard;
pub mod transfer_intrashard;

pub use call_contract::CallContractCommand;
pub use collect::CollectCommand;
pub use challenge4::{Challenge4DeployCommand, Challenge4DrainCommand, Challenge4MeasureGasCommand, Challenge4SpamCommand, Challenge4WrapCommand};
pub use check_wallets::CheckWalletsCommand;
pub use create_wallets::CreateWalletsCommand;
pub use deploy_contract::DeployContractCommand;
pub use fund::FundCommand;
pub use swap_dex::SwapDexCommand;
pub use transfer_all_cross_shards::TransferAllCrossShardsCommand;
pub use transfer_all_shards::TransferAllShardsCommand;
pub use transfer_cross_shard::TransferCrossShardCommand;
pub use transfer_intrashard::TransferIntrashardCommand;

/// Trait for CLI commands.
#[async_trait]
pub trait Command {
    /// Execute the command.
    async fn execute(&self) -> Result<()>;
}

pub fn generate_unsigned_txs(
    config: &multiversx_sdk::data::network_config::NetworkConfig,
    label: &str,
    sender_wallets: &[Arc<WalletEntry>],
    receiver_wallets: &[Arc<WalletEntry>],
    amount: u128,
    total_txs_per_wallet: usize,
    relayer_account: Option<&Arc<WalletEntry>>,
    random_relayer: bool,
    sender_to_eligible_relayers: &HashMap<String, Vec<Arc<WalletEntry>>>,
    gas_price: u64,
    ping_pong: bool,
) -> Vec<WalletQueue> {
    let total_txs = sender_wallets.len() * total_txs_per_wallet;
    println!(
        "[{}] Pre-generating {} unsigned transactions in memory...",
        label, total_txs
    );

    let mut rng = rand::thread_rng();
    let mut queues: Vec<WalletQueue> = Vec::with_capacity(sender_wallets.len());

    for (sender_idx, sender) in sender_wallets.iter().enumerate() {
        let mut queue = WalletQueue::new(sender.clone());
        queue.pending.reserve(total_txs_per_wallet);

        for k in 0..total_txs_per_wallet {
            let receiver = if ping_pong {
                let is_intrashard = std::ptr::eq(sender_wallets.as_ptr(), receiver_wallets.as_ptr());
                let receiver_idx = if is_intrashard {
                    if receiver_wallets.len() > 1 {
                        (sender_idx + 1 + (k % (receiver_wallets.len() - 1))) % receiver_wallets.len()
                    } else {
                        0
                    }
                } else {
                    let global_tx_idx = k * sender_wallets.len() + sender_idx;
                    global_tx_idx % receiver_wallets.len()
                };
                &receiver_wallets[receiver_idx]
            } else {
                receiver_wallets.choose(&mut rng).unwrap()
            };

            let relayer: Option<Arc<WalletEntry>> = if let Some(rel) = relayer_account {
                Some(rel.clone())
            } else if random_relayer {
                let eligible = sender_to_eligible_relayers
                    .get(&sender.public_key_hex())
                    .unwrap();
                Some(eligible.choose(&mut rng).unwrap().clone())
            } else {
                None
            };

            let tx = RelayedTransaction::from_parts(
                sender.get_nonce_then_increment(),
                amount,
                &receiver.bech32,
                &sender.bech32,
                gas_price,
                50_000,
                &config.chain_id,
                config.min_transaction_version,
                relayer.as_deref().map(|r| &r.bech32),
            );

            queue.push(tx, relayer);
        }

        // Store the first tx as a template for "run until done" refill and set target.
        if let Some((first_tx, first_relayer)) = queue.pending.front() {
            queue.tx_template = Some((first_tx.clone(), first_relayer.clone()));
        }
        queue.target = total_txs_per_wallet;

        queues.push(queue);
    }

    println!("[{}] Created {} unsigned transactions.", label, total_txs);

    queues
}

/// Build relayer lookup structures from command parameters.
/// Returns the fixed relayer account (if any) and the per-sender eligible-relayer map
/// (populated only when `random_relayer` is true).
pub fn build_relayer_config(
    relayer_addr: Option<&str>,
    random_relayer: bool,
    shard_wallets: &[Arc<WalletEntry>],
    shard: u8,
) -> Result<(Option<Arc<WalletEntry>>, HashMap<String, Vec<Arc<WalletEntry>>>)> {
    let relayer_account = if let Some(addr) = relayer_addr {
        Some(find_relayer_account(addr, shard_wallets, shard)?)
    } else {
        None
    };

    let mut sender_to_eligible_relayers: HashMap<String, Vec<Arc<WalletEntry>>> = HashMap::new();
    if random_relayer {
        if shard_wallets.len() < 2 {
            bail!("Not enough wallets to randomly pick a relayer that is not the sender.");
        }
        for sender in shard_wallets {
            let sender_key = sender.public_key_hex();
            let eligible: Vec<Arc<WalletEntry>> = shard_wallets
                .iter()
                .filter(|w| w.public_key_hex() != sender_key)
                .cloned()
                .collect();
            sender_to_eligible_relayers.insert(sender_key, eligible);
        }
    }

    Ok((relayer_account, sender_to_eligible_relayers))
}

/// Generate transactions for a shard pair.
/// For intrashard: pass `sender_wallets` as both sender and receiver.
/// For cross-shard: pass separate slices.
/// `sender_shard` is used for relayer lookup.
pub async fn generate_shard_txs(
    proxy: &multiversx_sdk_http::GatewayHttpProxy,
    label: &str,
    sender_wallets: &[Arc<WalletEntry>],
    receiver_wallets: &[Arc<WalletEntry>],
    sender_shard: u8,
    amount: u128,
    total_txs_per_wallet: usize,
    relayer: Option<&str>,
    random_relayer: bool,
    gas_price: u64,
    ping_pong: bool,
) -> Result<Vec<WalletQueue>> {
    if sender_wallets.is_empty() || receiver_wallets.is_empty() {
        println!("[{label}] Skipping: no wallets.");
        return Ok(Vec::new());
    }

    if sender_wallets.len() == receiver_wallets.len()
        && std::ptr::eq(sender_wallets.as_ptr(), receiver_wallets.as_ptr())
    {
        println!(
            "[{label}] {} wallet(s). Target: {} tx/wallet.",
            sender_wallets.len(),
            total_txs_per_wallet
        );
    } else {
        println!(
            "[{label}] {} sender(s), {} receiver(s). Target: {} tx/wallet.",
            sender_wallets.len(),
            receiver_wallets.len(),
            total_txs_per_wallet
        );
    }

    if random_relayer {
        println!("[{label}] Using random relayer.");
    }

    let (relayer_account, sender_to_eligible_relayers) =
        build_relayer_config(relayer, random_relayer, sender_wallets, sender_shard)?;

    println!("[{label}] Syncing wallet nonces...");
    crate::blockchain::nonce::NonceTracker::sync_nonces(proxy, sender_wallets).await?;

    let config = proxy.http_request(multiversx_sdk::gateway::NetworkConfigRequest).await?;

    Ok(generate_unsigned_txs(
        &config,
        label,
        sender_wallets,
        receiver_wallets,
        amount,
        total_txs_per_wallet,
        relayer_account.as_ref(),
        random_relayer,
        &sender_to_eligible_relayers,
        gas_price,
        ping_pong,
    ))
}

/// Set up burn-all queues with no pre-generated txs. All generation happens on the
/// fly in the broadcast loop via refill_and_trim as soon as a wallet has balance.
/// Works correctly with unequal starting balances across shards.
pub fn generate_burn_all_txs(
    config: &multiversx_sdk::data::network_config::NetworkConfig,
    label: &str,
    sender_wallets: &[Arc<WalletEntry>],
    receiver_wallets: &[Arc<WalletEntry>],
    balances: &[u128],
    amount: u128,
    relayer_account: Option<&Arc<WalletEntry>>,
    random_relayer: bool,
    sender_to_eligible_relayers: &HashMap<String, Vec<Arc<WalletEntry>>>,
    gas_price: u64,
) -> Vec<WalletQueue> {
    const GAS_LIMIT: u64 = 50_000;
    let n_receivers = receiver_wallets.len();
    let mut rng = rand::thread_rng();
    let mut queues: Vec<WalletQueue> = Vec::with_capacity(sender_wallets.len());

    for (sender_idx, (sender, &balance)) in sender_wallets.iter().zip(balances.iter()).enumerate() {
        let mut queue = WalletQueue::new(sender.clone());
        queue.egld_balance = balance;
        queue.burn_all = true;
        queue.target = usize::MAX;

        if n_receivers > 0 {
            // Fixed bilateral pair: (i ^ 1) % n — no self-send for odd wallet counts.
            let receiver = &receiver_wallets[(sender_idx ^ 1) % n_receivers];

            let relayer: Option<Arc<WalletEntry>> = if let Some(rel) = relayer_account {
                Some(rel.clone())
            } else if random_relayer {
                let eligible = sender_to_eligible_relayers.get(&sender.public_key_hex()).unwrap();
                Some(eligible.choose(&mut rng).unwrap().clone())
            } else {
                None
            };

            // Template tx — nonce is a placeholder, always overwritten on refill.
            let tx = RelayedTransaction::from_parts(
                sender.get_nonce_then_increment(),
                amount,
                &receiver.bech32,
                &sender.bech32,
                gas_price,
                GAS_LIMIT,
                &config.chain_id,
                config.min_transaction_version,
                relayer.as_deref().map(|r| &r.bech32),
            );
            queue.tx_template = Some((tx, relayer.clone()));
        }

        queues.push(queue);
    }

    println!("[{}] Burn-all: {} wallet(s) ready (on-the-fly).", label, queues.len());
    queues
}

/// Fetch nonces+balances and generate burn-all queues for one shard.
pub async fn generate_shard_txs_burn_all(
    proxy: &multiversx_sdk_http::GatewayHttpProxy,
    label: &str,
    sender_wallets: &[Arc<WalletEntry>],
    receiver_wallets: &[Arc<WalletEntry>],
    sender_shard: u8,
    amount: u128,
    relayer: Option<&str>,
    random_relayer: bool,
    gas_price: u64,
) -> Result<Vec<WalletQueue>> {
    if sender_wallets.is_empty() || receiver_wallets.is_empty() {
        println!("[{label}] Skipping: no wallets.");
        return Ok(Vec::new());
    }

    println!("[{label}] {} wallet(s) — burn-all mode.", sender_wallets.len());

    let (relayer_account, sender_to_eligible_relayers) =
        build_relayer_config(relayer, random_relayer, sender_wallets, sender_shard)?;

    println!("[{label}] Syncing wallet nonces and balances...");
    let balances = crate::blockchain::nonce::NonceTracker::sync_nonces(proxy, sender_wallets).await?;
    let config = proxy.http_request(multiversx_sdk::gateway::NetworkConfigRequest).await?;

    Ok(generate_burn_all_txs(
        &config,
        label,
        sender_wallets,
        receiver_wallets,
        &balances,
        amount,
        relayer_account.as_ref(),
        random_relayer,
        &sender_to_eligible_relayers,
        gas_price,
    ))
}

pub fn assign_gas_price(queues: &mut Vec<WalletQueue>, gas_price: u64) {
    for queue in queues.iter_mut() {
        for (tx, _) in queue.pending.iter_mut() {
            tx.gas_price = gas_price;
        }
    }
}

/// Run a single-queue broadcast inside the optional TUI.
/// Returns `RunResult::Restart` if the user requested a restart.
pub async fn broadcast_queues(
    title: String,
    label: String,
    queues: Vec<WalletQueue>,
    url: String,
    client: reqwest::Client,
    config: BroadcastConfig,
    no_tui: bool,
) -> Result<RunResult> {
    let total_planned: u64 = queues.iter().map(|q| q.pending.len() as u64).sum();
    tui::run_with_optional_tui(title, total_planned, no_tui, move |stats, log_handle| async move {
        BroadcastHelper::new(url, client)
            .broadcast_txs(&label, queues, config, Some(stats), log_handle)
            .await;
        Ok(())
    })
    .await
}
