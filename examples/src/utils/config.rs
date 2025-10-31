use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub network: NetworkConfig,
    pub user: KeyConfig,
    pub merchant: KeyConfig,
    pub channel: ChannelConfig,
    pub spillman_lock: SpillmanLockConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NetworkConfig {
    pub rpc_url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct KeyConfig {
    pub private_key: String,
    pub address: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ChannelConfig {
    pub capacity_ckb: u64,
    pub timeout_epochs: u64,
    pub tx_fee_shannon: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SpillmanLockConfig {
    pub code_hash: String,
    pub hash_type: String,
    pub tx_hash: String,
    pub index: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AuthConfig {
    pub tx_hash: String,
    pub index: u32,
}

/// Load configuration from specified path
pub fn load_config(config_path: &str) -> Result<Config> {
    let config_str = fs::read_to_string(config_path)
        .map_err(|_| anyhow!("Failed to read config file: {}", config_path))?;
    let config: Config = toml::from_str(&config_str)?;
    Ok(config)
}
