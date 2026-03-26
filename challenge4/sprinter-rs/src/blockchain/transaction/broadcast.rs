use crate::tui::app::AppLogHandle;
use crate::tui::stats::Stats;
use crate::wallet::{SignedEntry, WalletQueue};
use anyhow::{anyhow, Result};
use futures::future::join_all;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::blockchain::MIN_GAS_PRICE;
use super::log_or_print;
use super::signer::sign_burst_with_wallets;

const NONCE_RESYNC_THRESHOLD: usize = 3;
const MAX_CONSECUTIVE_REJECTIONS: usize = 15;
const MAX_NONCE_DELTA: usize = 100;
/// Max pending txs per wallet in burn-all mode. 10× MAX_NONCE_DELTA keeps
/// multiple batches ready while in-flight txs are confirming, eliminating
/// resync wait time in steady state.
const MAX_BURN_ALL_PENDING: usize = 1000;
const MONITOR_POLL_MS: u64 = 200;
const MONITOR_TIMEOUT_SECS: u64 = 30;
const CAP_STALL_TIMEOUT_SECS: u64 = 10;

/// MultiversX gasPriceModifier: SC execution gas is charged at 1% of gasPrice.
/// Only moveBalanceGas (50_000 + 1_500 × data_bytes) is charged at full price.
const GAS_PRICE_MODIFIER_DENOM: u128 = 100; // 1/100 = 0.01

/// Estimate the initial charge for a tx (what gets deducted from sender balance).
/// MultiversX charges: moveBalanceGas * gasPrice + (gasLimit - moveBalanceGas) * gasPrice / 100
/// `data_b64` is the base64-encoded data field; we estimate raw byte count from it.
fn estimate_tx_cost(value: u128, gas_limit: u64, gas_price: u64, data_b64: Option<&str>) -> u128 {
    // Raw data bytes ≈ base64 length × 3/4 (conservative: round up)
    let data_bytes = data_b64.map(|d| d.len() * 3 / 4).unwrap_or(0);
    let move_balance_gas = 50_000u64 + 1_500u64 * data_bytes as u64;
    let move_gas = move_balance_gas.min(gas_limit);
    let sc_gas = gas_limit.saturating_sub(move_gas);
    let gp = gas_price as u128;
    value + move_gas as u128 * gp + sc_gas as u128 * gp / GAS_PRICE_MODIFIER_DENOM
}

/// Configuration for `broadcast_txs`.
pub struct BroadcastConfig {
    pub batch_size: usize,
    pub sleep_time: u64,
    pub sign_threads: usize,
    pub send_parallelism: usize,
    /// When true, use gas_limit_override_cross instead of gas_limit_override.
    pub cross_shard: bool,
    pub verbose: bool,
}

/// Helper struct for spawned broadcast tasks.
pub struct BroadcastHelper {
    pub(crate) network_url: String,
    pub(crate) client: reqwest::Client,
}

impl BroadcastHelper {
    pub fn new(network_url: String, client: reqwest::Client) -> Self {
        Self { network_url, client }
    }

    pub async fn broadcast_txs(
        &self,
        label: &str,
        queues: Vec<WalletQueue>,
        config: BroadcastConfig,
        stats: Option<Arc<Stats>>,
        log_handle: Option<AppLogHandle>,
    ) {
        BroadcastRun::new(self, label, queues, config, stats, log_handle)
            .execute()
            .await;
    }
}

// ---------------------------------------------------------------------------
// BroadcastRun — per-invocation state and broadcast phases
// ---------------------------------------------------------------------------

enum BurstResult { Ready, WaitForResyncs, SpawnedResyncs, Empty }
enum MonitorResult { Continue, Done }

struct BroadcastRun<'a> {
    helper: &'a BroadcastHelper,
    queues: Vec<WalletQueue>,
    num_wallets: usize,
    // config
    label: String,
    batch_size: usize,
    sleep_time: u64,
    sign_threads: usize,
    send_parallelism: usize,
    verbose: bool,
    cross_shard: bool,
    // stats / logging
    stats: Option<Arc<Stats>>,
    log_handle: Option<AppLogHandle>,
    // resync channel
    resync_tx: mpsc::UnboundedSender<(usize, Result<(u64, u128)>)>,
    resync_rx: mpsc::UnboundedReceiver<(usize, Result<(u64, u128)>)>,
    // per-burst pre-allocated buffers
    burst_entries: Vec<SignedEntry>,
    burst_queue_indices: Vec<usize>,
    is_active: Vec<bool>,
    is_rejected: Vec<bool>,
    min_rejected_nonce: Vec<Option<u64>>,
    rejected_buf: Vec<usize>,
    max_accepted_nonce: Vec<Option<u64>>,
    accepted_count: Vec<usize>,
    // running counters
    total_pending: u64,
    resyncing_count: usize,
    num_txs: usize,
    // cap-stall detection
    cap_stall_start: Vec<Option<Instant>>,
    cap_stall_nonce: Vec<u64>,
}

impl<'a> BroadcastRun<'a> {
    fn new(
        helper: &'a BroadcastHelper,
        label: &str,
        queues: Vec<WalletQueue>,
        config: BroadcastConfig,
        stats: Option<Arc<Stats>>,
        log_handle: Option<AppLogHandle>,
    ) -> Self {
        let BroadcastConfig { batch_size, sleep_time, sign_threads, send_parallelism, verbose, cross_shard } = config;
        let num_wallets = queues.len();
        let burst_size = batch_size * num_wallets;
        let total_pending = queues.iter().map(|q| q.pending.len() as u64).sum();
        let (resync_tx, resync_rx) = mpsc::unbounded_channel();

        if let Some(ref s) = stats { s.set_batch_size(batch_size); }
        // Initialise TUI gas price from the first available tx — check pending first,
        // then fall back to tx_template (used in burn-all / on-the-fly mode where pending starts empty).
        let init_gas = queues.iter().find_map(|q| {
            q.pending.front().map(|(tx, _)| tx.gas_price)
                .or_else(|| q.tx_template.as_ref().map(|(tx, _)| tx.gas_price))
        });
        if let (Some(gas), Some(ref s)) = (init_gas, &stats) {
            s.set_gas_price(gas / 1_000_000);
        }

        Self {
            helper,
            num_wallets,
            label: label.to_string(),
            batch_size, sleep_time, sign_threads, send_parallelism, verbose, cross_shard,
            stats, log_handle,
            resync_tx, resync_rx,
            burst_entries: Vec::with_capacity(burst_size),
            burst_queue_indices: Vec::with_capacity(burst_size),
            is_active: vec![false; num_wallets],
            is_rejected: vec![false; num_wallets],
            min_rejected_nonce: vec![None; num_wallets],
            rejected_buf: Vec::new(),
            max_accepted_nonce: vec![None; num_wallets],
            accepted_count: vec![0; num_wallets],
            total_pending,
            resyncing_count: 0,
            num_txs: 0,
            cap_stall_start: vec![None; num_wallets],
            cap_stall_nonce: vec![0; num_wallets],
            queues,
        }
    }

    fn log(&self, msg: impl AsRef<str>) {
        log_or_print(
            &format!("[{}] {}", self.label, msg.as_ref()),
            &self.stats,
            &self.log_handle,
        );
    }

    // -----------------------------------------------------------------------
    // Top-level loop
    // -----------------------------------------------------------------------

    async fn execute(&mut self) {
        if self.num_wallets == 0 { return; }
        self.log(format!(
            "Broadcasting in bursts of up to {} txs per wallet ({} total per burst)...",
            self.batch_size, self.batch_size * self.num_wallets,
        ));

        'broadcast: loop {
            while self.queues.iter().any(|q| !q.pending.is_empty()) || self.resyncing_count > 0 {
                let burst_start = Instant::now();
                let mut burst_sent_count = 0;

                self.drain_resync_results();
                if let Some(ref s) = self.stats { s.set_deferred(self.total_pending); }

                match self.prepare_burst().await {
                    BurstResult::WaitForResyncs => {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        continue;
                    }
                    BurstResult::SpawnedResyncs => continue,
                    BurstResult::Empty => break,
                    BurstResult::Ready => {}
                }

                self.log(format!("Signing and sending burst of {} transactions...", self.burst_entries.len()));
                self.sign_and_send(&mut burst_sent_count).await;
                self.process_burst_results();
                self.spawn_background_resyncs();
                self.report_and_sleep(burst_start, burst_sent_count).await;
            }

            if let Some(ref s) = self.stats { s.set_deferred(self.total_pending); }

            if self.queues.iter().all(|q| q.in_flight_txs.is_empty()) {
                if self.total_pending > 0 { continue 'broadcast; }
                if self.trigger_refill_resyncs() {
                    // If every idle wallet has insufficient balance they are waiting for
                    // incoming funds — back off to avoid API hammering.
                    // Use effective gas (override if active) so the check matches
                    // what refill_and_trim will actually compute.
                    let gas_override = get_gas_override(&self.stats);
                    let gas_limit_override = get_gas_limit_override(&self.stats, self.cross_shard);
                    let all_waiting = self.queues.iter().all(|q| {
                        let cost = q.tx_template.as_ref().and_then(|(t, _)| {
                            let v: u128 = t.value.parse().ok()?;
                            let eff_gas = if gas_override > 0 { gas_override } else { t.gas_price };
                            let eff_limit = if gas_limit_override > 0 { gas_limit_override } else { t.gas_limit };
                            Some(v + eff_limit as u128 * eff_gas as u128)
                        }).unwrap_or(0);
                        cost == 0 || q.egld_balance < cost
                    });
                    if all_waiting {
                        // 1500ms > 2 × 600ms block time: covers the full cross-shard
                        // round-trip so incoming funds have landed before we re-poll.
                        tokio::time::sleep(Duration::from_millis(1500)).await;
                    }
                    continue 'broadcast;
                }
                break 'broadcast;
            }

            match self.monitor_confirmations().await {
                MonitorResult::Continue => continue 'broadcast,
                MonitorResult::Done => break 'broadcast,
            }
        }

        self.log(format!("\nSuccessfully broadcasted {} transactions!", self.num_txs));
    }

    // -----------------------------------------------------------------------
    // Phase 1: drain completed background nonce re-syncs
    // -----------------------------------------------------------------------

    fn drain_resync_results(&mut self) {
        while let Ok((qi, result)) = self.resync_rx.try_recv() {
            self.queues[qi].resyncing = false;
            self.resyncing_count -= 1;
            let bech32_str = self.queues[qi].sender.bech32.to_bech32_string();
            match result {
                Ok((new_nonce, new_balance)) => self.apply_resync(qi, new_nonce, new_balance),
                Err(e) => self.log(format!("⚠️ Nonce re-sync failed for {}: {}", bech32_str, e)),
            }
        }
    }

    fn apply_resync(&mut self, qi: usize, new_nonce: u64, new_balance: u128) {
        let bech32_str = self.queues[qi].sender.bech32.to_bech32_string();
        self.queues[qi].egld_balance = new_balance;

        // Safe starting nonce: never overwrite accepted-but-unconfirmed nonces.
        let safe_nonce = match self.queues[qi].highest_accepted_nonce {
            Some(han) if new_nonce <= han => han + 1,
            _ => new_nonce,
        };

        let drained = self.queues[qi].drain_confirmed(safe_nonce);
        if drained > 0 {
            if let Some(ref s) = self.stats { s.increment_confirmed(drained as u64); }
        }

        // Drop pending entries the chain already confirmed.
        let front_nonce = self.queues[qi].pending.front().map(|(tx, _)| tx.nonce).unwrap_or(safe_nonce);
        if safe_nonce > front_nonce {
            let drain_count = ((safe_nonce - front_nonce) as usize).min(self.queues[qi].pending.len());
            self.queues[qi].pending.drain(..drain_count);
            self.queues[qi].confirmed_count += drain_count;
            self.total_pending = self.total_pending.saturating_sub(drain_count as u64);
        }

        // Re-number remaining pending entries from safe_nonce.
        for (i, (tx, _)) in self.queues[qi].pending.iter_mut().enumerate() {
            tx.nonce = safe_nonce + i as u64;
            tx.clear_signatures();
        }

        // Recalculate in_flight_count from ground truth.
        self.queues[qi].in_flight_count = safe_nonce.saturating_sub(new_nonce) as usize;
        self.queues[qi].highest_accepted_nonce = if self.queues[qi].in_flight_count > 0 {
            Some(safe_nonce - 1)
        } else {
            None
        };
        self.queues[qi].consecutive_rejections = 0;

        self.handle_cap_stall(qi, new_nonce, &bech32_str);
        self.refill_and_trim(qi, safe_nonce, &bech32_str);

        let rebuilt = self.queues[qi].pending.len();
        let in_flight = self.queues[qi].in_flight_count;
        self.log(format!(
            "🔄 Re-synced nonce for {} to {} (safe: {}, {} txs rebuilt, {} in-flight).",
            bech32_str, new_nonce, safe_nonce, rebuilt, in_flight,
        ));
    }

    /// Detect and recover from the cap-stall deadlock: fully capped with no chain progress
    /// means in-flight txs were silently evicted — re-queue them.
    fn handle_cap_stall(&mut self, qi: usize, new_nonce: u64, bech32_str: &str) {
        if self.queues[qi].in_flight_count < MAX_NONCE_DELTA {
            self.cap_stall_start[qi] = None;
            return;
        }
        if self.cap_stall_start[qi].is_none() || new_nonce > self.cap_stall_nonce[qi] {
            self.cap_stall_start[qi] = Some(Instant::now());
            self.cap_stall_nonce[qi] = new_nonce;
            return;
        }
        if self.cap_stall_start[qi].unwrap().elapsed().as_secs() < CAP_STALL_TIMEOUT_SECS {
            return;
        }
        let count = self.queues[qi].in_flight_txs.len();
        if count > 0 {
            self.log(format!(
                "⚠️ {} capped {}s with no chain progress — re-queuing {} in-flight txs.",
                bech32_str, CAP_STALL_TIMEOUT_SECS, count,
            ));
            let evicted: Vec<_> = self.queues[qi].in_flight_txs.drain(..).rev().collect();
            for (mut tx, relayer) in evicted {
                tx.clear_signatures();
                self.queues[qi].pending.push_front((tx, relayer));
                self.total_pending += 1;
            }
        }
        self.queues[qi].clear_in_flight_state();
        self.cap_stall_start[qi] = None;
    }

    /// Refill pending for target-mode wallets, then trim to what the wallet can afford.
    fn refill_and_trim(&mut self, qi: usize, safe_nonce: u64, bech32_str: &str) {
        // Burn-all: generate as many txs as the live balance allows, accounting for
        // already-committed in-flight+pending txs. Gas override is factored in so
        // affordability is never underestimated.
        if self.queues[qi].burn_all {
            if let Some((tmpl_tx, tmpl_relayer)) = self.queues[qi].tx_template.clone() {
                let tmpl_b = self.queues[qi].tx_template_b.clone();
                let gas_override = get_gas_override(&self.stats);
                let gas_limit_override = get_gas_limit_override(&self.stats, self.cross_shard);
                let effective_gas = if gas_override > 0 { gas_override } else { tmpl_tx.gas_price };
                let effective_limit = if gas_limit_override > 0 { gas_limit_override } else { tmpl_tx.gas_limit };
                let value: u128 = tmpl_tx.value.parse().unwrap_or(0);
                let cost_per_tx = estimate_tx_cost(value, effective_limit, effective_gas, tmpl_tx.data.as_deref());
                if cost_per_tx == 0 { return; }
                let already_queued = self.queues[qi].in_flight_count + self.queues[qi].pending.len();
                let affordable = (self.queues[qi].egld_balance / cost_per_tx) as usize;
                // Cap total pending to MAX_NONCE_DELTA. The previous .min(MAX_NONCE_DELTA)
                // only capped the increment — a proactive resync firing with 99 pending
                // would add 100 more → 199, growing by ~100 each cycle until OOM.
                let max_new = MAX_BURN_ALL_PENDING.saturating_sub(self.queues[qi].pending.len());
                let to_refill = affordable.saturating_sub(already_queued).min(max_new);
                if to_refill == 0 {
                    self.log(format!("⏳ {} balance {} aEGLD — waiting for incoming funds.",
                        bech32_str, self.queues[qi].egld_balance));
                    return;
                }
                let start_nonce = safe_nonce + self.queues[qi].pending.len() as u64;
                for i in 0..to_refill {
                    // Alternate between template A and B (if present) to keep both
                    // destination shards funded in cross-shard burn-all mode.
                    let (tmpl, relayer) = if i % 2 == 1 {
                        if let Some((ref tx_b, ref rel_b)) = tmpl_b {
                            (tx_b.clone(), rel_b.clone())
                        } else {
                            (tmpl_tx.clone(), tmpl_relayer.clone())
                        }
                    } else {
                        (tmpl_tx.clone(), tmpl_relayer.clone())
                    };
                    let mut new_tx = tmpl;
                    new_tx.nonce = start_nonce + i as u64;
                    new_tx.clear_signatures();
                    self.queues[qi].pending.push_back((new_tx, relayer));
                    self.total_pending += 1;
                }
                self.log(format!("🔥 {} burn-all: refilled {} txs (balance: {} aEGLD).",
                    bech32_str, to_refill, self.queues[qi].egld_balance));
            }
            return;
        }

        // Refill
        if self.queues[qi].target > 0 {
            let already_queued = self.queues[qi].in_flight_count + self.queues[qi].pending.len();
            let remaining = self.queues[qi].target
                .saturating_sub(self.queues[qi].confirmed_count + already_queued);
            if remaining > 0 {
                if let Some((tmpl_tx, tmpl_relayer)) = self.queues[qi].tx_template.clone() {
                    let value: u128 = tmpl_tx.value.parse().unwrap_or(0);
                    let cost_per_tx = estimate_tx_cost(value, tmpl_tx.gas_limit, tmpl_tx.gas_price, tmpl_tx.data.as_deref());
                    let affordable = if cost_per_tx > 0 {
                        (self.queues[qi].egld_balance / cost_per_tx) as usize
                    } else {
                        remaining
                    };
                    let to_refill = affordable.min(remaining);
                    if to_refill == 0 {
                        self.log(format!("⚠️ {} balance ({} aEGLD) too low to refill. Marking done.",
                            bech32_str, self.queues[qi].egld_balance));
                        self.queues[qi].target = self.queues[qi].confirmed_count;
                    } else {
                        if to_refill < remaining {
                            self.log(format!("⚠️ {} balance ({} aEGLD) covers only {}/{} remaining txs.",
                                bech32_str, self.queues[qi].egld_balance, to_refill, remaining));
                            self.queues[qi].target = self.queues[qi].confirmed_count
                                + self.queues[qi].in_flight_count
                                + self.queues[qi].pending.len()
                                + to_refill;
                        }
                        let start_nonce = safe_nonce + self.queues[qi].pending.len() as u64;
                        for i in 0..to_refill {
                            let mut new_tx = tmpl_tx.clone();
                            new_tx.nonce = start_nonce + i as u64;
                            new_tx.clear_signatures();
                            self.queues[qi].pending.push_back((new_tx, tmpl_relayer.clone()));
                            self.total_pending += 1;
                        }
                        let (confirmed, target, balance) = (
                            self.queues[qi].confirmed_count,
                            self.queues[qi].target,
                            self.queues[qi].egld_balance,
                        );
                        self.log(format!("🎯 Refilled {} txs for {} (confirmed: {}/{}, balance: {} aEGLD).",
                            to_refill, bech32_str, confirmed, target, balance));
                    }
                }
            }
        }

        // Trim: affordability measured at MIN_GAS_PRICE (floor) so txs are only dropped
        // when the wallet truly cannot send at all.
        if self.queues[qi].pending.is_empty() { return; }
        let (value, gas_limit) = self.queues[qi].pending.front()
            .map(|(tx, _)| (tx.value.parse::<u128>().unwrap_or(0), tx.gas_limit))
            .unwrap();
        let cost_at_min = value + gas_limit as u128 * MIN_GAS_PRICE as u128;
        if cost_at_min == 0 { return; }

        let affordable = (self.queues[qi].egld_balance / cost_at_min) as usize;
        if affordable >= self.queues[qi].pending.len() { return; }

        let drop_count = self.queues[qi].pending.len() - affordable;
        self.queues[qi].pending.truncate(affordable);
        self.total_pending = self.total_pending.saturating_sub(drop_count as u64);
        self.queues[qi].target = self.queues[qi].confirmed_count
            + self.queues[qi].in_flight_count
            + affordable;
        if let Some(ref s) = self.stats { s.decrement_planned(drop_count as u64); }

        let balance = self.queues[qi].egld_balance;
        if affordable == 0 {
            self.log(format!("💸 {} balance {} aEGLD insufficient — dropping all {} pending txs.",
                bech32_str, balance, drop_count));
        } else {
            self.log(format!("💸 {} balance {} aEGLD covers {} more txs at min gas — trimmed {} pending.",
                bech32_str, balance, affordable, drop_count));
        }
    }

    // -----------------------------------------------------------------------
    // Phase 2: build the next burst
    // -----------------------------------------------------------------------

    async fn prepare_burst(&mut self) -> BurstResult {
        let gas_override = get_gas_override(&self.stats);
        let gas_limit_override = get_gas_limit_override(&self.stats, self.cross_shard);
        if gas_override > 0 {
            self.log(format!("Using gas price override: {} atomic units", gas_override));
        }
        if gas_limit_override > 0 {
            self.log(format!("Using gas limit override: {}M", gas_limit_override / 1_000_000));
        }

        self.burst_entries.clear();
        self.burst_queue_indices.clear();
        self.is_active.fill(false);
        self.is_rejected.fill(false);
        self.min_rejected_nonce.fill(None);
        self.max_accepted_nonce.fill(None);
        self.accepted_count.fill(0);

        for (qi, wq) in self.queues.iter_mut().enumerate() {
            if wq.resyncing { continue; }
            let can_send = self.batch_size.min(MAX_NONCE_DELTA.saturating_sub(wq.in_flight_count));
            for _ in 0..can_send {
                match wq.pending.pop_front() {
                    Some((mut tx, relayer)) => {
                        if gas_override > 0 { tx.gas_price = gas_override; }
                        if gas_limit_override > 0 { tx.gas_limit = gas_limit_override; }
                        self.burst_entries.push((tx, wq.sender.clone(), relayer));
                        self.burst_queue_indices.push(qi);
                        self.is_active[qi] = true;
                        self.total_pending = self.total_pending.saturating_sub(1);
                    }
                    None => break,
                }
            }
        }

        if self.burst_entries.is_empty() {
            return self.handle_empty_burst().await;
        }
        BurstResult::Ready
    }

    async fn handle_empty_burst(&mut self) -> BurstResult {
        if self.resyncing_count > 0 {
            return BurstResult::WaitForResyncs;
        }
        if self.total_pending > 0 {
            // Wallets have pending txs but are blocked by the nonce delta cap.
            // Force a resync on each capped wallet to reconcile in_flight_count.
            let mut spawned_any = false;
            for qi in 0..self.num_wallets {
                if self.queues[qi].resyncing || self.queues[qi].pending.is_empty() { continue; }
                if self.queues[qi].in_flight_count + self.batch_size <= MAX_NONCE_DELTA { continue; }
                let bech32 = self.queues[qi].sender.bech32.to_bech32_string();
                self.log(format!("🔄 Nonce delta cap reached — re-syncing {}...", bech32));
                self.queues[qi].resyncing = true;
                self.resyncing_count += 1;
                spawned_any = true;
                self.spawn_resync(qi, bech32);
            }
            if spawned_any { return BurstResult::SpawnedResyncs; }
        }
        BurstResult::Empty
    }

    // -----------------------------------------------------------------------
    // Phase 3: sign and send
    // -----------------------------------------------------------------------

    async fn sign_and_send(&mut self, burst_sent_count: &mut usize) {
        tokio::task::block_in_place(|| {
            sign_burst_with_wallets(&mut self.burst_entries, self.sign_threads, &self.stats, &self.log_handle);
        });
        self.helper.send_all_with_wallets(
            &self.label, &self.burst_entries, &mut self.rejected_buf,
            burst_sent_count, &mut self.num_txs,
            self.send_parallelism, self.verbose, &self.stats, &self.log_handle,
        ).await;
    }

    // -----------------------------------------------------------------------
    // Phase 4: process rejections, update in-flight state
    // -----------------------------------------------------------------------

    fn process_burst_results(&mut self) {
        // Find min rejected nonce per wallet.
        for &i in &self.rejected_buf {
            let qi = self.burst_queue_indices[i];
            let nonce = self.burst_entries[i].0.nonce;
            let slot = &mut self.min_rejected_nonce[qi];
            *slot = Some(slot.map_or(nonce, |m| m.min(nonce)));
            self.is_rejected[qi] = true;
        }

        // Record accepted entries: update in-flight tracking and save for monitoring.
        for (i, entry) in self.burst_entries.iter().enumerate() {
            let qi = self.burst_queue_indices[i];
            let nonce = entry.0.nonce;
            if self.min_rejected_nonce[qi].map_or(true, |min_n| nonce < min_n) {
                self.accepted_count[qi] += 1;
                let slot = &mut self.max_accepted_nonce[qi];
                *slot = Some(slot.map_or(nonce, |m: u64| m.max(nonce)));
                self.queues[qi].in_flight_txs.push_back((entry.0.clone(), entry.2.clone()));
            }
        }

        // Push-back cascade: re-queue entries at or after the min rejected nonce.
        // Process in reverse so push_front preserves nonce ordering.
        let burst_len = self.burst_entries.len();
        for rev_i in 0..burst_len {
            let i = burst_len - 1 - rev_i;
            let entry = self.burst_entries.pop().unwrap();
            let qi = self.burst_queue_indices[i];
            if let Some(min_nonce) = self.min_rejected_nonce[qi] {
                if entry.0.nonce >= min_nonce {
                    let mut tx = entry.0;
                    tx.clear_signatures();
                    self.queues[qi].pending.push_front((tx, entry.2));
                    self.total_pending += 1;
                }
            }
        }

        // Update in_flight_count and highest_accepted_nonce.
        for qi in 0..self.num_wallets {
            if !self.is_active[qi] { continue; }
            self.queues[qi].in_flight_count += self.accepted_count[qi];
            if let Some(new_max) = self.max_accepted_nonce[qi] {
                let prev = self.queues[qi].highest_accepted_nonce;
                self.queues[qi].highest_accepted_nonce =
                    Some(prev.map_or(new_max, |old| old.max(new_max)));
            }
        }

        // Update consecutive_rejections; force-resync persistent rejecters.
        for qi in 0..self.num_wallets {
            if !self.is_active[qi] { continue; }
            self.queues[qi].consecutive_rejections = if self.is_rejected[qi] {
                self.queues[qi].consecutive_rejections + 1
            } else {
                0
            };
            if self.queues[qi].consecutive_rejections >= MAX_CONSECUTIVE_REJECTIONS
                && !self.queues[qi].pending.is_empty()
            {
                let bech32 = self.queues[qi].sender.bech32.to_bech32_string();
                let n = self.queues[qi].consecutive_rejections;
                self.log(format!("⚠️ Wallet {} rejected {} times — forcing re-sync and retrying.", bech32, n));
                self.queues[qi].consecutive_rejections = 0;
                if !self.queues[qi].resyncing {
                    self.queues[qi].resyncing = true;
                    self.resyncing_count += 1;
                    self.spawn_resync(qi, bech32);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Phase 5: spawn background nonce re-syncs
    // -----------------------------------------------------------------------

    fn spawn_background_resyncs(&mut self) {
        for qi in 0..self.num_wallets {
            if needs_resync(&self.queues[qi], self.batch_size) {
                let bech32 = self.queues[qi].sender.bech32.to_bech32_string();
                self.log(format!("🔄 Spawning background nonce re-sync for {}...", bech32));
                self.queues[qi].resyncing = true;
                self.resyncing_count += 1;
                self.spawn_resync(qi, bech32);
            } else if needs_idle_refill_resync(&self.queues[qi], get_gas_override(&self.stats)) {
                // burn-all wallet with no pending/in-flight — refill it independently
                // of whether other wallets are still being monitored.
                let bech32 = self.queues[qi].sender.bech32.to_bech32_string();
                self.queues[qi].resyncing = true;
                self.resyncing_count += 1;
                self.spawn_resync(qi, bech32);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Phase 6: report burst stats and sleep
    // -----------------------------------------------------------------------

    async fn report_and_sleep(&mut self, burst_start: Instant, burst_sent_count: usize) {
        let burst_elapsed = burst_start.elapsed().as_secs_f64();
        if let Some(ref s) = self.stats { s.record_burst(burst_sent_count, burst_elapsed); }
        self.log(format!("-> Burst: {} txs sent in {:.2}s.", burst_sent_count, burst_elapsed));

        if self.total_pending > 0 {
            let time_to_sleep = self.sleep_time as f64 - burst_elapsed;
            if time_to_sleep > 0.0 {
                self.log(format!("-> Sleeping {:.2}s to fill the remaining {}s block window...",
                    time_to_sleep, self.sleep_time));
                tokio::time::sleep(Duration::from_secs_f64(time_to_sleep)).await;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Monitoring phase: poll on-chain nonces until all txs confirm or time out
    // -----------------------------------------------------------------------

    async fn monitor_confirmations(&mut self) -> MonitorResult {
        let count = self.queues.iter().filter(|q| !q.in_flight_txs.is_empty()).count();
        self.log(format!("🔍 Monitoring {} wallet(s) for confirmation (timeout: {}s)...",
            count, MONITOR_TIMEOUT_SECS));

        // Initialise stall-detection state once per monitoring session.
        // last_progress is set to now only for wallets that currently have in-flight txs;
        // the timeout is measured from the first time we observe no progress, not from entry.
        let monitor_start = Instant::now();
        let mut last_progress: Vec<Instant> = (0..self.num_wallets).map(|_| monitor_start).collect();
        let mut last_nonces: Vec<u64> = (0..self.num_wallets)
            .map(|qi| self.queues[qi].in_flight_txs.front().map_or(u64::MAX, |(tx, _)| tx.nonce))
            .collect();
        let mut monitored: Vec<usize> = Vec::with_capacity(self.num_wallets);
        let mut fetch_futs: Vec<_> = Vec::with_capacity(self.num_wallets);

        loop {
            monitored.clear();
            monitored.extend((0..self.num_wallets).filter(|&qi| !self.queues[qi].in_flight_txs.is_empty()));

            if monitored.is_empty() {
                // Drain any resyncs that completed while we were monitoring.
                self.drain_resync_results();
                if self.resyncing_count > 0 {
                    // Proactive resyncs spawned during monitoring haven't landed yet — wait briefly.
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
                if self.queues.iter().any(|q| !q.pending.is_empty()) { return MonitorResult::Continue; }
                if self.trigger_refill_resyncs() { return MonitorResult::Continue; }
                return MonitorResult::Done;
            }

            fetch_futs.clear();
            fetch_futs.extend(monitored.iter().map(|&qi| {
                let client = self.helper.client.clone();
                let base_url = self.helper.network_url.clone();
                let bech32 = self.queues[qi].sender.bech32.to_bech32_string();
                async move { fetch_account_raw(client, base_url, bech32).await }
            }));
            let results = join_all(fetch_futs.drain(..)).await;

            let mut any_timed_out = false;
            for (&qi, result) in monitored.iter().zip(results.into_iter()) {
                match result {
                    Ok((on_chain_nonce, balance)) => {
                        self.queues[qi].egld_balance = balance;
                        let drained = self.queues[qi].drain_confirmed(on_chain_nonce);
                        if drained > 0 {
                            if let Some(ref s) = self.stats { s.increment_confirmed(drained as u64); }
                        }
                        if self.queues[qi].in_flight_txs.is_empty() {
                            self.queues[qi].clear_in_flight_state();
                            let bech32 = self.queues[qi].sender.bech32.to_bech32_string();
                            self.log(format!("✅ {} fully confirmed.", bech32));
                            // Pipeline: start refill resync now so pending is ready for the next burst.
                            if !self.queues[qi].resyncing && self.queues[qi].tx_template.is_some() {
                                self.queues[qi].resyncing = true;
                                self.resyncing_count += 1;
                                self.spawn_resync(qi, bech32);
                            }
                            continue;
                        }
                        if on_chain_nonce > last_nonces[qi] {
                            last_nonces[qi] = on_chain_nonce;
                            last_progress[qi] = Instant::now();
                        }
                        if last_progress[qi].elapsed().as_secs() >= MONITOR_TIMEOUT_SECS {
                            any_timed_out = true;
                        }
                    }
                    Err(e) => {
                        let bech32 = self.queues[qi].sender.bech32.to_string();
                        self.log(format!("⚠️ Monitor fetch failed for {}: {}", bech32, e));
                    }
                }
            }

            // Process any resyncs that landed while we were fetching nonces.
            self.drain_resync_results();

            // Don't let one stuck wallet block wallets that are ready to send.
            if self.queues.iter().any(|q| !q.pending.is_empty()) {
                return MonitorResult::Continue;
            }

            if any_timed_out {
                self.handle_monitor_timeout(&last_progress);
                return MonitorResult::Continue;
            }

            tokio::time::sleep(Duration::from_millis(MONITOR_POLL_MS)).await;
        }
    }

    fn handle_monitor_timeout(&mut self, last_progress: &[Instant]) {
        for qi in 0..self.num_wallets {
            if self.queues[qi].in_flight_txs.is_empty() { continue; }
            if last_progress[qi].elapsed().as_secs() < MONITOR_TIMEOUT_SECS { continue; }
            let count = self.queues[qi].in_flight_txs.len();
            let bech32 = self.queues[qi].sender.bech32.to_string();
            if self.queues[qi].egld_balance == 0 {
                self.queues[qi].in_flight_txs.drain(..);
                self.queues[qi].clear_in_flight_state();
                self.log(format!("❌ {} out of funds — dropping {} unconfirmed txs.", bech32, count));
            } else {
                let evicted: Vec<_> = self.queues[qi].in_flight_txs.drain(..).rev().collect();
                self.queues[qi].clear_in_flight_state();
                self.queues[qi].consecutive_rejections = 0;
                for (mut tx, relayer) in evicted {
                    tx.clear_signatures();
                    self.queues[qi].pending.push_front((tx, relayer));
                    self.total_pending += 1;
                }
                self.log(format!("⚠️ {} stalled — re-queuing {} evicted txs for retry.", bech32, count));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Spawn a background fetch_account_raw task and send the result on resync_tx.
    fn spawn_resync(&self, qi: usize, bech32: String) {
        let tx = self.resync_tx.clone();
        let client = self.helper.client.clone();
        let base_url = self.helper.network_url.clone();
        tokio::spawn(async move {
            let result = fetch_account_raw(client, base_url, bech32).await;
            let _ = tx.send((qi, result));
        });
    }

    /// Spawn background re-syncs for target-mode wallets with nothing left to send.
    /// Returns true if any were spawned (caller should `continue 'broadcast`).
    fn trigger_refill_resyncs(&mut self) -> bool {
        let mut any = false;
        for qi in 0..self.num_wallets {
            if self.queues[qi].target == 0 || self.queues[qi].confirmed_count >= self.queues[qi].target { continue; }
            if self.queues[qi].resyncing || self.queues[qi].tx_template.is_none() { continue; }
            if !self.queues[qi].pending.is_empty()
                || !self.queues[qi].in_flight_txs.is_empty()
                || self.queues[qi].in_flight_count > 0
            {
                continue;
            }
            let bech32 = self.queues[qi].sender.bech32.to_bech32_string();
            self.queues[qi].resyncing = true;
            self.resyncing_count += 1;
            self.spawn_resync(qi, bech32);
            any = true;
        }
        any
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

fn needs_resync(wq: &WalletQueue, _batch_size: usize) -> bool {
    if wq.resyncing || wq.pending.is_empty() { return false; }
    let by_rejections = wq.consecutive_rejections > 0
        && wq.consecutive_rejections < MAX_CONSECUTIVE_REJECTIONS
        && wq.consecutive_rejections % NONCE_RESYNC_THRESHOLD == 0;
    let by_nonce_delta = wq.in_flight_count >= MAX_NONCE_DELTA * 3 / 4;
    by_rejections || by_nonce_delta
}

/// True for a burn-all wallet that is fully idle and likely affordable — needs a
/// proactive resync so it can refill independently of other wallets' in-flight state.
/// `gas_override` is the active TUI gas override (0 if none).
fn needs_idle_refill_resync(wq: &WalletQueue, _gas_override: u64) -> bool {
    if !wq.burn_all { return false; }
    if wq.resyncing { return false; }
    if wq.tx_template.is_none() { return false; }
    if !wq.pending.is_empty() || !wq.in_flight_txs.is_empty() || wq.in_flight_count > 0 { return false; }
    // Do not gate on last-known balance: any stale balance (including near-zero)
    // may be outdated if cross-shard funds arrived since the last resync.
    // refill_and_trim will discard the wallet if the fresh balance is still insufficient.
    true
}

pub(crate) async fn fetch_account_raw(
    client: reqwest::Client,
    base_url: String,
    bech32: String,
) -> Result<(u64, u128)> {
    let url = format!("{}/address/{}", base_url, bech32);
    let body: Value = client.get(&url).send().await?.json().await?;
    let nonce = body.pointer("/data/account/nonce")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow!("missing /data/account/nonce in response for {}", bech32))?;
    let balance = body.pointer("/data/account/balance")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u128>().ok())
        .ok_or_else(|| anyhow!("missing/invalid /data/account/balance in response for {}", bech32))?;
    Ok((nonce, balance))
}

fn get_gas_override(stats: &Option<Arc<Stats>>) -> u64 {
    stats.as_ref().map(|s| s.get_gas_price_override()).unwrap_or(0)
}

fn get_gas_limit_override(stats: &Option<Arc<Stats>>, cross_shard: bool) -> u64 {
    stats.as_ref().map(|s| {
        if cross_shard {
            s.get_gas_limit_override_cross()
        } else {
            s.get_gas_limit_override()
        }
    }).unwrap_or(0)
}
