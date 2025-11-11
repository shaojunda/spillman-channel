use anyhow::{anyhow, Result};
use ckb_crypto::secp::Pubkey;
use ckb_sdk::{constants::MultisigScript, Address, Since, SinceType};
use ckb_types::{bytes::Bytes, core::ScriptHashType, packed, prelude::*, H256};
use std::str::FromStr;

use crate::utils::config::Config;
use crate::utils::crypto::{pubkey_hash, SpillmanLockArgs};

/// Detect multisig algorithm_id from merchant address
/// Returns:
/// - 0: single-sig
/// - 6: multisig Legacy (hash_type = Type)
/// - 7: multisig V2 (hash_type = Data1)
fn detect_multisig_algorithm_id(config: &Config) -> Result<u8> {
    if !config.merchant.is_multisig() {
        return Ok(0); // single-sig
    }

    // Parse merchant address to get lock script
    let merchant_address = Address::from_str(&config.merchant.address)
        .map_err(|e| anyhow!("Failed to parse merchant address: {}", e))?;

    let lock_script = packed::Script::from(&merchant_address);
    let code_hash: H256 = lock_script.code_hash().unpack();

    // Compare with known multisig script IDs
    let legacy_script_id = MultisigScript::Legacy.script_id();
    let v2_script_id = MultisigScript::V2.script_id();

    if code_hash == legacy_script_id.code_hash
        && lock_script.hash_type() == legacy_script_id.hash_type.into()
    {
        Ok(6) // Legacy multisig
    } else if code_hash == v2_script_id.code_hash
        && lock_script.hash_type() == v2_script_id.hash_type.into()
    {
        Ok(7) // V2 multisig
    } else {
        Err(anyhow!("Unknown multisig type for merchant address"))
    }
}

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

    let args = SpillmanLockArgs::new(
        merchant_pubkey_hash,
        user_pubkey_hash,
        timeout_since.value(),
    );
    let args_bytes = args.to_bytes();

    let code_hash_str = config.spillman_lock.code_hash.trim_start_matches("0x");
    let code_hash = H256::from_str(code_hash_str).map_err(|e| {
        anyhow!(
            "Invalid code hash '{}': {}",
            config.spillman_lock.code_hash,
            e
        )
    })?;
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
    // Detect algorithm_id from merchant address
    // - 0: single-sig
    // - 6: multisig Legacy (hash_type = Type)
    // - 7: multisig V2 (hash_type = Data1)
    let algorithm_id = detect_multisig_algorithm_id(config)?;

    build_spillman_lock_script_with_hash_and_algorithm(
        config,
        user_pubkey,
        merchant_pubkey_hash,
        timeout_timestamp,
        algorithm_id,
    )
}

/// Build Spillman Lock script with pre-computed merchant pubkey hash and explicit algorithm_id
/// This is useful for multisig scenarios where merchant_pubkey_hash is blake160(multisig_config)
pub fn build_spillman_lock_script_with_hash_and_algorithm(
    config: &Config,
    user_pubkey: &Pubkey,
    merchant_pubkey_hash: &[u8],
    timeout_timestamp: u64,
    algorithm_id: u8,
) -> Result<packed::Script> {
    let user_pubkey_hash = pubkey_hash(user_pubkey);

    // Encode timeout_timestamp as absolute timestamp-based Since value
    // SinceType::Timestamp uses median time to avoid miner manipulation
    let timeout_since = Since::new(SinceType::Timestamp, timeout_timestamp, false);

    // Use the provided merchant_pubkey_hash directly (could be from single-sig or multisig)
    // Convert &[u8] to [u8; 20]
    let mut merchant_hash_array = [0u8; 20];
    merchant_hash_array.copy_from_slice(&merchant_pubkey_hash[0..20]);
    let args = SpillmanLockArgs::new_with_algorithm(
        merchant_hash_array,
        user_pubkey_hash,
        timeout_since.value(),
        algorithm_id,
    );
    let args_bytes = args.to_bytes();

    let code_hash_str = config.spillman_lock.code_hash.trim_start_matches("0x");
    let code_hash = H256::from_str(code_hash_str).map_err(|e| {
        anyhow!(
            "Invalid code hash '{}': {}",
            config.spillman_lock.code_hash,
            e
        )
    })?;
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
