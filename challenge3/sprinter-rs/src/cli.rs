use clap::{Args, Parser, Subcommand};

/// CLI argument parsing for the sprinter application.
#[derive(Parser)]
#[command(name = "sprinter", about = "MultiversX Sprinter CLI (Rust)")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

/// Arguments shared by all transfer commands.
#[derive(Args, Clone)]
pub struct TransferArgs {
    #[arg(long)]
    pub wallets_dir: String,
    /// Path to network config TOML file (proxy + per-shard observer URLs)
    #[arg(long, default_value = "network.toml")]
    pub config: String,
    #[arg(long)]
    pub amount: u128,
    #[arg(long)]
    pub relayer: Option<String>,
    #[arg(long)]
    pub random_relayer: bool,
    #[arg(long, default_value = "99")]
    pub total_txs_per_wallet: usize,
    #[arg(long, default_value = "99")]
    pub batch_size: usize,
    #[arg(long, default_value = "0")]
    pub sleep_time: u64,
    #[arg(long, default_value = "0")]
    pub sign_threads: usize,
    #[arg(long, default_value = "1")]
    pub send_parallelism: usize,
    #[arg(long, default_value = "1000000000")]
    pub gas_price: u64,
    /// Disable the TUI dashboard and use simple console output
    #[arg(long)]
    pub no_tui: bool,
    /// Log every accepted tx hash (very verbose; avoid in production runs)
    #[arg(long)]
    pub verbose: bool,
    /// Use deterministic wallet pairing (ping-pong) instead of random receivers
    #[arg(long, default_value_t = true)]
    pub ping_pong: bool,
    /// Mirror all log output to this file (console output still shown)
    #[arg(long)]
    pub log_file: Option<String>,
    /// Write all log output to this file only (suppresses console/TUI log output)
    #[arg(long)]
    pub log_file_only: Option<String>,
}


#[derive(Subcommand)]
pub enum Commands {
    /// Fund all wallets from a whale wallet
    Fund {
        #[arg(long)]
        wallets_dir: String,
        /// Path to network config TOML file
        #[arg(long, default_value = "network.toml")]
        config: String,
        #[arg(long)]
        whale: String,
        #[arg(long)]
        amount: Option<u128>,
    },
    /// Intrashard transfers
    TransferIntrashard {
        #[command(flatten)]
        transfer: TransferArgs,
        #[arg(long)]
        shard: u8,
    },
    /// Cross-shard transfers
    TransferCrossShard {
        #[command(flatten)]
        transfer: TransferArgs,
        #[arg(long)]
        source_shard: u8,
        #[arg(long)]
        destination_shard: u8,
    },
    /// Concurrent cross-shard blast across all 6 ordered shard pairs
    TransferAllCrossShards {
        #[command(flatten)]
        transfer: TransferArgs,
    },
    /// Concurrent blast across all shards
    TransferAllShards {
        #[command(flatten)]
        transfer: TransferArgs,
    },
    /// List nonces and balances of all loaded wallets
    CheckWallets {
        #[arg(long)]
        wallets_dir: String,
        /// Path to network config TOML file
        #[arg(long, default_value = "network.toml")]
        config: String,
    },
    /// Create new wallets
    CreateWallets {
        #[arg(long)]
        wallets_dir: String,
        #[arg(long)]
        number_of_wallets: usize,
        #[arg(long)]
        balanced: bool,
    },
}
