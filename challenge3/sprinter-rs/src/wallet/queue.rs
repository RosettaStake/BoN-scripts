use super::entry::WalletEntry;
use super::transaction::RelayedTransaction;
use std::{collections::VecDeque, sync::Arc};

/// Per-wallet transaction queue. Owns pending txs in nonce-ascending order.
/// The relayer is stored per-tx (as Option<Arc<WalletEntry>>) to support random-relayer mode.
pub struct WalletQueue {
    pub sender: Arc<WalletEntry>,
    pub pending: VecDeque<(RelayedTransaction, Option<Arc<WalletEntry>>)>,
    pub consecutive_rejections: usize,
    /// True while a background nonce re-sync task is in flight for this wallet.
    /// The wallet is skipped in burst builds until the re-sync completes.
    pub resyncing: bool,
    /// Number of txs that received a hash (accepted by the node) but are not yet confirmed on-chain.
    pub in_flight_count: usize,
    /// Highest nonce that was accepted by the node but not yet confirmed on-chain.
    pub highest_accepted_nonce: Option<u64>,
    /// Full tx data for accepted-but-unconfirmed txs, in nonce-ascending order.
    /// Used by the post-send monitoring phase to re-queue silently evicted txs.
    pub in_flight_txs: VecDeque<(RelayedTransaction, Option<Arc<WalletEntry>>)>,
    /// Total on-chain confirmations accumulated for this wallet across all runs.
    pub confirmed_count: usize,
    /// Target number of confirmations. 0 = no target (legacy: run until pending exhausted).
    pub target: usize,
    /// Prototype tx used to generate refill batches when confirmed_count < target.
    /// Nonce is irrelevant — overwritten on generation. All other fields are cloned as-is.
    pub tx_template: Option<(RelayedTransaction, Option<Arc<WalletEntry>>)>,
    /// Optional second prototype for burn-all refill (cross-shard: alternates with tx_template
    /// so both destination shards stay funded and all 6 pairs remain active).
    pub tx_template_b: Option<(RelayedTransaction, Option<Arc<WalletEntry>>)>,
    /// Last known EGLD balance in atomic units (updated on each nonce re-sync).
    pub egld_balance: u128,
    /// When true, run in burn-all mode: generate txs on the fly based on live balance,
    /// loop forever (target = usize::MAX), skip balance trim.
    pub burn_all: bool,
}

impl WalletQueue {
    pub fn new(sender: Arc<WalletEntry>) -> Self {
        Self {
            sender,
            pending: VecDeque::new(),
            consecutive_rejections: 0,
            resyncing: false,
            in_flight_count: 0,
            highest_accepted_nonce: None,
            in_flight_txs: VecDeque::new(),
            confirmed_count: 0,
            target: 0,
            tx_template: None,
            tx_template_b: None,
            egld_balance: 0,
            burn_all: false,
        }
    }

    pub fn push(&mut self, tx: RelayedTransaction, relayer: Option<Arc<WalletEntry>>) {
        self.pending.push_back((tx, relayer));
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Drain all in-flight txs whose nonce was confirmed (nonce < threshold).
    /// Increments confirmed_count and returns the number drained.
    pub fn drain_confirmed(&mut self, threshold: u64) -> usize {
        let mut count = 0;
        while self.in_flight_txs.front().map_or(false, |(tx, _)| tx.nonce < threshold) {
            self.in_flight_txs.pop_front();
            self.confirmed_count += 1;
            if self.in_flight_count > 0 {
                self.in_flight_count -= 1;
            }
            count += 1;
        }
        count
    }

    /// Reset in-flight tracking state after full confirmation or eviction re-queue.
    pub fn clear_in_flight_state(&mut self) {
        self.in_flight_count = 0;
        self.highest_accepted_nonce = None;
    }
}
