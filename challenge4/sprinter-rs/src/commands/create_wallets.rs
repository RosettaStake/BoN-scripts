use crate::commands::Command;
use crate::wallet::{create_wallets as create_wallets_impl, create_wallets_with_quotas};
use anyhow::{bail, Result};
use async_trait::async_trait;

/// Create new wallets command.
pub struct CreateWalletsCommand {
    pub wallets_dir: String,
    pub number_of_wallets: usize,
    pub balanced: bool,
    /// Explicit per-shard counts [S0, S1, S2]. Overrides number_of_wallets + balanced.
    pub shards: Option<String>,
}

#[async_trait]
impl Command for CreateWalletsCommand {
    async fn execute(&self) -> Result<()> {
        if let Some(spec) = &self.shards {
            let parts: Vec<usize> = spec
                .split(',')
                .map(|s| s.trim().parse::<usize>())
                .collect::<Result<_, _>>()
                .map_err(|_| anyhow::anyhow!("--shards must be 3 comma-separated numbers, e.g. 20,60,20"))?;
            if parts.len() != 3 {
                bail!("--shards must have exactly 3 values (S0,S1,S2), got {}", parts.len());
            }
            create_wallets_with_quotas(&self.wallets_dir, [parts[0], parts[1], parts[2]])
        } else {
            create_wallets_impl(&self.wallets_dir, self.number_of_wallets, self.balanced)
        }
    }
}
