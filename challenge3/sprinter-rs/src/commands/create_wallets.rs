use crate::commands::Command;
use crate::wallet::create_wallets as create_wallets_impl;
use anyhow::Result;
use async_trait::async_trait;

/// Create new wallets command.
pub struct CreateWalletsCommand {
    pub wallets_dir: String,
    pub number_of_wallets: usize,
    pub balanced: bool,
}

#[async_trait]
impl Command for CreateWalletsCommand {
    async fn execute(&self) -> Result<()> {
        create_wallets_impl(&self.wallets_dir, self.number_of_wallets, self.balanced)
    }
}
