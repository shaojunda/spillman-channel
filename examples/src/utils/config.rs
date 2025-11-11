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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usdi: Option<XudtConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NetworkConfig {
    pub rpc_url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct KeyConfig {
    // 单签字段（保留，使用 Option 让它可选以兼容旧配置）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,

    // 多签字段（新增，都是可选的）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multisig_threshold: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub multisig_total: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_keys: Option<Vec<String>>,

    // address 保持必填
    pub address: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ChannelConfig {
    pub capacity_ckb: u64,
    #[serde(default)]
    pub timeout_epochs: u64, // Deprecated, keeping for backwards compatibility
    pub timeout_timestamp: u64,
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct XudtConfig {
    pub code_hash: String,
    pub hash_type: String,
    pub args: String,
    pub tx_hash: String,
    pub index: u32,
    pub decimal: u8,
}

impl KeyConfig {
    /// 判断是否为多签配置
    pub fn is_multisig(&self) -> bool {
        self.private_keys.is_some() && self.multisig_threshold.is_some()
    }

    /// 获取所有密钥（兼容单签和多签）
    pub fn get_secret_keys(&self) -> Result<Vec<secp256k1::SecretKey>> {
        if let Some(keys) = &self.private_keys {
            // 多签模式
            keys.iter()
                .map(|k| Self::parse_secret_key(k))
                .collect()
        } else if let Some(key) = &self.private_key {
            // 单签模式（向后兼容）
            Ok(vec![Self::parse_secret_key(key)?])
        } else {
            Err(anyhow!("Neither private_key nor private_keys is configured"))
        }
    }

    /// 获取多签配置（threshold, total）
    pub fn get_multisig_config(&self) -> Option<(u8, u8)> {
        if let (Some(threshold), Some(total)) = (self.multisig_threshold, self.multisig_total) {
            Some((threshold, total))
        } else {
            None
        }
    }

    /// 验证配置的合法性
    pub fn validate(&self, name: &str) -> Result<()> {
        // 检查是否至少有一种配置
        if self.private_key.is_none() && self.private_keys.is_none() {
            return Err(anyhow!(
                "{}: must specify either private_key or private_keys",
                name
            ));
        }

        // 检查不能同时配置两种
        if self.private_key.is_some() && self.private_keys.is_some() {
            return Err(anyhow!(
                "{}: cannot specify both private_key and private_keys",
                name
            ));
        }

        // 验证多签配置
        if let Some(keys) = &self.private_keys {
            let threshold = self.multisig_threshold.ok_or_else(|| {
                anyhow!("{}: multisig_threshold is required for multisig", name)
            })?;
            let total = self.multisig_total.ok_or_else(|| {
                anyhow!("{}: multisig_total is required for multisig", name)
            })?;

            if threshold == 0 || threshold > total {
                return Err(anyhow!(
                    "{}: invalid multisig config: threshold={}, total={}",
                    name,
                    threshold,
                    total
                ));
            }

            if keys.len() != total as usize {
                return Err(anyhow!(
                    "{}: private_keys length ({}) must equal multisig_total ({})",
                    name,
                    keys.len(),
                    total
                ));
            }
        }

        Ok(())
    }

    /// 解析私钥字符串
    fn parse_secret_key(key_str: &str) -> Result<secp256k1::SecretKey> {
        let key_hex = key_str.trim_start_matches("0x");
        let key_bytes = hex::decode(key_hex)?;
        Ok(secp256k1::SecretKey::from_slice(&key_bytes)?)
    }
}

impl Config {
    /// 验证整个配置的合法性
    pub fn validate(&self) -> Result<()> {
        self.user.validate("user")?;
        self.merchant.validate("merchant")?;
        Ok(())
    }
}

/// Load configuration from specified path
pub fn load_config(config_path: &str) -> Result<Config> {
    let config_str = fs::read_to_string(config_path)
        .map_err(|_| anyhow!("Failed to read config file: {}", config_path))?;
    let config: Config = toml::from_str(&config_str)?;

    // 验证配置
    config.validate()?;

    Ok(config)
}
