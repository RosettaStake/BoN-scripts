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

/// Broadcast arguments shared by smart-contract commands (SwapDex, CallContract).
#[derive(Args, Clone)]
pub struct SmartContractArgs {
    #[arg(long)]
    pub wallets_dir: String,
    /// Path to network config TOML file (proxy + per-shard observer URLs)
    #[arg(long, default_value = "network.toml")]
    pub config: String,
    #[arg(long)]
    pub shard: u8,
    #[arg(long)]
    pub relayer: Option<String>,
    #[arg(long)]
    pub random_relayer: bool,
    #[arg(long, default_value = "99")]
    pub batch_size: usize,
    #[arg(long, default_value = "0")]
    pub sleep_time: u64,
    #[arg(long, default_value = "0")]
    pub sign_threads: usize,
    #[arg(long, default_value = "2")]
    pub send_parallelism: usize,
    #[arg(long, default_value = "1000000000")]
    pub gas_price: u64,
    /// Disable the TUI dashboard and use simple console output
    #[arg(long)]
    pub no_tui: bool,
    /// Log every accepted tx hash (very verbose; avoid in production runs)
    #[arg(long)]
    pub verbose: bool,
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
    /// Collect all EGLD from wallets back to a single address
    Collect {
        #[arg(long)]
        wallets_dir: String,
        /// Path to network config TOML file
        #[arg(long, default_value = "network.toml")]
        config: String,
        /// Destination bech32 address to collect funds into
        #[arg(long)]
        destination: String,
    },
    /// List nonces and balances of all loaded wallets
    CheckWallets {
        #[arg(long)]
        wallets_dir: String,
        /// Path to network config TOML file
        #[arg(long, default_value = "network.toml")]
        config: String,
    },
    /// Swap tokens on a DEX pair contract
    SwapDex {
        #[command(flatten)]
        sc: SmartContractArgs,
        #[arg(long)]
        contract: String,
        #[arg(long)]
        token_in: String,
        #[arg(long, default_value = "0")]
        amount_in: u128,
        #[arg(long)]
        token_out: String,
        #[arg(long, default_value = "1")]
        amount_out_min: u128,
        /// Swap entire token_in balance for each wallet (1 tx per wallet)
        #[arg(long)]
        swap_all: bool,
        #[arg(long, default_value = "5")]
        total_txs_per_wallet: usize,
    },
    /// Call a smart contract function
    CallContract {
        #[command(flatten)]
        sc: SmartContractArgs,
        /// Contract address (bech32)
        #[arg(long)]
        contract: String,
        /// Function name to call
        #[arg(long)]
        function: String,
        /// Hex-encoded arguments, space-separated (e.g. --args deadbeef 0a)
        #[arg(long, num_args = 0..)]
        args: Vec<String>,
        /// ESDT token identifier to attach (triggers ESDTTransfer encoding)
        #[arg(long)]
        token: Option<String>,
        /// Amount of ESDT token to transfer (atomic units)
        #[arg(long, default_value = "0")]
        token_amount: u128,
        #[arg(long, default_value = "15000000")]
        gas_limit: u64,
        #[arg(long, default_value = "99")]
        total_txs_per_wallet: usize,
    },
    /// Create new wallets
    CreateWallets {
        #[arg(long)]
        wallets_dir: String,
        #[arg(long, default_value = "0")]
        number_of_wallets: usize,
        #[arg(long)]
        balanced: bool,
        /// Explicit per-shard counts as "S0,S1,S2" (e.g. --shards 20,60,20).
        /// Overrides --number-of-wallets and --balanced.
        #[arg(long)]
        shards: Option<String>,
    },
    /// Deploy a smart contract
    DeployContract {
        #[arg(long)]
        wallets_dir: String,
        #[arg(long, default_value = "network.toml")]
        config: String,
        #[arg(long)]
        shard: u8,
        #[arg(long)]
        wasm_path: String,
        #[arg(long, num_args = 0..)]
        args: Vec<String>,
        #[arg(long, default_value = "100000000")]
        gas_limit: u64,
        #[arg(long, default_value = "1000000000")]
        gas_price: u64,
        #[arg(long)]
        no_tui: bool,
        #[arg(long)]
        verbose: bool,
    },
    /// Challenge 4 — Contract Storm (prepare / spam / drain)
    Challenge4 {
        #[command(subcommand)]
        sub: Challenge4Sub,
    },
}

#[derive(Subcommand)]
pub enum Challenge4Sub {
    /// Deploy forwarder-blind.wasm to all wallets (run before funding — uses only gas)
    Deploy {
        #[arg(long)]
        wallets_dir: String,
        #[arg(long, default_value = "network.toml")]
        config: String,
        /// Path to forwarder-blind.wasm
        #[arg(long)]
        wasm_path: String,
        /// DEX pair address (printed in gas-measurement hint at the end)
        #[arg(long, default_value = "erd1qqqqqqqqqqqqqpgqeel2kumf0r8ffyhth7pqdujjat9nx0862jpsg2pqaq")]
        dex_pair: String,
        /// WEGLD token identifier (printed in gas-measurement hint)
        #[arg(long, default_value = "WEGLD-bd4d79")]
        wegld_token: String,
        /// USDC token identifier (printed in gas-measurement hint)
        #[arg(long, default_value = "USDC-c76f1f")]
        usdc_token: String,
        #[arg(long, default_value = "1000000000")]
        gas_price: u64,
        #[arg(long)]
        no_tui: bool,
        #[arg(long)]
        verbose: bool,
    },
    /// Wrap EGLD→WEGLD for all wallets (run after receiving the 500 EGLD funding)
    Wrap {
        #[arg(long)]
        wallets_dir: String,
        #[arg(long, default_value = "network.toml")]
        config: String,
        /// WEGLD wrap contract address
        #[arg(long, default_value = "erd1qqqqqqqqqqqqqpgqmuk0q2saj0mgutxm4teywre6dl8wqf58xamqdrukln")]
        wegld_wrap_contract: String,
        /// Amount of EGLD to wrap per wallet in aEGLD; default 0.015 EGLD
        /// At 0.000001 WEGLD/call, S0/S2 wallets need ~0.012 WEGLD (12k calls).
        /// 0.015 gives 25% buffer. Total: 0.015 × 100 = 1.5 EGLD locked (recoverable).
        #[arg(long, default_value = "15000000000000000")]
        wrap_amount: u128,
        #[arg(long, default_value = "1000000000")]
        gas_price: u64,
    },
    /// Phase 1 milestone burst + Phase 2 burn-all volume spam
    Spam {
        #[arg(long)]
        wallets_dir: String,
        #[arg(long, default_value = "network.toml")]
        config: String,
        /// Path to forwarders.toml written by `prepare`.
        /// Defaults to `{wallets_dir}/forwarders.toml` when not specified.
        #[arg(long, default_value = "")]
        forwarders_file: String,
        /// DEX pair address (Shard 1, WEGLD↔USDC xExchange)
        #[arg(long, default_value = "erd1qqqqqqqqqqqqqpgqeel2kumf0r8ffyhth7pqdujjat9nx0862jpsg2pqaq")]
        dex_pair: String,
        /// WEGLD token identifier
        #[arg(long, default_value = "WEGLD-bd4d79")]
        wegld_token: String,
        /// USDC token identifier
        #[arg(long, default_value = "USDC-c76f1f")]
        usdc_token: String,
        /// WEGLD amount per tx in atomic units (18 decimals; default 0.000001 WEGLD)
        /// At 198k WEGLD pool reserves, outputs ~4 atomic USDC (safe even after 5k dump).
        #[arg(long, default_value = "1000000000000")]
        token_amount: u128,
        /// Gas price for Phase 1 milestone burst (S1 only), in aEGLD.
        /// With gasPriceModifier=0.01, 5 Gwei spike costs only ~4.6 EGLD total for Phase 1.
        /// Gives priority block inclusion for hitting the 2,500 milestone first.
        #[arg(long, default_value = "5000000000")]
        milestone_gas_price: u64,
        /// Gas price for Phase 2 volume spam, in aEGLD (default 1 Gwei)
        #[arg(long, default_value = "1000000000")]
        gas_price: u64,
        /// Gas limit for S1 calls (all 4 types, intra-shard to DEX).
        /// Under congestion: blindAsyncV2 fails at 25M, succeeds at 30M.
        /// Unused gas is refunded on S1 (gasUsed < gasLimit), so 30M is safe.
        #[arg(long, default_value = "30000000")]
        gas_limit: u64,
        /// Gas limit for S0/S2 calls (blindAsyncV1, cross-shard to S1 DEX).
        /// Cross-shard calls consume full gasLimit (no refund). 20M fails; 30M succeeds.
        #[arg(long, default_value = "30000000")]
        gas_limit_cross: u64,
        /// Phase 1 txs per call type per wallet.
        /// 7 × 4 types × 60 S1 wallets + 7 × 4 × 40 S0/S2 = 2,800 txs → milestone 2,500 ✓
        /// All 4 type minimums: 7 × 60 = 420 each ≥ 300 ✓
        #[arg(long, default_value = "7")]
        phase1_per_type: usize,
        /// UTC time to fire at, format HH:MM:SS or HH:MM:SS.f (e.g. 15:59:59.5).
        /// Queues are built first, then the process waits with a live countdown.
        /// Tip: start ~500ms early to account for network latency to block inclusion.
        /// If omitted, prompts for manual confirmation instead.
        #[arg(long)]
        start_at: Option<String>,
        #[arg(long, default_value = "4")]
        batch_size: usize,
        #[arg(long, default_value = "0")]
        sleep_time: u64,
        #[arg(long, default_value = "0")]
        sign_threads: usize,
        #[arg(long, default_value = "8")]
        send_parallelism: usize,
        #[arg(long)]
        no_tui: bool,
        #[arg(long)]
        verbose: bool,
    },
    /// Simulate one tx per call type to measure exact gas; outputs recommended --gas-limit
    MeasureGas {
        #[arg(long)]
        wallets_dir: String,
        #[arg(long, default_value = "network.toml")]
        config: String,
        /// Deployed forwarder-blind address on Shard 1 (required)
        #[arg(long)]
        forwarder_s1: String,
        /// Deployed forwarder-blind address on Shard 0 (optional; measures cross-shard gas)
        #[arg(long, default_value = "")]
        forwarder_s0: String,
        #[arg(long, default_value = "erd1qqqqqqqqqqqqqpgqeel2kumf0r8ffyhth7pqdujjat9nx0862jpsg2pqaq")]
        dex_pair: String,
        #[arg(long, default_value = "WEGLD-bd4d79")]
        wegld_token: String,
        #[arg(long, default_value = "USDC-c76f1f")]
        usdc_token: String,
        /// Token amount per simulated tx (same as --token-amount in spam); default 0.00001 WEGLD
        #[arg(long, default_value = "10000000000000")]
        token_amount: u128,
        #[arg(long, default_value = "1000000000")]
        gas_price: u64,
    },
    /// Drain trapped tokens from forwarder contracts (S0/S2 always; S1 for blindTransfExec)
    Drain {
        #[arg(long)]
        wallets_dir: String,
        #[arg(long, default_value = "network.toml")]
        config: String,
        /// Path to forwarders.toml written by `prepare`.
        /// Defaults to `{wallets_dir}/forwarders.toml` when not specified.
        #[arg(long, default_value = "")]
        forwarders_file: String,
        /// WEGLD token identifier
        #[arg(long, default_value = "WEGLD-bd4d79")]
        wegld_token: String,
        /// USDC token identifier
        #[arg(long, default_value = "USDC-c76f1f")]
        usdc_token: String,
        #[arg(long, default_value = "1000000000")]
        gas_price: u64,
        /// Gas limit for drain calls (10M matches dex-interactor reference)
        #[arg(long, default_value = "10000000")]
        gas_limit: u64,
        /// Loop indefinitely, draining every --interval-secs seconds
        #[arg(long)]
        continuous: bool,
        /// Seconds between drain runs in continuous mode (default 60)
        #[arg(long, default_value = "60")]
        interval_secs: u64,
        #[arg(long)]
        verbose: bool,
    },
}
