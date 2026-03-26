//! Challenge 4 — Contract Storm
//!
//! Sub-commands:
//!   deploy      — deploy forwarder-blind.wasm (1 per wallet), write forwarders.toml
//!   wrap        — wrap EGLD→WEGLD for all wallets (run after 500 EGLD funding)
//!   measure-gas — probe minimum gas limit per call type
//!   spam        — Phase 1 milestone burst + Phase 2 burn-all volume
//!   drain       — recover trapped tokens from forwarder contracts

use crate::blockchain::nonce::NonceTracker;
use crate::blockchain::transaction::{BroadcastConfig, BroadcastHelper};
use crate::commands::Command;
use crate::network_config::NetworkConfig;
use crate::tui;
use crate::wallet::{RelayedTransaction, WalletEntry, WalletManager, WalletQueue};
use anyhow::{Context, Result};
use async_trait::async_trait;
use multiversx_chain_core::{std::Bech32Address, types::Address};
use multiversx_sdk::gateway::NetworkConfigRequest;
use multiversx_sdk_http::GatewayHttpProxy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

// ── Encoding helpers ──────────────────────────────────────────────────────────

/// Decode a bech32 address to its 32-byte pubkey hex (for tx data fields).
pub fn bech32_to_hex(addr: &str) -> String {
    hex::encode(Bech32Address::from_bech32_string(addr.to_string()).to_address().as_bytes())
}

/// Minimal big-endian hex of a u128 (always even-length, MultiversX convention).
fn hex_u128(v: u128) -> String {
    crate::commands::swap_dex::hex_encode_u128(v)
}

/// Build the data field for a forwarder-blind ESDTTransfer call.
///
/// Wire encoding (joined by "@", then base64):
///   ESDTTransfer @ token_in_hex @ amount_hex @ call_type_hex
///              @ dest_pubkey_hex @ swap_endpoint_hex @ token_out_hex @ min_out_hex
///
/// The dest_pubkey_hex is the 32-byte raw pubkey of the DEX pair, NOT a bech32 string.
fn build_forwarder_data(
    token_in: &str,
    amount_in: u128,
    call_type: &str,       // "blindSync" | "blindAsyncV1" | "blindAsyncV2" | "blindTransfExec"
    dest_pubkey_hex: &str, // bech32_to_hex(dex_pair_addr)
    token_out: &str,
    min_out: u128,
) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let raw = [
        "ESDTTransfer".to_string(),
        hex::encode(token_in),
        hex_u128(amount_in),
        hex::encode(call_type),
        dest_pubkey_hex.to_string(),
        hex::encode("swapTokensFixedInput"),
        hex::encode(token_out),
        hex_u128(min_out),
    ]
    .join("@");
    STANDARD.encode(raw.as_bytes())
}

/// Build data for `drain@<token_hex>@` — recovers trapped tokens from the forwarder.
/// Second arg is token_nonce: empty (canonical MultiversX top-encode of u64=0) for fungible ESDT.
/// Official format from brief: drain@USDC-c76f1f@ (plaintext) = drain@<hex>@ (wire encoding).
fn build_drain_data(token: &str) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let raw = format!("drain@{}@", hex::encode(token));
    STANDARD.encode(raw.as_bytes())
}

/// Build data for `wrapEgld` (converts EGLD → WEGLD on the wrap contract).
fn build_wrap_egld_data() -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    STANDARD.encode(b"wrapEgld")
}

/// Build deploy data for a WASM SC (Upgradeable + Payable metadata).
fn build_deploy_data(wasm_bytes: &[u8]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let raw = [
        hex::encode(wasm_bytes),
        "0500".to_string(), // VM type: WASM
        "0102".to_string(), // Code metadata: Upgradeable | Payable
    ]
    .join("@");
    STANDARD.encode(raw.as_bytes())
}

/// Create a forwarder call tx (ESDTTransfer to forwarder, value = 0 EGLD).
///
/// Use `nonce = 0` for burn-all templates — the broadcast loop overwrites nonce on refill.
fn make_forwarder_tx(
    sender: &Arc<WalletEntry>,
    forwarder: &Bech32Address,
    token_in: &str,
    amount_in: u128,
    call_type: &str,
    dest_pubkey_hex: &str,
    token_out: &str,
    nonce: u64,
    gas_price: u64,
    gas_limit: u64,
    chain_id: &str,
    version: u32,
) -> RelayedTransaction {
    let mut tx = RelayedTransaction::from_parts(
        nonce,
        0, // value = 0 EGLD; ESDT tokens transferred via data field
        forwarder,
        &sender.bech32,
        gas_price,
        gas_limit,
        chain_id,
        version,
        None,
    );
    tx.data = Some(build_forwarder_data(
        token_in, amount_in, call_type, dest_pubkey_hex, token_out, 1,
    ));
    tx
}

/// Query ESDT balance of an address via REST.  Returns 0 on error or missing token.
async fn get_esdt_balance(
    client: &reqwest::Client,
    proxy_url: &str,
    addr: &str,
    token: &str,
) -> u128 {
    let url = format!("{}/address/{}/esdt/{}", proxy_url, addr, token);
    let Ok(resp) = client.get(&url).send().await else { return 0 };
    let Ok(body) = resp.json::<serde_json::Value>().await else { return 0 };
    body.pointer("/data/tokenData/balance")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u128>().ok())
        .unwrap_or(0)
}

/// Pad a balance vector to `len` with zeros (covers partial sync failures).
fn pad_balances(mut v: Vec<u128>, len: usize) -> Vec<u128> {
    v.resize(len, 0);
    v
}

/// Block the task until a given UTC wall-clock time (HH:MM:SS or HH:MM:SS.f).
/// Prints a live countdown on a single line. No-ops if the time has already passed.
async fn wait_until_utc(hh_mm_ss: &str) -> Result<()> {
    use chrono::{NaiveTime, Timelike, Utc};
    use std::io::Write;

    let target = NaiveTime::parse_from_str(hh_mm_ss, "%H:%M:%S%.f")
        .or_else(|_| NaiveTime::parse_from_str(hh_mm_ss, "%H:%M:%S"))
        .map_err(|e| anyhow::anyhow!("Bad --start-at format (expected HH:MM:SS[.ms] UTC): {e}"))?;

    loop {
        let now = Utc::now().time();
        if now >= target {
            break;
        }
        // Duration until target (same day; wraps at midnight gracefully)
        let delta = target
            .signed_duration_since(now)
            .to_std()
            .unwrap_or(Duration::ZERO);
        let h = delta.as_secs() / 3600;
        let m = (delta.as_secs() % 3600) / 60;
        let s = delta.as_secs() % 60;
        let ms = delta.subsec_millis();
        print!(
            "\r⏰  Challenge starts in {:02}:{:02}:{:02}.{:03}  (UTC now {:02}:{:02}:{:02}) — queues hot, ready to fire   ",
            h, m, s, ms,
            now.hour(), now.minute(), now.second(),
        );
        std::io::stdout().flush().ok();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    println!("\n🚀  {} UTC — FIRE!", hh_mm_ss);
    Ok(())
}

/// Estimate total SC calls based on per-wallet EGLD balance and gas params.
/// Uses the MultiversX fee model: moveBalanceGas at full price, SC gas at 1%.
/// Assumes ~280 base64 data bytes (typical ESDTTransfer+forwarder call ≈ 210 raw bytes).
fn estimate_calls(balances: &[u128], gas_limit: u64, gas_price: u64) -> u64 {
    let data_bytes: u64 = 210; // approx raw data bytes for forwarder ESDTTransfer
    let move_gas = (50_000u64 + 1_500 * data_bytes).min(gas_limit);
    let sc_gas = gas_limit.saturating_sub(move_gas);
    let gp = gas_price as u128;
    let cost_per_tx = move_gas as u128 * gp + sc_gas as u128 * gp / 100;
    if cost_per_tx == 0 {
        return 0;
    }
    balances.iter().map(|&b| (b / cost_per_tx) as u64).sum()
}

// ── Forwarder file I/O ────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ForwarderEntry {
    wallet: String,
    forwarder: String,
    shard: u8,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct ForwardersFile {
    entries: Vec<ForwarderEntry>,
}

fn forwarders_path(wallets_dir: &str) -> String {
    format!("{}/forwarders.toml", wallets_dir)
}

fn load_forwarders(file: &str) -> Result<Vec<ForwarderEntry>> {
    let content = std::fs::read_to_string(file)
        .with_context(|| format!("Cannot read forwarders file: {file}\nRun `challenge4 deploy` first."))?;
    let parsed: ForwardersFile = toml::from_str(&content)
        .with_context(|| format!("Cannot parse forwarders file: {file}"))?;
    Ok(parsed.entries)
}

fn write_forwarders(file: &str, entries: &[ForwarderEntry]) -> Result<()> {
    let data = ForwardersFile { entries: entries.to_vec() };
    let content = toml::to_string_pretty(&data)
        .context("Failed to serialize forwarders.toml")?;
    std::fs::write(file, format!("# Generated by: sprinter challenge4 deploy\n{}", content))?;
    Ok(())
}

/// Compute the deterministic MultiversX SC deployment address.
///
/// Formula (from @multiversx/sdk-core `AddressComputer.computeContractAddress`):
///   address[0..8]   = 0x00 × 8
///   address[8..10]  = vmType = 0x05,0x00 (WASM VM)
///   address[10..30] = keccak256(ownerPubkey || nonceLE8)[10..30]
///   address[30..32] = ownerPubkey[30..32]  ← shard selector
fn compute_contract_address(creator: &Address, nonce: u64) -> Address {
    use sha3::{Digest, Keccak256};
    let owner = creator.as_bytes();
    let mut hasher = Keccak256::new();
    hasher.update(owner);
    hasher.update(&nonce.to_le_bytes());
    let hash: [u8; 32] = hasher.finalize().into();

    let mut addr = [0u8; 32];
    addr[8] = 0x05;
    addr[9] = 0x00;
    addr[10..30].copy_from_slice(&hash[10..30]);
    addr[30..32].copy_from_slice(&owner[30..32]);
    Address::new(addr)
}

// ── Challenge4SpamCommand ─────────────────────────────────────────────────────

/// Main spam command.
///
/// **Phase 1** (milestone burst):
///   Shard 1 wallets: `phase1_per_type` txs of each of the 4 call types
///     = 4 × phase1_per_type txs/wallet.  At p1=7: 60 wallets × 7 = 420 per type (≥300 min).
///   Shard 0/2 wallets: `4 × phase1_per_type` txs of blindAsyncV1 only.
///     Shard 1 covers all 4 type minimums; S0/S2 add volume for milestone.
///     Total: 7×4×100 = 2,800 ≥ 2,500 milestone ✓
///
/// **Phase 2** (base gas — burn-all volume):
///   All shards: unidirectional WEGLD→USDC blindSync (S1) / blindAsyncV1 (S0/S2).
///   Runs until EGLD balance exhausted.
///
/// **Gas budget at 33M-45M gas/call**:
///   At 45M gas × 1 Gwei: 0.045 EGLD/call → ~11,000 total calls on 499 EGLD.
///   Spike gas is NOT viable: 5 Gwei × 45M × 2,800 P1 txs = 630 EGLD (exceeds budget).
///   Use 1 Gwei for both phases; raise via TUI [G] key only if blocks are full.
pub struct Challenge4SpamCommand {
    pub wallets_dir: String,
    pub network_config: NetworkConfig,
    /// Path to forwarders.toml written by `deploy`.
    /// Defaults to `{wallets_dir}/forwarders.toml` when empty.
    pub forwarders_file: String,
    pub dex_pair: String,
    pub wegld_token: String,
    pub usdc_token: String,
    /// WEGLD token amount per tx in atomic units (18 decimals).
    /// Default: 10_000_000_000_000 = 0.00001 WEGLD.
    /// Keep small — at 33M-45M gas/call, each wallet makes ~111-151 calls before EGLD
    /// runs out, consuming ~0.0015 WEGLD total. The 0.005 WEGLD wrap default covers this 3×.
    pub token_amount: u128,
    /// Gas price for Phase 1 milestone burst, in aEGLD.
    /// At 33M-45M gas, spike gas is not viable (5 Gwei × 45M × 2800 txs = 630 EGLD).
    /// Default: same as gas_price (1 Gwei, no spike).
    pub milestone_gas_price: u64,
    /// Gas price for Phase 2 volume spam, in aEGLD.
    pub gas_price: u64,
    /// Gas limit for S1 calls (all 4 types, intra-shard to DEX).
    pub gas_limit: u64,
    /// Gas limit for S0/S2 calls (blindAsyncV1, cross-shard to S1 DEX).
    /// Much cheaper than S1 (~5M vs ~30M) → 6× more calls per EGLD.
    pub gas_limit_cross: u64,
    /// Phase 1 txs per call type per wallet.
    /// 7 × 4 types × 60 S1 + 7 × 4 × 40 S0/S2 = 2,800 ≥ 2,500 milestone ✓
    pub phase1_per_type: usize,
    /// Optional UTC time to wait for before firing (format "HH:MM:SS").
    /// Build queues before calling spam, then let it countdown to exact start time.
    pub start_at: Option<String>,
    pub batch_size: usize,
    pub sleep_time: u64,
    pub sign_threads: usize,
    pub send_parallelism: usize,
    pub no_tui: bool,
    pub verbose: bool,
}

#[async_trait]
impl Command for Challenge4SpamCommand {
    async fn execute(&self) -> Result<()> {
        let client = reqwest::Client::new();

        println!("\n╔══════════════════════════════════════════════════╗");
        println!("║     CHALLENGE 4 — CONTRACT STORM — SPAM          ║");
        println!("╚══════════════════════════════════════════════════╝");
        println!("DEX pair     : {}", self.dex_pair);
        println!("WEGLD        : {}", self.wegld_token);
        println!("USDC         : {}", self.usdc_token);
        println!("Token/tx     : {} atomic ({:.6} WEGLD)", self.token_amount,
            self.token_amount as f64 / 1e18);
        println!("Gas limit S1 : {}M",  self.gas_limit / 1_000_000);
        println!("Gas limit S0/2: {}M (cross-shard)", self.gas_limit_cross / 1_000_000);
        println!("Gas price P1 : {} Gwei", self.milestone_gas_price / 1_000_000_000);
        println!("Gas price P2 : {} Gwei", self.gas_price / 1_000_000_000);
        println!("Phase1/type  : {} txs ({} total per wallet on S1)",
            self.phase1_per_type, self.phase1_per_type * 4);
        if let Some(t) = &self.start_at {
            println!("Start at     : {} UTC", t);
        }

        let mut wallet_manager = WalletManager::new(&self.wallets_dir);
        wallet_manager.load_wallets()?;

        let ff_path = if self.forwarders_file.is_empty() {
            forwarders_path(&self.wallets_dir)
        } else {
            self.forwarders_file.clone()
        };
        let forwarder_entries = load_forwarders(&ff_path)?;
        // One forwarder per shard; all wallets in a shard call the same forwarder.
        let forwarder_by_shard: HashMap<u8, Bech32Address> = forwarder_entries
            .into_iter()
            .map(|e| (e.shard, Bech32Address::from_bech32_string(e.forwarder)))
            .collect();

        let s0 = wallet_manager.get_wallets_by_shard(0).to_vec();
        let s1 = wallet_manager.get_wallets_by_shard(1).to_vec();
        let s2 = wallet_manager.get_wallets_by_shard(2).to_vec();

        println!("\nWallets S0:{} S1:{} S2:{} (total: {})",
            s0.len(), s1.len(), s2.len(), s0.len() + s1.len() + s2.len());
        println!("Forwarders   : {} shard forwarder(s) (from {})", forwarder_by_shard.len(), ff_path);

        // Sync nonces + balances across all 3 shards in parallel.
        println!("Syncing nonces + balances...");
        let proxy0 = GatewayHttpProxy::new(self.network_config.shard_url(0));
        let proxy1 = GatewayHttpProxy::new(self.network_config.shard_url(1));
        let proxy2 = GatewayHttpProxy::new(self.network_config.shard_url(2));

        let (b0, b1, b2) = tokio::join!(
            NonceTracker::sync_nonces(&proxy0, &s0),
            NonceTracker::sync_nonces(&proxy1, &s1),
            NonceTracker::sync_nonces(&proxy2, &s2),
        );
        let balances0 = pad_balances(b0.unwrap_or_else(|e| { println!("⚠️ S0 sync failed: {e}"); vec![] }), s0.len());
        let balances1 = pad_balances(b1.unwrap_or_else(|e| { println!("⚠️ S1 sync failed: {e}"); vec![] }), s1.len());
        let balances2 = pad_balances(b2.unwrap_or_else(|e| { println!("⚠️ S2 sync failed: {e}"); vec![] }), s2.len());

        // Print budget estimate (per-shard gas limits).
        let est_s1 = estimate_calls(&balances1, self.gas_limit, self.gas_price);
        let est_s0 = estimate_calls(&balances0, self.gas_limit_cross, self.gas_price);
        let est_s2 = estimate_calls(&balances2, self.gas_limit_cross, self.gas_price);
        let p1_s1 = s1.len() * self.phase1_per_type * 4;
        let p1_cross = (s0.len() + s2.len()) * self.phase1_per_type * 4;
        let p1_total = p1_s1 + p1_cross;
        println!("\nBudget estimate:");
        println!("  Phase 1 txs : {} (S1: {} @{}M, S0/S2: {} @{}M)",
            p1_total, p1_s1, self.gas_limit / 1_000_000,
            p1_cross, self.gas_limit_cross / 1_000_000);
        println!("  Phase 2 est : S1 ~{} (@{}M), S0/S2 ~{} (@{}M) → ~{} total",
            est_s1, self.gas_limit / 1_000_000,
            est_s0 + est_s2, self.gas_limit_cross / 1_000_000,
            est_s1 + est_s0 + est_s2);
        println!("  Total est   : ~{}", p1_total as u64 + est_s1 + est_s0 + est_s2);

        let config = proxy1.http_request(NetworkConfigRequest).await?;
        let chain_id = config.chain_id.clone();
        let version = config.min_transaction_version;

        let dex_hex = bech32_to_hex(&self.dex_pair);

        let wegld = &self.wegld_token;
        let usdc = &self.usdc_token;
        let amt = self.token_amount;
        let mgp = self.milestone_gas_price;
        let bgp = self.gas_price;
        let gl = self.gas_limit;         // S1 gas limit
        let gl_cross = self.gas_limit_cross; // S0/S2 gas limit
        let cid = chain_id.as_str();
        let p1 = self.phase1_per_type;

        // ── Shard 1 queues ────────────────────────────────────────────────────
        // Phase 1: p1 txs of each of the 4 types (round-robin by type).
        // Phase 2: blindSync WEGLD→USDC burn-all (unidirectional; tokens returned
        //          directly to wallet via intra-shard sync call — no drain needed).
        //
        // NOTE: blindAsyncV1/V2 on Shard 1 with a Shard 1 forwarder and Shard 1 DEX
        // are also intra-shard → tokens also return.  But blindSync is the fastest
        // SC call type so we use it exclusively in Phase 2.
        let queues1: Vec<WalletQueue> = s1
            .iter()
            .zip(balances1.iter())
            .filter_map(|(wallet, &balance)| {
                let fwd1 = forwarder_by_shard.get(&1)?.clone();
                let mut queue = WalletQueue::new(wallet.clone());
                queue.egld_balance = balance;

                const TYPES: [&str; 4] =
                    ["blindSync", "blindAsyncV1", "blindAsyncV2", "blindTransfExec"];
                for i in 0..(p1 * 4) {
                    let tx = make_forwarder_tx(
                        wallet, &fwd1, wegld, amt, TYPES[i % 4], &dex_hex, usdc,
                        wallet.get_nonce_then_increment(), mgp, gl, cid, version,
                    );
                    queue.push(tx, None);
                }

                let tmpl = make_forwarder_tx(
                    wallet, &fwd1, wegld, amt, "blindSync", &dex_hex, usdc,
                    0, bgp, gl, cid, version,
                );
                queue.tx_template = Some((tmpl, None));
                queue.burn_all = true;
                queue.target = usize::MAX;
                Some(queue)
            })
            .collect();

        // ── Shard 0 queues ────────────────────────────────────────────────────
        // Phase 1: 4*p1 blindAsyncV1 txs at base gas (bgp) with cross-shard gas limit.
        //   Cross-shard blindAsyncV1 needs ~5M gas vs ~30M for S1 intra-shard calls.
        //   6× cheaper → S0/S2 produce bulk of total call volume.
        // Phase 2: blindAsyncV1 WEGLD→USDC burn-all (tokens trapped in forwarder → drain).
        let queues0: Vec<WalletQueue> = s0
            .iter()
            .zip(balances0.iter())
            .filter_map(|(wallet, &balance)| {
                let fwd0 = forwarder_by_shard.get(&0)?.clone();
                let mut queue = WalletQueue::new(wallet.clone());
                queue.egld_balance = balance;
                for _ in 0..(p1 * 4) {
                    let tx = make_forwarder_tx(
                        wallet, &fwd0, wegld, amt, "blindAsyncV1", &dex_hex, usdc,
                        wallet.get_nonce_then_increment(), bgp, gl_cross, cid, version,
                    );
                    queue.push(tx, None);
                }
                let tmpl = make_forwarder_tx(
                    wallet, &fwd0, wegld, amt, "blindAsyncV1", &dex_hex, usdc,
                    0, bgp, gl_cross, cid, version,
                );
                queue.tx_template = Some((tmpl, None));
                queue.burn_all = true;
                queue.target = usize::MAX;
                Some(queue)
            })
            .collect();

        // ── Shard 2 queues ────────────────────────────────────────────────────
        // Same as Shard 0: cross-shard gas limit, base gas price.
        let queues2: Vec<WalletQueue> = s2
            .iter()
            .zip(balances2.iter())
            .filter_map(|(wallet, &balance)| {
                let fwd2 = forwarder_by_shard.get(&2)?.clone();
                let mut queue = WalletQueue::new(wallet.clone());
                queue.egld_balance = balance;
                for _ in 0..(p1 * 4) {
                    let tx = make_forwarder_tx(
                        wallet, &fwd2, wegld, amt, "blindAsyncV1", &dex_hex, usdc,
                        wallet.get_nonce_then_increment(), bgp, gl_cross, cid, version,
                    );
                    queue.push(tx, None);
                }
                let tmpl = make_forwarder_tx(
                    wallet, &fwd2, wegld, amt, "blindAsyncV1", &dex_hex, usdc,
                    0, bgp, gl_cross, cid, version,
                );
                queue.tx_template = Some((tmpl, None));
                queue.burn_all = true;
                queue.target = usize::MAX;
                Some(queue)
            })
            .collect();

        let p1_built: usize = queues0.iter().chain(queues1.iter()).chain(queues2.iter())
            .map(|q| q.pending.len())
            .sum();
        println!("\nQueues ready:");
        println!("  Phase 1 txs : {} (all wallets × {} txs)", p1_built, p1 * 4);
        println!("  Phase 2 tmpl: burn-all loaded (S1: blindSync, S0/S2: blindAsyncV1)");
        println!("  Milestone   : 2,500 calls → +10/+7/+5 pts");

        // Countdown to exact start time (if provided), otherwise prompt.
        match &self.start_at {
            Some(t) => wait_until_utc(t).await?,
            None    => crate::utils::wait_for_user_confirmation(),
        }

        let network_config = self.network_config.clone();
        let batch_size = self.batch_size;
        let sleep_time = self.sleep_time;
        let sign_threads = self.sign_threads;
        let send_parallelism = self.send_parallelism;
        let verbose = self.verbose;

        tui::run_with_optional_tui(
            "Challenge4 Spam — Contract Storm".to_string(),
            0u64,
            self.no_tui,
            move |stats, log_handle| async move {
                stats.burn_all_mode.store(true, std::sync::atomic::Ordering::Relaxed);

                let mut handles = Vec::new();
                for (shard, queues) in [(0u8, queues0), (1u8, queues1), (2u8, queues2)] {
                    if queues.is_empty() {
                        continue;
                    }
                    let url = network_config.shard_url(shard);
                    let stats_c = Arc::clone(&stats);
                    let log_c = log_handle.clone();
                    let client_c = client.clone();
                    handles.push(tokio::spawn(async move {
                        BroadcastHelper::new(url, client_c)
                            .broadcast_txs(
                                &format!("S{shard}"),
                                queues,
                                BroadcastConfig {
                                    batch_size,
                                    sleep_time,
                                    sign_threads,
                                    send_parallelism,
                                    verbose,
                                    cross_shard: shard != 1,
                                },
                                Some(stats_c),
                                log_c,
                            )
                            .await;
                    }));
                }
                for h in handles {
                    if let Err(e) = h.await {
                        println!("⚠️ Shard broadcaster panicked: {e}");
                    }
                }
                Ok(())
            },
        )
        .await?;

        Ok(())
    }
}

// ── Challenge4DrainCommand ────────────────────────────────────────────────────

/// Drain trapped tokens from forwarder contracts.
///
/// Calls `drain@USDC` + `drain@WEGLD` on each configured forwarder from the first
/// wallet on that shard.  Tokens flow back to the drain caller.
///
/// **Which shards to drain**:
///   S0/S2: blindAsyncV1 always traps tokens (cross-shard or same-shard alike for
///          the USDC return leg) → must drain.
///   S1:    blindTransfExec always traps tokens → should drain S1 too.
///          blindSync/V1/V2 on S1 return tokens directly → no drain needed for those.
///
/// **Continuous mode** (`--continuous --interval-secs N`):
///   Loops forever, draining every N seconds.  Run this in a separate terminal during
///   the entire challenge window.  Stopped with Ctrl-C.
pub struct Challenge4DrainCommand {
    pub wallets_dir: String,
    pub network_config: NetworkConfig,
    /// Path to forwarders.toml written by `deploy`.
    /// Defaults to `{wallets_dir}/forwarders.toml` when empty.
    pub forwarders_file: String,
    pub wegld_token: String,
    pub usdc_token: String,
    pub gas_price: u64,
    /// Gas limit for drain calls (plain SC call, ~5M sufficient)
    pub gas_limit: u64,
    /// Run drain in a loop indefinitely
    pub continuous: bool,
    /// Seconds between drain runs in continuous mode (default 60)
    pub interval_secs: u64,
    pub verbose: bool,
}

#[async_trait]
impl Command for Challenge4DrainCommand {
    async fn execute(&self) -> Result<()> {
        let client = reqwest::Client::new();

        let mut wallet_manager = WalletManager::new(&self.wallets_dir);
        wallet_manager.load_wallets()?;

        if self.continuous {
            println!("Drain — continuous mode, interval {}s. Ctrl-C to stop.", self.interval_secs);
        }

        let mut run = 0u32;
        loop {
            run += 1;
            if self.continuous {
                println!("\n[drain run #{}]", run);
            }
            self.run_once(&client, &wallet_manager).await;

            if !self.continuous {
                break;
            }
            println!("Next drain in {}s...", self.interval_secs);
            tokio::time::sleep(Duration::from_secs(self.interval_secs)).await;
        }
        Ok(())
    }
}

impl Challenge4DrainCommand {
    async fn run_once(
        &self,
        client: &reqwest::Client,
        wallet_manager: &WalletManager,
    ) {
        let ff_path = if self.forwarders_file.is_empty() {
            forwarders_path(&self.wallets_dir)
        } else {
            self.forwarders_file.clone()
        };
        let entries = match load_forwarders(&ff_path) {
            Ok(e) => e,
            Err(err) => { println!("⚠️ drain: {err}"); return; }
        };

        // Build wallet lookup map
        let all_wallets = wallet_manager.get_all_wallets();
        let wallet_map: HashMap<String, Arc<WalletEntry>> = all_wallets.iter()
            .map(|w| (w.bech32.to_string(), w.clone()))
            .collect();

        // Group entries by shard for parallel nonce syncing
        let mut by_shard: HashMap<u8, Vec<&ForwarderEntry>> = HashMap::new();
        for entry in &entries {
            by_shard.entry(entry.shard).or_default().push(entry);
        }

        // Sync nonces per shard
        for (&shard, shard_entries) in &by_shard {
            let proxy = GatewayHttpProxy::new(self.network_config.shard_url(shard));
            let shard_wallets: Vec<Arc<WalletEntry>> = shard_entries.iter()
                .filter_map(|e| wallet_map.get(&e.wallet))
                .cloned()
                .collect();
            if !shard_wallets.is_empty() {
                let _ = NonceTracker::sync_nonces(&proxy, &shard_wallets).await;
            }
        }

        // Get chain config
        let proxy1 = GatewayHttpProxy::new(self.network_config.shard_url(1));
        let config = match proxy1.http_request(NetworkConfigRequest).await {
            Ok(c) => c,
            Err(e) => { println!("⚠️ Failed to fetch network config: {e}"); return; }
        };
        let chain_id = config.chain_id.clone();
        let version = config.min_transaction_version;
        let wegld = &self.wegld_token;
        let usdc = &self.usdc_token;
        let gp = self.gas_price;
        let gl = self.gas_limit;

        let mut handles = Vec::new();
        for entry in &entries {
            let Some(wallet) = wallet_map.get(&entry.wallet) else {
                println!("  drain: wallet {} not loaded, skipping", entry.wallet);
                continue;
            };
            let fwd_str = &entry.forwarder;
            let shard = entry.shard;
            let proxy_url = self.network_config.shard_url(shard);

            let usdc_bal = get_esdt_balance(client, &proxy_url, fwd_str, usdc).await;
            let wegld_bal = get_esdt_balance(client, &proxy_url, fwd_str, wegld).await;
            if usdc_bal == 0 && wegld_bal == 0 {
                continue;
            }

            println!("  drain S{shard} {} → {} (USDC:{} WEGLD:{})",
                entry.wallet, fwd_str, usdc_bal, wegld_bal);

            let fwd = Bech32Address::from_bech32_string(fwd_str.clone());
            let wallet = wallet.clone();
            let chain_id = chain_id.clone();

            let mut queue = WalletQueue::new(wallet.clone());
            for (token, bal) in [(usdc.as_str(), usdc_bal), (wegld.as_str(), wegld_bal)] {
                if bal > 0 {
                    let mut tx = RelayedTransaction::from_parts(
                        wallet.get_nonce_then_increment(),
                        0, &fwd, &wallet.bech32, gp, gl, &chain_id, version, None,
                    );
                    tx.data = Some(build_drain_data(token));
                    queue.push(tx, None);
                }
            }
            queue.target = queue.pending.len();

            let client_c = client.clone();
            let verbose = self.verbose;
            handles.push(tokio::spawn(async move {
                BroadcastHelper::new(proxy_url, client_c)
                    .broadcast_txs(
                        &format!("drain-S{shard}"),
                        vec![queue],
                        BroadcastConfig { batch_size: 10, sleep_time: 0, sign_threads: 0, send_parallelism: 1, verbose, cross_shard: false },
                        None, None,
                    )
                    .await;
            }));
        }

        if handles.is_empty() {
            println!("  Nothing to drain (all forwarders have 0 balance).");
            return;
        }
        for h in handles {
            if let Err(e) = h.await {
                println!("⚠️ Drain failed: {e}");
            }
        }
        println!("  Drain txs sent ({} forwarders).", entries.len());
    }
}

// ── Challenge4DeployCommand ──────────────────────────────────────────────────

/// Deploy forwarder-blind.wasm — 1 contract per shard (3 total).
///
/// Uses the first wallet of each shard as deployer.
/// Writes forwarder addresses to {wallets_dir}/forwarders.toml.
/// Run with your own EGLD before the guild distributes the 500 EGLD budget.
pub struct Challenge4DeployCommand {
    pub wallets_dir: String,
    pub network_config: NetworkConfig,
    pub wasm_path: String,
    pub dex_pair: String,
    pub wegld_token: String,
    pub usdc_token: String,
    pub gas_price: u64,
    pub no_tui: bool,
    pub verbose: bool,
}

// ── Challenge4WrapCommand ───────────────────────────────────────────────────

/// Wrap EGLD → WEGLD for all wallets.
///
/// Run after the guild distributes the 500 EGLD budget.
/// At 33M-45M gas/call (1 Gwei), each wallet makes ~111-151 calls before EGLD runs out.
/// 0.005 WEGLD covers 500 calls — 3-4× buffer over EGLD-limited call count.
pub struct Challenge4WrapCommand {
    pub wallets_dir: String,
    pub network_config: NetworkConfig,
    pub wegld_wrap_contract: String,
    pub wrap_amount: u128,
    pub gas_price: u64,
}

#[async_trait]
impl Command for Challenge4DeployCommand {
    async fn execute(&self) -> Result<()> {
        let client = reqwest::Client::new();

        println!("\n╔══════════════════════════════════════════════════╗");
        println!("║   CHALLENGE 4 — DEPLOY FORWARDERS                ║");
        println!("╚══════════════════════════════════════════════════╝");

        let mut wallet_manager = WalletManager::new(&self.wallets_dir);
        wallet_manager.load_wallets()?;

        let s0 = wallet_manager.get_wallets_by_shard(0).to_vec();
        let s1 = wallet_manager.get_wallets_by_shard(1).to_vec();
        let s2 = wallet_manager.get_wallets_by_shard(2).to_vec();

        let proxy  = GatewayHttpProxy::new(self.network_config.proxy.clone());
        let proxy0 = GatewayHttpProxy::new(self.network_config.shard_url(0));
        let proxy1 = GatewayHttpProxy::new(self.network_config.shard_url(1));
        let proxy2 = GatewayHttpProxy::new(self.network_config.shard_url(2));

        let config = proxy.http_request(NetworkConfigRequest).await?;
        let chain_id = &config.chain_id;
        let version = config.min_transaction_version;

        // ── Step 1: Deploy forwarder-blind.wasm (1 contract per wallet) ─────
        println!("\n[1/2] Deploying forwarder-blind.wasm — 1 contract per shard (3 total)...");

        let wasm_bytes = std::fs::read(&self.wasm_path)?;
        let deploy_data = build_deploy_data(&wasm_bytes);
        let deploy_addr = Bech32Address::from(Address::new([0u8; 32]));

        let mut forwarder_entries: Vec<ForwarderEntry> = Vec::new();

        // Deploy from first wallet of each shard only.
        let deployers: [(u8, Option<&Arc<WalletEntry>>, &GatewayHttpProxy); 3] = [
            (0, s0.first(), &proxy0),
            (1, s1.first(), &proxy1),
            (2, s2.first(), &proxy2),
        ];

        for (shard, deployer_opt, px) in &deployers {
            let Some(wallet) = deployer_opt else {
                println!("  S{shard}: no wallets — skipping");
                continue;
            };
            let _ = NonceTracker::sync_nonces(px, std::slice::from_ref(wallet)).await;
            let deploy_nonce = wallet.get_nonce_then_increment();
            let new_addr = compute_contract_address(&wallet.address, deploy_nonce);
            let new_bech32 = Bech32Address::from(new_addr);
            forwarder_entries.push(ForwarderEntry {
                wallet: wallet.bech32.to_string(),
                forwarder: new_bech32.to_string(),
                shard: *shard,
            });
            let mut tx = RelayedTransaction::from_parts(
                deploy_nonce, 0, &deploy_addr, &wallet.bech32,
                self.gas_price, 80_000_000, chain_id, version, None,
            );
            tx.data = Some(deploy_data.clone());
            let mut queue = WalletQueue::new((*wallet).clone());
            queue.push(tx, None);
            queue.target = 1;
            let proxy_url = self.network_config.shard_url(*shard);
            BroadcastHelper::new(proxy_url, client.clone())
                .send_once(&format!("S{shard}-deploy"), vec![queue])
                .await;
        }

        let fwd_file = forwarders_path(&self.wallets_dir);
        write_forwarders(&fwd_file, &forwarder_entries)?;
        println!("\n  ✓ {} forwarder addresses written to: {}", forwarder_entries.len(), fwd_file);
        println!("  ⚠️  Verify addresses on explorer before running spam!");
        for e in &forwarder_entries {
            println!("    S{}: deployer {} → forwarder {}", e.shard, e.wallet, e.forwarder);
        }

        // ── Step 2: Next steps ──────────────────────────────────────────────
        println!("\n[2/2] Next steps:");
        println!("  1. Once wallets are funded with 500 EGLD, run:");
        println!("       sprinter challenge4 wrap --wallets-dir {}", self.wallets_dir);
        println!();
        println!("  2. Measure gas (after wrap):");
        println!("       sprinter challenge4 measure-gas --wallets-dir {} \\", self.wallets_dir);
        println!("         --forwarder-s1 <S1_FORWARDER> --dex-pair {} \\", self.dex_pair);
        println!("         --wegld-token {} --usdc-token {}", self.wegld_token, self.usdc_token);
        println!();

        // Budget table using MultiversX fee model (gasPriceModifier=0.01).
        // moveBalanceGas ≈ 365,000 (50k base + 1500 × ~210 data bytes).
        let move_gas: f64 = 365_000.0;
        println!("  Budget table (499 EGLD, gasPriceModifier=0.01, ~210 data bytes):");
        println!("  {:>10}  {:>8}  {:>16}  {:>14}",
            "gas_limit", "Gwei", "cost/call (EGLD)", "~total calls");
        for &gas in &[5_000_000u64, 30_000_000, 45_000_000] {
            for &gwei in &[1u64, 2, 3] {
                let gp = gwei as f64 * 1e-9; // EGLD per gas unit
                let sc_gas = (gas as f64 - move_gas).max(0.0);
                let cost = move_gas * gp + sc_gas * gp * 0.01;
                let calls = 499.0 / cost;
                println!("  {:>10}  {:>8}  {:>16.6}  {:>14.0}", gas, gwei, cost, calls);
            }
        }

        println!("\n  ✓ Deploy complete. Forwarder addresses saved to: {}", fwd_file);
        println!("    Run `challenge4 wrap` after funding, then `challenge4 spam` at 16:00 UTC.");

        Ok(())
    }
}

#[async_trait]
impl Command for Challenge4WrapCommand {
    async fn execute(&self) -> Result<()> {
        let client = reqwest::Client::new();

        println!("\n╔══════════════════════════════════════════════════╗");
        println!("║   CHALLENGE 4 — WRAP EGLD → WEGLD                ║");
        println!("╚══════════════════════════════════════════════════╝");

        let mut wallet_manager = WalletManager::new(&self.wallets_dir);
        wallet_manager.load_wallets()?;

        let s0 = wallet_manager.get_wallets_by_shard(0).to_vec();
        let s1 = wallet_manager.get_wallets_by_shard(1).to_vec();
        let s2 = wallet_manager.get_wallets_by_shard(2).to_vec();

        let proxy0 = GatewayHttpProxy::new(self.network_config.shard_url(0));
        let proxy1 = GatewayHttpProxy::new(self.network_config.shard_url(1));
        let proxy2 = GatewayHttpProxy::new(self.network_config.shard_url(2));

        let config = proxy1.http_request(NetworkConfigRequest).await?;
        let chain_id = &config.chain_id;
        let version = config.min_transaction_version;

        let total_wallets = s0.len() + s1.len() + s2.len();
        let total_egld = self.wrap_amount as f64 / 1e18 * total_wallets as f64;
        println!(
            "\nWrapping {:.4} EGLD per wallet → WEGLD ({} wallets, {:.2} EGLD total)...",
            self.wrap_amount as f64 / 1e18, total_wallets, total_egld
        );

        let _ = tokio::join!(
            NonceTracker::sync_nonces(&proxy0, &s0),
            NonceTracker::sync_nonces(&proxy1, &s1),
            NonceTracker::sync_nonces(&proxy2, &s2),
        );

        let wrap_addr = Bech32Address::from_bech32_string(self.wegld_wrap_contract.clone());
        let wrap_data = build_wrap_egld_data();

        let mut handles = Vec::new();
        for (shard, wallets) in [(0u8, &s0), (1u8, &s1), (2u8, &s2)] {
            if wallets.is_empty() { continue; }
            let mut queues: Vec<WalletQueue> = wallets
                .iter()
                .map(|wallet| {
                    let mut queue = WalletQueue::new(wallet.clone());
                    let mut tx = RelayedTransaction::from_parts(
                        wallet.get_nonce_then_increment(),
                        self.wrap_amount,
                        &wrap_addr,
                        &wallet.bech32,
                        self.gas_price,
                        6_000_000,
                        chain_id,
                        version,
                        None,
                    );
                    tx.data = Some(wrap_data.clone());
                    queue.push(tx, None);
                    queue.target = 1;
                    queue
                })
                .collect();
            super::assign_gas_price(&mut queues, self.gas_price);
            let url = self.network_config.shard_url(shard);
            let client_c = client.clone();
            handles.push(tokio::spawn(async move {
                BroadcastHelper::new(url, client_c)
                    .send_once(&format!("S{shard}-wrap"), queues)
                    .await;
            }));
        }
        for h in handles {
            if let Err(e) = h.await {
                println!("⚠️ Wrap task failed: {e}");
            }
        }
        println!("\n  ✓ Wrap txs sent. Tokens should appear within 1-2 blocks.");
        println!("  Run `challenge4 measure-gas` next, then `challenge4 spam` at 16:00 UTC.");

        Ok(())
    }
}

// ── Challenge4MeasureGasCommand ───────────────────────────────────────────────

/// Simulate one tx per call type against the deployed forwarder and report exact gas usage.
///
/// Run this AFTER `prepare` deployment has confirmed and you know the forwarder addresses.
/// Uses `/transaction/simulate` — no gas consumed, pure dry-run.
///
/// Outputs:
///   - gasUsed per call type per swap direction
///   - Recommended --gas-limit for the `spam` command (max gasUsed × 1.10)
///
/// Workflow:
///   1. `challenge4 prepare` → deploys forwarder, wraps WEGLD
///   2. Find forwarder address from explorer
///   3. `challenge4 measure-gas --forwarder-s1 <addr> ...` → prints --gas-limit
///   4. `challenge4 spam --gas-limit <result> --start-at 16:00:00 ...`
pub struct Challenge4MeasureGasCommand {
    pub wallets_dir: String,
    pub network_config: NetworkConfig,
    /// Forwarder address on Shard 1 (required; blindSync only works on S1)
    pub forwarder_s1: String,
    /// Forwarder address on Shard 0 (optional; measures blindAsyncV1 cross-shard gas)
    pub forwarder_s0: String,
    pub dex_pair: String,
    pub wegld_token: String,
    pub usdc_token: String,
    /// WEGLD amount to simulate per tx (same as --token-amount in spam)
    pub token_amount: u128,
    pub gas_price: u64,
}

/// Result of simulating one transaction.
struct SimResult {
    call_type: &'static str,
    gas_used: u64,
    success: bool,
    fail_reason: String,
}

#[async_trait]
impl Command for Challenge4MeasureGasCommand {
    async fn execute(&self) -> Result<()> {
        println!("\n╔══════════════════════════════════════════════════╗");
        println!("║  CHALLENGE 4 — MEASURE GAS (simulation)          ║");
        println!("╚══════════════════════════════════════════════════╝");
        println!("Forwarder S1 : {}", self.forwarder_s1);
        if !self.forwarder_s0.is_empty() {
            println!("Forwarder S0 : {}", self.forwarder_s0);
        }
        println!("DEX pair     : {}", self.dex_pair);
        println!("Token/tx     : {} atomic", self.token_amount);
        println!("(No gas consumed — pure simulation)");

        let mut wallet_manager = WalletManager::new(&self.wallets_dir);
        wallet_manager.load_wallets()?;

        let s0 = wallet_manager.get_wallets_by_shard(0).to_vec();
        let s1 = wallet_manager.get_wallets_by_shard(1).to_vec();

        if s1.is_empty() {
            anyhow::bail!("No Shard 1 wallets found in {}", self.wallets_dir);
        }

        let proxy1 = GatewayHttpProxy::new(self.network_config.shard_url(1));
        let _ = NonceTracker::sync_nonces(&proxy1, &s1).await;

        let net_cfg = proxy1.http_request(NetworkConfigRequest).await?;
        let chain_id = &net_cfg.chain_id;
        let version = net_cfg.min_transaction_version;

        let dex_hex = bech32_to_hex(&self.dex_pair);
        let fwd1 = Bech32Address::from_bech32_string(self.forwarder_s1.clone());
        let wegld = &self.wegld_token;
        let usdc = &self.usdc_token;
        let amt = self.token_amount;
        let gp = self.gas_price;
        // Use generous gas limit for simulation (doesn't affect cost, only sets ceiling)
        const SIM_GAS_LIMIT: u64 = 600_000_000;

        let wallet_s1 = &s1[0];
        let client = reqwest::Client::new();
        let proxy_url = self.network_config.shard_url(1);

        // Simulate all 4 types on Shard 1 (WEGLD→USDC direction)
        let types_s1: &[(&str, &str, &str)] = &[
            ("blindSync",        wegld, usdc),
            ("blindAsyncV1",     wegld, usdc),
            ("blindAsyncV2",     wegld, usdc),
            ("blindTransfExec",  wegld, usdc),
        ];

        println!("\nSimulating Shard 1 calls (forwarder → DEX, same-shard)...");
        let mut results: Vec<SimResult> = Vec::new();

        for &(call_type, token_in, token_out) in types_s1 {
            let mut tx = make_forwarder_tx(
                wallet_s1, &fwd1, token_in, amt, call_type, &dex_hex, token_out,
                wallet_s1.get_nonce_then_increment(), gp, SIM_GAS_LIMIT, chain_id, version,
            );
            // Sign before simulation
            tx.sign_sender(wallet_s1);

            let sim = simulate_tx(&client, &proxy_url, &tx).await;
            results.push(SimResult {
                call_type,
                gas_used: sim.gas_used,
                success: sim.success,
                fail_reason: sim.fail_reason,
            });
        }

        // Optionally simulate blindAsyncV1 on Shard 0 (cross-shard gas may differ)
        if !self.forwarder_s0.is_empty() && !s0.is_empty() {
            let proxy0 = GatewayHttpProxy::new(self.network_config.shard_url(0));
            let _ = NonceTracker::sync_nonces(&proxy0, &s0).await;
            let fwd0 = Bech32Address::from_bech32_string(self.forwarder_s0.clone());
            let wallet_s0 = &s0[0];
            let proxy_url0 = self.network_config.shard_url(0);

            let mut tx = make_forwarder_tx(
                wallet_s0, &fwd0, wegld, amt, "blindAsyncV1", &dex_hex, usdc,
                wallet_s0.get_nonce_then_increment(), gp, SIM_GAS_LIMIT, chain_id, version,
            );
            tx.sign_sender(wallet_s0);
            let sim = simulate_tx(&client, &proxy_url0, &tx).await;
            results.push(SimResult {
                call_type: "blindAsyncV1 (S0→S1 cross-shard)",
                gas_used: sim.gas_used,
                success: sim.success,
                fail_reason: sim.fail_reason,
            });
        }

        // Print results table
        println!();
        println!("  {:<35}  {:>12}  {:>10}  {}", "Call type", "gasUsed", "EGLD cost", "Status");
        println!("  {}", "-".repeat(80));
        let mut max_gas = 0u64;
        for r in &results {
            let egld_cost = r.gas_used as f64 * gp as f64 / 1e18;
            let status = if r.success { "✓ ok".to_string() } else { format!("✗ {}", r.fail_reason) };
            println!("  {:<35}  {:>12}  {:>10.6}  {}", r.call_type, r.gas_used, egld_cost, status);
            if r.success && r.gas_used > max_gas {
                max_gas = r.gas_used;
            }
        }

        let recommended = (max_gas as f64 * 1.10) as u64;
        println!();
        println!("  Max gasUsed      : {}", max_gas);
        println!("  Recommended limit: {} (max × 1.10)", recommended);
        println!();

        if max_gas == 0 {
            println!("⚠️  All simulations failed or returned 0 gas. Check forwarder address,");
            println!("   token identifiers, and that wallets have WEGLD balance.");
        } else {
            let budget_aegld: u128 = 499_000_000_000_000_000_000;
            let cost_per_call = recommended as u128 * gp as u128;
            let total_calls = budget_aegld / cost_per_call;
            println!("  Budget at this gas limit ({} Gwei): ~{} total calls",
                gp / 1_000_000_000, total_calls);
            println!();
            println!("  ✓ Use this in your spam command:");
            println!("      --gas-limit {}", recommended);
        }

        Ok(())
    }
}

/// Raw result from `/transaction/simulate`.
struct SimResponse {
    gas_used: u64,
    success: bool,
    fail_reason: String,
}

/// POST a signed tx to `/transaction/simulate` and return gasUsed.
/// Uses a generous gas_limit for the simulation ceiling (doesn't affect real cost).
async fn simulate_tx(
    client: &reqwest::Client,
    proxy_url: &str,
    tx: &RelayedTransaction,
) -> SimResponse {
    let url = format!("{}/transaction/simulate", proxy_url);

    let resp = match client.post(&url).json(tx).send().await {
        Ok(r) => r,
        Err(e) => {
            return SimResponse { gas_used: 0, success: false, fail_reason: e.to_string() };
        }
    };

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return SimResponse { gas_used: 0, success: false, fail_reason: e.to_string() };
        }
    };

    // Response shape: {"data": {"result": {"gasRemaining": N, "returnCode": "ok", ...}}}
    let result = &body["data"]["result"];

    let gas_limit = tx.gas_limit;
    let gas_remaining = result["gasRemaining"].as_u64().unwrap_or(0);
    let gas_used = gas_limit.saturating_sub(gas_remaining);

    let return_code = result["returnCode"].as_str().unwrap_or("");
    let fail_reason = result["failReason"].as_str().unwrap_or("").to_string();
    let return_msg   = result["returnMessage"].as_str().unwrap_or("").to_string();

    let success = return_code == "ok" && fail_reason.is_empty();
    let fail_str = if !fail_reason.is_empty() {
        fail_reason
    } else if !return_msg.is_empty() {
        return_msg
    } else if !return_code.is_empty() && return_code != "ok" {
        return_code.to_string()
    } else {
        String::new()
    };

    SimResponse { gas_used, success, fail_reason: fail_str }
}
