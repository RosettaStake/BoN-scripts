use anyhow::{anyhow, bail, Context, Result};
use ed25519_dalek::SigningKey;
use multiversx_chain_core::{std::Bech32Address, types::Address};
use multiversx_sdk::wallet::Wallet;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

/// Compute the shard ID for a given address using the real MultiversX algorithm.
/// This matches mx-chain-go/sharding/multiShardCoordinator.ComputeIdFromBytes.
/// The SDK's Wallet::get_shard() uses `last_byte % 3` which is WRONG.
pub fn compute_shard(address: &Address, num_shards: u32) -> u8 {
    if num_shards <= 1 {
        return 0;
    }
    let addr_bytes = address.as_bytes();
    let last_byte = addr_bytes[addr_bytes.len() - 1] as u32;

    let n = (num_shards as f64).log2().ceil() as u32;
    let mask_high = (1u32 << n) - 1;
    let mask_low = (1u32 << (n - 1)) - 1;

    let mut shard = last_byte & mask_high;
    if shard >= num_shards {
        shard = last_byte & mask_low;
    }

    shard as u8
}

/// Wallet entry containing wallet data and metadata.
#[derive(Clone)]
pub struct WalletEntry {
    pub signing_key: SigningKey,
    pub address: Address,
    pub bech32: Bech32Address,
    pub shard: u8,
    pub nonce: Arc<AtomicU64>,
}

impl WalletEntry {
    /// Create a new wallet entry from a wallet and its private key seed bytes.
    pub fn new(wallet: Wallet, seed: [u8; 32]) -> Self {
        let address = wallet.to_address();
        let bech32 = address.to_bech32("erd");
        let shard = compute_shard(&address, 3);
        let signing_key = SigningKey::from_bytes(&seed);
        Self {
            signing_key,
            address,
            bech32,
            shard,
            nonce: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Get the current nonce and increment it atomically.
    pub fn get_nonce_then_increment(&self) -> u64 {
        self.nonce.fetch_add(1, Ordering::SeqCst)
    }

    /// Get the public key as a hex string.
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.address.as_bytes())
    }
}

/// WalletManager handles loading and organizing wallets by shard.
pub struct WalletManager {
    pub wallets_dir: PathBuf,
    pub shard_wallets: [Vec<Arc<WalletEntry>>; 3],
}

impl WalletManager {
    /// Create a new wallet manager for the given directory.
    pub fn new(wallets_dir: &str) -> Self {
        Self {
            wallets_dir: PathBuf::from(wallets_dir),
            shard_wallets: [Vec::new(), Vec::new(), Vec::new()],
        }
    }

    /// Load all wallets from the wallets directory.
    pub fn load_wallets(&mut self) -> Result<()> {
        if !self.wallets_dir.exists() {
            bail!("Directory {:?} does not exist.", self.wallets_dir);
        }

        let mut pem_files: Vec<PathBuf> = std::fs::read_dir(&self.wallets_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |ext| ext == "pem"))
            .collect();
        pem_files.sort();

        if pem_files.is_empty() {
            bail!("No PEM files found in {:?}", self.wallets_dir);
        }

        println!(
            "Loading {} wallet(s) from {:?}...",
            pem_files.len(),
            self.wallets_dir
        );

        for pem_file in &pem_files {
            let file_name = pem_file.file_name().unwrap().to_string_lossy().into_owned();
            let load_result: Result<WalletEntry> = (|| {
                let path_str = pem_file.to_str()
                    .ok_or_else(|| anyhow!("non-UTF8 path"))?;
                let wallet = Wallet::from_pem_file(path_str)
                    .with_context(|| format!("failed to parse PEM"))?;
                let (priv_hex, _pub_hex) = Wallet::get_wallet_keys_pem(path_str);
                let seed_bytes = hex::decode(&priv_hex)
                    .with_context(|| format!("invalid private key hex in PEM"))?;
                if seed_bytes.len() < 32 {
                    return Err(anyhow!("private key too short ({} bytes)", seed_bytes.len()));
                }
                let mut seed = [0u8; 32];
                seed.copy_from_slice(&seed_bytes[..32]);
                Ok(WalletEntry::new(wallet, seed))
            })();
            match load_result {
                Ok(entry) => {
                    let shard = entry.shard as usize;
                    if shard < 3 {
                        self.shard_wallets[shard].push(Arc::new(entry));
                    }
                }
                Err(e) => {
                    println!("  ✗ Failed to load {}: {}", file_name, e);
                }
            }
        }

        println!("\nWallets loaded by shard:");
        for s in 0..3 {
            println!("  Shard {}: {} wallet(s)", s, self.shard_wallets[s].len());
        }
        let total: usize = self.shard_wallets.iter().map(|v| v.len()).sum();
        println!("  Total: {} wallet(s)\n", total);
        Ok(())
    }

    /// Get all wallets across all shards.
    pub fn get_all_wallets(&self) -> Vec<Arc<WalletEntry>> {
        self.shard_wallets.iter().flat_map(|v| v.iter().cloned()).collect()
    }

    /// Get wallets for a specific shard.
    pub fn get_wallets_by_shard(&self, shard: u8) -> &[Arc<WalletEntry>] {
        if (shard as usize) < 3 {
            &self.shard_wallets[shard as usize]
        } else {
            &[]
        }
    }
}

/// Create new wallets in the specified directory.
pub fn create_wallets(
    wallets_dir: &str,
    number_of_wallets: usize,
    balanced: bool,
) -> Result<()> {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    println!("Creating wallets ...");
    let path = PathBuf::from(wallets_dir);
    std::fs::create_dir_all(&path)?;

    if balanced {
        let base_count = number_of_wallets / 3;
        let remainder = number_of_wallets % 3;
        let quotas: [usize; 3] = [
            base_count + if remainder > 0 { 1 } else { 0 },
            base_count,
            base_count + if remainder > 1 { 1 } else { 0 },
        ];
        println!(
            "Balanced mode: Target quotas -> Shard 0: {}, Shard 1: {}, Shard 2: {}",
            quotas[0], quotas[1], quotas[2]
        );

        let mut created_per_shard = [0usize; 3];
        let mut total_created = 0usize;

        while total_created < number_of_wallets {
            let signing_key = SigningKey::generate(&mut OsRng);
            let verifying_key = signing_key.verifying_key();
            let public_key_bytes = verifying_key.as_bytes();
            let private_key_hex = hex::encode(signing_key.as_bytes());
            let public_key_hex = hex::encode(public_key_bytes);

            let address = Address::new(*public_key_bytes);
            let shard = compute_shard(&address, 3) as usize;

            if created_per_shard[shard] < quotas[shard] {
                let bech32 = address.to_bech32("erd");
                let pem_content = Wallet::generate_pem_content(
                    "erd",
                    &address,
                    &private_key_hex,
                    &public_key_hex,
                );
                let pem_path = path.join(format!("{}.pem", bech32.to_bech32_string()));
                std::fs::write(&pem_path, &pem_content)?;

                created_per_shard[shard] += 1;
                total_created += 1;

                if total_created % 50 == 0 || total_created == number_of_wallets {
                    println!(
                        "  Created {}/{} (S0: {}, S1: {}, S2: {})",
                        total_created,
                        number_of_wallets,
                        created_per_shard[0],
                        created_per_shard[1],
                        created_per_shard[2]
                    );
                }
            }
        }
    } else {
        for _i in 0..number_of_wallets {
            let signing_key = SigningKey::generate(&mut OsRng);
            let verifying_key = signing_key.verifying_key();
            let public_key_bytes = verifying_key.as_bytes();
            let private_key_hex = hex::encode(signing_key.as_bytes());
            let public_key_hex = hex::encode(public_key_bytes);

            let address = Address::new(*public_key_bytes);
            let bech32 = address.to_bech32("erd");
            let pem_content = Wallet::generate_pem_content(
                "erd",
                &address,
                &private_key_hex,
                &public_key_hex,
            );
            let pem_path = path.join(format!("{}.pem", bech32.to_bech32_string()));
            std::fs::write(&pem_path, &pem_content)?;
        }
    }

    println!(
        "Created {} wallet(s) in {:?}",
        number_of_wallets,
        path.canonicalize().unwrap_or(path)
    );
    Ok(())
}

/// Find a relayer account by address from a list of wallets.
pub fn find_relayer_account(
    relayer_address: &str,
    shard_wallets: &[Arc<WalletEntry>],
    shard: u8,
) -> Result<Arc<WalletEntry>> {
    let relayer_bech32 = Bech32Address::from_bech32_string(relayer_address.to_string());
    let relayer_addr = relayer_bech32.to_address();
    let relayer_shard = compute_shard(&relayer_addr, 3);

    if relayer_shard != shard {
        bail!(
            "Relayer address is in shard {}, but transactions are in shard {}",
            relayer_shard,
            shard
        );
    }

    let relayer_pubkey = hex::encode(relayer_addr.as_bytes());
    for w in shard_wallets {
        if w.public_key_hex() == relayer_pubkey {
            return Ok(w.clone());
        }
    }
    bail!(
        "Relayer address {} not found in loaded wallets for shard {}.",
        relayer_address,
        shard
    );
}
