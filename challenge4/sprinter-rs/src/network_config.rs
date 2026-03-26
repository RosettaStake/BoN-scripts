//! Network topology configuration loaded from a TOML file.
//!
//! Example config:
//!
//! ```toml
//! proxy = "https://gateway.multiversx.com"
//!
//! [observers]
//! shard0 = "https://observer-shard0.url"
//! shard1 = "https://observer-shard1.url"
//! shard2 = "https://observer-shard2.url"
//! ```
//!
//! The `[observers]` section is optional. Any shard without an entry falls back
//! to the proxy URL for broadcasting.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ObserversConfig {
    pub shard0: Option<String>,
    pub shard1: Option<String>,
    pub shard2: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkConfig {
    pub proxy: String,
    #[serde(default)]
    pub observers: ObserversConfig,
}

impl NetworkConfig {
    pub fn load(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read network config: {}", path))?;
        toml::from_str(&content)
            .with_context(|| format!("Failed to parse network config: {}", path))
    }

    /// Returns the observer URL for a shard, falling back to the proxy if none is configured.
    pub fn shard_url(&self, shard: u8) -> String {
        let url = match shard {
            0 => self.observers.shard0.as_ref(),
            1 => self.observers.shard1.as_ref(),
            2 => self.observers.shard2.as_ref(),
            _ => None,
        };
        url.unwrap_or(&self.proxy).clone()
    }
}
