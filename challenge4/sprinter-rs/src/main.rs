use anyhow::Result;
use clap::Parser;

use sprinter::cli::{Challenge4Sub, Cli, Commands};
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
        Commands::Collect {
            wallets_dir,
            config,
            destination,
        } => {
            sprinter::commands::CollectCommand {
                wallets_dir,
                network_config: NetworkConfig::load(&config)?,
                destination,
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
        Commands::SwapDex { sc, contract, token_in, amount_in, token_out, amount_out_min, swap_all, total_txs_per_wallet } => {
            sprinter::commands::SwapDexCommand {
                wallets_dir: sc.wallets_dir,
                network_config: NetworkConfig::load(&sc.config)?,
                shard: sc.shard,
                contract,
                token_in,
                amount_in,
                token_out,
                amount_out_min,
                swap_all,
                relayer: sc.relayer,
                random_relayer: sc.random_relayer,
                total_txs_per_wallet,
                batch_size: sc.batch_size,
                sleep_time: sc.sleep_time,
                sign_threads: sc.sign_threads,
                send_parallelism: sc.send_parallelism,
                gas_price: sc.gas_price,
                no_tui: sc.no_tui,
                verbose: sc.verbose,
            }
            .execute()
            .await?;
        }
        Commands::CallContract { sc, contract, function, args, token, token_amount, gas_limit, total_txs_per_wallet } => {
            sprinter::commands::CallContractCommand {
                wallets_dir: sc.wallets_dir,
                network_config: NetworkConfig::load(&sc.config)?,
                shard: sc.shard,
                contract,
                function,
                args,
                token,
                token_amount,
                gas_limit,
                relayer: sc.relayer,
                random_relayer: sc.random_relayer,
                total_txs_per_wallet,
                batch_size: sc.batch_size,
                sleep_time: sc.sleep_time,
                sign_threads: sc.sign_threads,
                send_parallelism: sc.send_parallelism,
                gas_price: sc.gas_price,
                no_tui: sc.no_tui,
                verbose: sc.verbose,
            }
            .execute()
            .await?;
        }
        Commands::CreateWallets {
            wallets_dir,
            number_of_wallets,
            balanced,
            shards,
        } => {
            sprinter::commands::CreateWalletsCommand {
                wallets_dir,
                number_of_wallets,
                balanced,
                shards,
            }
            .execute()
            .await?;
        }
        Commands::DeployContract {
            wallets_dir,
            config,
            shard,
            wasm_path,
            args,
            gas_limit,
            gas_price,
            no_tui,
            verbose,
        } => {
            sprinter::commands::DeployContractCommand {
                wallets_dir,
                network_config: NetworkConfig::load(&config)?,
                shard,
                wasm_path,
                args,
                gas_limit,
                gas_price,
                no_tui,
                verbose,
            }
            .execute()
            .await?;
        }
        Commands::Challenge4 { sub } => match sub {
            Challenge4Sub::Deploy {
                wallets_dir, config, wasm_path, dex_pair, wegld_token, usdc_token,
                gas_price, no_tui, verbose,
            } => {
                sprinter::commands::Challenge4DeployCommand {
                    wallets_dir,
                    network_config: NetworkConfig::load(&config)?,
                    wasm_path,
                    dex_pair,
                    wegld_token,
                    usdc_token,
                    gas_price,
                    no_tui,
                    verbose,
                }
                .execute()
                .await?;
            }
            Challenge4Sub::Wrap {
                wallets_dir, config, wegld_wrap_contract, wrap_amount, gas_price,
            } => {
                sprinter::commands::Challenge4WrapCommand {
                    wallets_dir,
                    network_config: NetworkConfig::load(&config)?,
                    wegld_wrap_contract,
                    wrap_amount,
                    gas_price,
                }
                .execute()
                .await?;
            }
            Challenge4Sub::Spam {
                wallets_dir, config, forwarders_file,
                dex_pair, wegld_token, usdc_token, token_amount,
                milestone_gas_price, gas_price, gas_limit, gas_limit_cross,
                phase1_per_type, start_at,
                batch_size, sleep_time, sign_threads, send_parallelism, no_tui, verbose,
            } => {
                sprinter::commands::Challenge4SpamCommand {
                    wallets_dir,
                    network_config: NetworkConfig::load(&config)?,
                    forwarders_file,
                    dex_pair,
                    wegld_token,
                    usdc_token,
                    token_amount,
                    milestone_gas_price,
                    gas_price,
                    gas_limit,
                    gas_limit_cross,
                    phase1_per_type,
                    start_at,
                    batch_size,
                    sleep_time,
                    sign_threads,
                    send_parallelism,
                    no_tui,
                    verbose,
                }
                .execute()
                .await?;
            }
            Challenge4Sub::MeasureGas {
                wallets_dir, config, forwarder_s1, forwarder_s0, dex_pair,
                wegld_token, usdc_token, token_amount, gas_price,
            } => {
                sprinter::commands::Challenge4MeasureGasCommand {
                    wallets_dir,
                    network_config: NetworkConfig::load(&config)?,
                    forwarder_s1,
                    forwarder_s0,
                    dex_pair,
                    wegld_token,
                    usdc_token,
                    token_amount,
                    gas_price,
                }
                .execute()
                .await?;
            }
            Challenge4Sub::Drain {
                wallets_dir, config, forwarders_file,
                wegld_token, usdc_token, gas_price, gas_limit,
                continuous, interval_secs, verbose,
            } => {
                sprinter::commands::Challenge4DrainCommand {
                    wallets_dir,
                    network_config: NetworkConfig::load(&config)?,
                    forwarders_file,
                    wegld_token,
                    usdc_token,
                    gas_price,
                    gas_limit,
                    continuous,
                    interval_secs,
                    verbose,
                }
                .execute()
                .await?;
            }
        },
    }

    Ok(())
}
