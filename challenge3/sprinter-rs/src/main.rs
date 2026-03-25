use anyhow::Result;
use clap::Parser;

use sprinter::cli::{Cli, Commands};
use sprinter::commands::Command;
use sprinter::network_config::NetworkConfig;

fn init_log(log_file: Option<&str>, log_file_only: Option<&str>) -> Result<()> {
    if let Some(path) = log_file_only {
        sprinter::blockchain::transaction::init_log_file(path, true)?;
    } else if let Some(path) = log_file {
        sprinter::blockchain::transaction::init_log_file(path, false)?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Fund {
            wallets_dir,
            config,
            whale,
            amount,
        } => {
            sprinter::commands::FundCommand {
                wallets_dir,
                network_config: NetworkConfig::load(&config)?,
                whale,
                amount,
            }
            .execute()
            .await?;
        }
        Commands::TransferIntrashard { transfer, shard } => {
            sprinter::commands::TransferIntrashardCommand {
                wallets_dir: transfer.wallets_dir,
                network_config: NetworkConfig::load(&transfer.config)?,
                shard,
                amount: transfer.amount,
                relayer: transfer.relayer,
                random_relayer: transfer.random_relayer,
                total_txs_per_wallet: transfer.total_txs_per_wallet,
                batch_size: transfer.batch_size,
                sleep_time: transfer.sleep_time,
                sign_threads: transfer.sign_threads,
                send_parallelism: transfer.send_parallelism,
                gas_price: transfer.gas_price,
                no_tui: transfer.no_tui,
                verbose: transfer.verbose,
                ping_pong: transfer.ping_pong,
            }
            .execute()
            .await?;
        }
        Commands::TransferCrossShard {
            transfer,
            source_shard,
            destination_shard,
        } => {
            sprinter::commands::TransferCrossShardCommand {
                wallets_dir: transfer.wallets_dir,
                network_config: NetworkConfig::load(&transfer.config)?,
                source_shard,
                destination_shard,
                amount: transfer.amount,
                relayer: transfer.relayer,
                random_relayer: transfer.random_relayer,
                total_txs_per_wallet: transfer.total_txs_per_wallet,
                batch_size: transfer.batch_size,
                sleep_time: transfer.sleep_time,
                sign_threads: transfer.sign_threads,
                send_parallelism: transfer.send_parallelism,
                gas_price: transfer.gas_price,
                no_tui: transfer.no_tui,
                verbose: transfer.verbose,
                ping_pong: transfer.ping_pong,
            }
            .execute()
            .await?;
        }
        Commands::TransferAllCrossShards { transfer } => {
            init_log(transfer.log_file.as_deref(), transfer.log_file_only.as_deref())?;
            sprinter::commands::TransferAllCrossShardsCommand {
                wallets_dir: transfer.wallets_dir,
                network_config: NetworkConfig::load(&transfer.config)?,
                amount: transfer.amount,
                relayer: transfer.relayer,
                random_relayer: transfer.random_relayer,
                batch_size: transfer.batch_size,
                sleep_time: transfer.sleep_time,
                sign_threads: transfer.sign_threads,
                send_parallelism: transfer.send_parallelism,
                gas_price: transfer.gas_price,
                no_tui: transfer.no_tui,
                verbose: transfer.verbose,
            }
            .execute()
            .await?;
        }
        Commands::TransferAllShards { transfer } => {
            init_log(transfer.log_file.as_deref(), transfer.log_file_only.as_deref())?;
            sprinter::commands::TransferAllShardsCommand {
                wallets_dir: transfer.wallets_dir,
                network_config: NetworkConfig::load(&transfer.config)?,
                amount: transfer.amount,
                relayer: transfer.relayer,
                random_relayer: transfer.random_relayer,
                batch_size: transfer.batch_size,
                sleep_time: transfer.sleep_time,
                sign_threads: transfer.sign_threads,
                send_parallelism: transfer.send_parallelism,
                gas_price: transfer.gas_price,
                no_tui: transfer.no_tui,
                verbose: transfer.verbose,
            }
            .execute()
            .await?;
        }
        Commands::CheckWallets {
            wallets_dir,
            config,
        } => {
            sprinter::commands::CheckWalletsCommand {
                wallets_dir,
                network_config: NetworkConfig::load(&config)?,
            }
            .execute()
            .await?;
        }
        Commands::CreateWallets {
            wallets_dir,
            number_of_wallets,
            balanced,
        } => {
            sprinter::commands::CreateWalletsCommand {
                wallets_dir,
                number_of_wallets,
                balanced,
            }
            .execute()
            .await?;
        }
    }

    Ok(())
}
