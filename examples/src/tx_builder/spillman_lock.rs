use anyhow::{anyhow, Result};
use ckb_crypto::secp::Pubkey;
use ckb_types::{bytes::Bytes, core::ScriptHashType, packed, prelude::*, H256};
use std::str::FromStr;

use crate::utils::config::Config;
use crate::utils::crypto::{pubkey_hash, SpillmanLockArgs};

/// Build Spillman Lock script
pub fn build_spillman_lock_script(
    config: &Config,
    user_pubkey: &Pubkey,
    merchant_pubkey: &Pubkey,
    timeout_epoch: u64,
) -> Result<packed::Script> {
    let user_pubkey_hash = pubkey_hash(user_pubkey);
    let merchant_pubkey_hash = pubkey_hash(merchant_pubkey);

    let args = SpillmanLockArgs::new(merchant_pubkey_hash, user_pubkey_hash, timeout_epoch);
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
