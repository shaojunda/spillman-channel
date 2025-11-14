use anyhow::{anyhow, Result};
use ckb_crypto::secp::{Privkey, Pubkey};
use ckb_sdk::util::blake160;

/// Calculate pubkey hash using Blake160 (CKB standard)
pub fn pubkey_hash(pubkey: &Pubkey) -> [u8; 20] {
    blake160(&pubkey.serialize()).into()
}

/// Parse private key from hex string
pub fn parse_privkey(hex: &str) -> Result<Privkey> {
    let hex = hex.trim_start_matches("0x");
    let bytes = hex::decode(hex)?;
    if bytes.len() != 32 {
        return Err(anyhow!(
            "Invalid private key length: expected 32 bytes, got {}",
            bytes.len()
        ));
    }
    Ok(Privkey::from_slice(&bytes))
}

/// Spillman Lock Args structure (50 bytes)
/// Layout: merchant_lock_arg(20) + user_pubkey_hash(20) + timeout_timestamp(8) + algorithm_id(1) + version(1)
#[derive(Debug, Clone)]
pub struct SpillmanLockArgs {
    pub merchant_pubkey_hash: [u8; 20],
    pub user_pubkey_hash: [u8; 20],
    pub timeout_timestamp: u64,
    pub algorithm_id: u8, // 0 for single-sig, 6 for multi-sig
    pub version: u8,
}

impl SpillmanLockArgs {
    pub fn new_with_algorithm(
        merchant_pubkey_hash: [u8; 20],
        user_pubkey_hash: [u8; 20],
        timeout_timestamp: u64,
        algorithm_id: u8,
    ) -> Self {
        Self {
            merchant_pubkey_hash,
            user_pubkey_hash,
            timeout_timestamp,
            algorithm_id,
            version: 0,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(50);
        bytes.extend_from_slice(&self.merchant_pubkey_hash);
        bytes.extend_from_slice(&self.user_pubkey_hash);
        bytes.extend_from_slice(&self.timeout_timestamp.to_le_bytes());
        bytes.push(self.algorithm_id);
        bytes.push(self.version);
        bytes
    }
}
