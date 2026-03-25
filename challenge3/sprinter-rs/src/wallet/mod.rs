mod entry;
mod queue;
mod transaction;

pub use entry::{compute_shard, create_wallets, find_relayer_account, WalletEntry, WalletManager};
pub use queue::WalletQueue;
pub use transaction::{RelayedTransaction, SignedEntry};

#[cfg(test)]
mod signing_tests {
    use super::*;
    use multiversx_chain_core::std::Bech32Address;
    use multiversx_sdk::wallet::Wallet;

    #[test]
    fn compare_signing_methods() {
        let seed_hex = "b8ca6f8203fb4b545a8e83c5384da033c415db155b53fb5b8efa7ff5a0f0d828";
        let seed_bytes = hex::decode(seed_hex).unwrap();
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&seed_bytes);

        let wallet = Wallet::from_private_key(seed_hex).unwrap();
        let entry = WalletEntry::new(wallet, seed);
        let sender_bech32 = entry.bech32.clone();

        let mut tx = RelayedTransaction::from_parts(
            0,
            1000000000000000000,
            &sender_bech32,
            &sender_bech32,
            1000000000,
            50000,
            "T",
            2,
            None,
        );

        // Verify our JSON serialization matches the SDK's Transaction format.
        let our_json = serde_json::to_string(&tx).unwrap();
        let sdk_tx = multiversx_sdk::data::transaction::Transaction {
            nonce: tx.nonce,
            value: tx.value.clone(),
            receiver: tx.receiver.clone(),
            sender: tx.sender.clone(),
            gas_price: tx.gas_price,
            gas_limit: tx.gas_limit,
            data: None,
            signature: None,
            chain_id: tx.chain_id.clone(),
            version: tx.version,
            options: 0,
        };
        let sdk_json = serde_json::to_string(&sdk_tx).unwrap();
        println!("\nOur JSON: {}", our_json);
        println!("SDK JSON: {}", sdk_json);
        assert_eq!(our_json, sdk_json, "JSON payloads for non-relayed tx must match");

        // Verify our signature matches the SDK's.
        tx.sign_sender(&entry);
        let our_sig = tx.signature.clone().unwrap();
        let sdk_sig = hex::encode(wallet.sign_tx(&sdk_tx));
        println!("Our sig: {}", our_sig);
        println!("SDK sig: {}", sdk_sig);
        assert_eq!(our_sig, sdk_sig, "Signatures for non-relayed tx must match");
    }

    #[test]
    fn test_compute_shard_matches_network() {
        let sender_bech32 = Bech32Address::from_bech32_string(
            "erd14cr59upt6f88czqy685rv7nx3tdew3j99crmytfzx3qxyltcdy6qq355us".to_string(),
        );
        let receiver_bech32 = Bech32Address::from_bech32_string(
            "erd1cn22wy8crxzrjvl2wmqy72e67gp44g5cfgqgjed234nmq6qp4nxsh4zena".to_string(),
        );

        let sender_shard = compute_shard(&sender_bech32.to_address(), 3);
        let receiver_shard = compute_shard(&receiver_bech32.to_address(), 3);

        assert_eq!(sender_shard, 0, "Sender should be in shard 0 per network");
        assert_eq!(receiver_shard, 1, "Receiver should be in shard 1 per network");
    }
}
