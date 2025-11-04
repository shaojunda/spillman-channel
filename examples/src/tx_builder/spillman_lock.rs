use anyhow::{anyhow, Result};
use ckb_crypto::secp::Pubkey;
use ckb_sdk::{Since, SinceType};
use ckb_types::{bytes::Bytes, core::ScriptHashType, packed, prelude::*, H256};
use std::str::FromStr;

use crate::utils::config::Config;
use crate::utils::crypto::{pubkey_hash, SpillmanLockArgs};

/// Build Spillman Lock script
pub fn build_spillman_lock_script(
    config: &Config,
    user_pubkey: &Pubkey,
    merchant_pubkey: &Pubkey,
    timeout_timestamp: u64,
) -> Result<packed::Script> {
    let user_pubkey_hash = pubkey_hash(user_pubkey);
    let merchant_pubkey_hash = pubkey_hash(merchant_pubkey);

    // Encode timeout_timestamp as absolute timestamp-based Since value
    // SinceType::Timestamp uses median time to avoid miner manipulation
    let timeout_since = Since::new(SinceType::Timestamp, timeout_timestamp, false);

    let args = SpillmanLockArgs::new(merchant_pubkey_hash, user_pubkey_hash, timeout_since.value());
    let args_bytes = args.to_bytes();

    let code_hash_str = config.spillman_lock.code_hash.trim_start_matches("0x");
    let code_hash = H256::from_str(code_hash_str)
        .map_err(|e| anyhow!("Invalid code hash '{}': {}", config.spillman_lock.code_hash, e))?;
    let hash_type = match config.spillman_lock.hash_type.as_str() {
        "data" => ScriptHashType::Data,
        "type" => ScriptHashType::Type,
        "data1" => ScriptHashType::Data1,
        "data2" => ScriptHashType::Data2,
        _ => return Err(anyhow!("Invalid hash type")),
    };

    let hash_type_byte: packed::Byte = hash_type.into();
    Ok(packed::Script::new_builder()
        .code_hash(code_hash.pack())
        .hash_type(hash_type_byte)
        .args(Bytes::from(args_bytes).pack())
        .build())
}

/// Build Spillman Lock script with pre-computed merchant pubkey hash
/// This is useful for multisig scenarios where merchant_pubkey_hash is blake160(multisig_config)
pub fn build_spillman_lock_script_with_hash(
    config: &Config,
    user_pubkey: &Pubkey,
    merchant_pubkey_hash: &[u8],
    timeout_timestamp: u64,
) -> Result<packed::Script> {
    let user_pubkey_hash = pubkey_hash(user_pubkey);

    // Encode timeout_timestamp as absolute timestamp-based Since value
    // SinceType::Timestamp uses median time to avoid miner manipulation
    let timeout_since = Since::new(SinceType::Timestamp, timeout_timestamp, false);

    // Use the provided merchant_pubkey_hash directly (could be from single-sig or multisig)
    // Convert &[u8] to [u8; 20]
    let mut merchant_hash_array = [0u8; 20];
    merchant_hash_array.copy_from_slice(&merchant_pubkey_hash[0..20]);
    let args = SpillmanLockArgs::new(merchant_hash_array, user_pubkey_hash, timeout_since.value());
    let args_bytes = args.to_bytes();

    let code_hash_str = config.spillman_lock.code_hash.trim_start_matches("0x");
    let code_hash = H256::from_str(code_hash_str)
        .map_err(|e| anyhow!("Invalid code hash '{}': {}", config.spillman_lock.code_hash, e))?;
    let hash_type = match config.spillman_lock.hash_type.as_str() {
        "data" => ScriptHashType::Data,
        "type" => ScriptHashType::Type,
        "data1" => ScriptHashType::Data1,
        "data2" => ScriptHashType::Data2,
        _ => return Err(anyhow!("Invalid hash type")),
    };

    let hash_type_byte: packed::Byte = hash_type.into();
    Ok(packed::Script::new_builder()
        .code_hash(code_hash.pack())
        .hash_type(hash_type_byte)
        .args(Bytes::from(args_bytes).pack())
        .build())
}
