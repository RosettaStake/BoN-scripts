//! Blockchain module for interacting with the MultiversX network.

pub mod nonce;
pub mod transaction;

/// Minimum gas price in atomic units (1 Gwei = 1_000_000_000).
pub(crate) const MIN_GAS_PRICE: u64 = 1_000_000_000;
