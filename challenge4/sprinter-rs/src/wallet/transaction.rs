use super::entry::WalletEntry;
use ed25519_dalek::Signer;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Relayed transaction type (extends SDK Transaction with relayer fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayedTransaction {
    pub nonce: u64,
    pub value: String,
    pub receiver: multiversx_chain_core::std::Bech32Address,
    pub sender: multiversx_chain_core::std::Bech32Address,
    pub gas_price: u64,
    pub gas_limit: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(rename = "chainID")]
    pub chain_id: String,
    pub version: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub options: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relayer: Option<multiversx_chain_core::std::Bech32Address>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relayer_signature: Option<String>,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

impl RelayedTransaction {
    /// Clear both signature fields (used before re-signing after pushback or eviction retry).
    pub fn clear_signatures(&mut self) {
        self.signature = None;
        self.relayer_signature = None;
    }

    /// Create a new relayed transaction from parts.
    pub fn from_parts(
        nonce: u64,
        value: u128,
        receiver: &multiversx_chain_core::std::Bech32Address,
        sender: &multiversx_chain_core::std::Bech32Address,
        gas_price: u64,
        gas_limit: u64,
        chain_id: &str,
        version: u32,
        relayer_addr: Option<&multiversx_chain_core::std::Bech32Address>,
    ) -> Self {
        // Relayed v3 requires an extra base_cost and version 2
        let (effective_gas_limit, effective_version) = if relayer_addr.is_some() {
            (gas_limit + 50_000, 2)
        } else {
            (gas_limit, version)
        };

        Self {
            nonce,
            value: value.to_string(),
            receiver: receiver.clone(),
            sender: sender.clone(),
            gas_price,
            gas_limit: effective_gas_limit,
            data: None,
            signature: None,
            chain_id: chain_id.to_string(),
            version: effective_version,
            options: 0,
            relayer: relayer_addr.cloned(),
            relayer_signature: None,
        }
    }

    /// Build the JSON bytes that need to be signed.
    /// Both signature fields have `skip_serializing_if = "Option::is_none"`, so when both
    /// are None (the common case) we serialize directly without cloning.
    fn signable_bytes(&self) -> Vec<u8> {
        if self.signature.is_none() && self.relayer_signature.is_none() {
            return serde_json::to_string(self).unwrap().into_bytes();
        }
        let mut signable = self.clone();
        signable.signature = None;
        signable.relayer_signature = None;
        serde_json::to_string(&signable).unwrap().into_bytes()
    }

    /// Sign both sender and (optionally) relayer in one pass, computing signable bytes once.
    /// Use this in the hot signing loop instead of separate sign_sender/sign_relayer calls.
    pub fn sign_both(&mut self, sender_entry: &WalletEntry, relayer_entry: Option<&WalletEntry>) {
        let tx_bytes = self.signable_bytes();
        let sig = sender_entry.signing_key.sign(&tx_bytes);
        self.signature = Some(hex::encode(sig.to_bytes()));
        if let Some(rel) = relayer_entry {
            let sig = rel.signing_key.sign(&tx_bytes);
            self.relayer_signature = Some(hex::encode(sig.to_bytes()));
        }
    }

    /// Sign the transaction as the sender.
    /// For relayed v3, this includes the `relayer` field in the signed payload.
    pub fn sign_sender(&mut self, sender_entry: &WalletEntry) {
        let tx_bytes = self.signable_bytes();
        let sig = sender_entry.signing_key.sign(&tx_bytes);
        self.signature = Some(hex::encode(sig.to_bytes()));
    }

    /// Sign the transaction as the relayer.
    /// The relayer signs the same serializable form as the sender
    /// (full tx JSON with `relayer` field, but without signatures).
    pub fn sign_relayer(&mut self, relayer_entry: &WalletEntry) {
        let tx_bytes = self.signable_bytes();
        let sig = relayer_entry.signing_key.sign(&tx_bytes);
        self.relayer_signature = Some(hex::encode(sig.to_bytes()));
    }
}

/// Type alias for a signed transaction entry: (tx, sender_wallet, optional_relayer_wallet).
pub type SignedEntry = (RelayedTransaction, Arc<WalletEntry>, Option<Arc<WalletEntry>>);
