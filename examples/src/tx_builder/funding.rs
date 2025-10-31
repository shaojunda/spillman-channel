use anyhow::{anyhow, Result};
use ckb_sdk::{
    constants::SIGHASH_TYPE_HASH,
    rpc::CkbRpcClient,
    traits::{
        DefaultCellCollector, DefaultCellDepResolver, DefaultHeaderDepResolver,
        DefaultTransactionDependencyProvider, SecpCkbRawKeySigner,
    },
    tx_builder::{transfer::CapacityTransferBuilder, CapacityBalancer, TxBuilder},
    unlock::{ScriptUnlocker, SecpSighashUnlocker},
    Address, ScriptId,
};
use ckb_types::{
    bytes::Bytes,
    core::BlockView,
    packed::{CellOutput, Script, WitnessArgs},
    prelude::*,
    H256,
};
use std::collections::HashMap;
use std::fs;

use crate::utils::config::Config;

/// Build complete funding transaction with inputs and signatures
///
/// This function:
/// - Collects user's live cells as inputs
/// - Creates Spillman Lock cell as output
/// - Calculates and adds change output
/// - Signs transaction with user's private key
/// - Saves signed transaction to file
///
/// Returns: (tx_hash, output_index) where output_index is the Spillman Lock cell index
pub async fn build_funding_transaction(
    config: &Config,
    user_address: &Address,
    spillman_lock_script: &Script,
    capacity_ckb: u64,
    output_path: &str,
) -> Result<(H256, u32)> {
    let capacity_shannon = capacity_ckb * 100_000_000;

    println!("  - Spillman Lock cell capacity: {} CKB ({} shannon)", capacity_ckb, capacity_shannon);

    // Setup providers from RPC
    let ckb_client = CkbRpcClient::new(&config.network.rpc_url);

    let cell_dep_resolver = {
        let genesis_block = ckb_client.get_block_by_number(0.into())?.unwrap();
        DefaultCellDepResolver::from_genesis(&BlockView::from(genesis_block))?
    };
    let header_dep_resolver = DefaultHeaderDepResolver::new(&config.network.rpc_url);
    let mut cell_collector = DefaultCellCollector::new(&config.network.rpc_url);
    let tx_dep_provider = DefaultTransactionDependencyProvider::new(&config.network.rpc_url, 10);

    // Build sender lock script from user address
    let sender = Script::from(user_address);

    // Build ScriptUnlocker for signing
    // Convert Privkey to secp256k1::SecretKey
    // We need to re-parse the private key from the config
    let privkey_hex = &config.user.private_key;
    let privkey_hex_trimmed = privkey_hex.trim_start_matches("0x");
    let privkey_bytes = hex::decode(privkey_hex_trimmed)
        .map_err(|e| anyhow!("failed to decode private key hex: {}", e))?;
    let sender_key = secp256k1::SecretKey::from_slice(&privkey_bytes)
        .map_err(|e| anyhow!("invalid user private key: {}", e))?;
    let signer = SecpCkbRawKeySigner::new_with_secret_keys(vec![sender_key]);
    let sighash_unlocker = SecpSighashUnlocker::from(Box::new(signer) as Box<_>);
    let sighash_script_id = ScriptId::new_type(SIGHASH_TYPE_HASH.clone());
    let mut unlockers = HashMap::default();
    unlockers.insert(
        sighash_script_id,
        Box::new(sighash_unlocker) as Box<dyn ScriptUnlocker>,
    );

    // Build CapacityBalancer
    let placeholder_witness = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(vec![0u8; 65])).pack())
        .build();
    let balancer = CapacityBalancer::new_simple(sender, placeholder_witness, 1000);

    // Build Spillman Lock cell output
    let spillman_cell = CellOutput::new_builder()
        .capacity((capacity_shannon as u64).pack())
        .lock(spillman_lock_script.clone())
        .build();

    // Build the transaction
    println!("  - 收集用户的 live cells 并构建交易...");
    let builder = CapacityTransferBuilder::new(vec![(spillman_cell, Bytes::default())]);
    let (tx, still_locked_groups) = builder.build_unlocked(
        &mut cell_collector,
        &cell_dep_resolver,
        &header_dep_resolver,
        &tx_dep_provider,
        &balancer,
        &unlockers,
    )?;

    if !still_locked_groups.is_empty() {
        return Err(anyhow!("Some script groups are still locked: {:?}", still_locked_groups));
    }

    let tx_hash = tx.hash();
    println!("✓ 交易已构建并签名");
    println!("  - Transaction hash: {:#x}", tx_hash);
    println!("  - Inputs 数量: {}", tx.inputs().len());
    println!("  - Outputs 数量: {}", tx.outputs().len());

    // Save signed transaction
    let tx_json = ckb_jsonrpc_types::TransactionView::from(tx);
    let json_str = serde_json::to_string_pretty(&tx_json)?;

    if let Some(parent) = std::path::Path::new(output_path).parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, json_str)?;

    println!("✓ 已签名的 Funding transaction 已保存: {}", output_path);

    // Return tx_hash and output_index (Spillman Lock cell is always at index 0)
    Ok((tx_hash.unpack(), 0))
}

/// Build co-fund funding transaction where both user and merchant contribute
///
/// This function:
/// - User contributes the specified capacity
/// - Merchant contributes minimum cell occupied capacity
/// - Collects cells from both user and merchant
/// - Creates Spillman Lock cell as output with combined capacity
/// - Signs transaction with both private keys
/// - Saves signed transaction to file
///
/// Returns: (tx_hash, output_index) where output_index is the Spillman Lock cell index
pub async fn build_cofund_funding_transaction(
    config: &Config,
    user_address: &Address,
    merchant_address: &Address,
    user_capacity_ckb: u64,
    spillman_lock_script: &Script,
    output_path: &str,
) -> Result<(H256, u32)> {
    println!("  - Co-fund 模式：User + Merchant 共同出资");

    let user_capacity_shannon = user_capacity_ckb * 100_000_000;

    // Calculate merchant's minimum occupied capacity using SDK
    // Build a temp merchant cell with capacity=0 to calculate occupied capacity
    let merchant_lock = Script::from(merchant_address);
    let temp_merchant_cell = CellOutput::new_builder()
        .capacity(0u64.pack())
        .lock(merchant_lock)
        .build();

    let merchant_capacity_shannon = temp_merchant_cell
        .occupied_capacity(ckb_types::core::Capacity::bytes(0).unwrap())
        .unwrap()
        .as_u64();

    // User adds extra 1 CKB as buffer (for fees, etc.)
    let user_buffer_shannon = 1 * 100_000_000;
    let total_capacity_shannon = user_capacity_shannon + merchant_capacity_shannon + user_buffer_shannon;

    println!("  - User 出资: {} CKB + 1 CKB buffer", user_capacity_ckb);
    println!("  - Merchant 出资: {} CKB (最小占用)", merchant_capacity_shannon / 100_000_000);
    println!("  - Spillman Lock cell 总容量: {} CKB", total_capacity_shannon / 100_000_000);

    // Setup providers from RPC
    let ckb_client = CkbRpcClient::new(&config.network.rpc_url);

    let cell_dep_resolver = {
        let genesis_block = ckb_client.get_block_by_number(0.into())?.unwrap();
        DefaultCellDepResolver::from_genesis(&BlockView::from(genesis_block))?
    };
    let header_dep_resolver = DefaultHeaderDepResolver::new(&config.network.rpc_url);
    let mut cell_collector = DefaultCellCollector::new(&config.network.rpc_url);
    let tx_dep_provider = DefaultTransactionDependencyProvider::new(&config.network.rpc_url, 10);

    // Build unlockers for both user and merchant
    let mut unlockers = HashMap::default();

    // User unlocker
    let user_privkey_hex = &config.user.private_key;
    let user_privkey_hex_trimmed = user_privkey_hex.trim_start_matches("0x");
    let user_privkey_bytes = hex::decode(user_privkey_hex_trimmed)
        .map_err(|e| anyhow!("failed to decode user private key hex: {}", e))?;
    let user_key = secp256k1::SecretKey::from_slice(&user_privkey_bytes)
        .map_err(|e| anyhow!("invalid user private key: {}", e))?;

    // Merchant unlocker
    let merchant_privkey_hex = &config.merchant.private_key;
    let merchant_privkey_hex_trimmed = merchant_privkey_hex.trim_start_matches("0x");
    let merchant_privkey_bytes = hex::decode(merchant_privkey_hex_trimmed)
        .map_err(|e| anyhow!("failed to decode merchant private key hex: {}", e))?;
    let merchant_key = secp256k1::SecretKey::from_slice(&merchant_privkey_bytes)
        .map_err(|e| anyhow!("invalid merchant private key: {}", e))?;

    let signer = SecpCkbRawKeySigner::new_with_secret_keys(vec![user_key, merchant_key]);
    let sighash_unlocker = SecpSighashUnlocker::from(Box::new(signer) as Box<_>);
    let sighash_script_id = ScriptId::new_type(SIGHASH_TYPE_HASH.clone());
    unlockers.insert(
        sighash_script_id,
        Box::new(sighash_unlocker) as Box<dyn ScriptUnlocker>,
    );

    // Build CapacityBalancer with user's lock as change address
    let user_lock = Script::from(user_address);
    let placeholder_witness = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(vec![0u8; 65])).pack())
        .build();
    let balancer = CapacityBalancer::new_simple(user_lock, placeholder_witness, 1000);

    // Build Spillman Lock cell output with combined capacity
    let spillman_cell = CellOutput::new_builder()
        .capacity((total_capacity_shannon as u64).pack())
        .lock(spillman_lock_script.clone())
        .build();

    // Build the transaction with Spillman Lock cell only
    println!("  - 收集 User 和 Merchant 的 live cells 并构建交易...");
    let builder = CapacityTransferBuilder::new(vec![
        (spillman_cell, Bytes::default()),
    ]);

    let (tx, still_locked_groups) = builder.build_unlocked(
        &mut cell_collector,
        &cell_dep_resolver,
        &header_dep_resolver,
        &tx_dep_provider,
        &balancer,
        &unlockers,
    )?;

    if !still_locked_groups.is_empty() {
        return Err(anyhow!("Some script groups are still locked: {:?}", still_locked_groups));
    }

    let tx_hash = tx.hash();
    println!("✓ Co-fund 交易已构建并签名");
    println!("  - Transaction hash: {:#x}", tx_hash);
    println!("  - Inputs 数量: {}", tx.inputs().len());
    println!("  - Outputs 数量: {}", tx.outputs().len());

    // Save signed transaction
    let tx_json = ckb_jsonrpc_types::TransactionView::from(tx);
    let json_str = serde_json::to_string_pretty(&tx_json)?;

    if let Some(parent) = std::path::Path::new(output_path).parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, json_str)?;

    println!("✓ 已签名的 Co-fund Funding transaction 已保存: {}", output_path);

    // Return tx_hash and output_index (Spillman Lock cell is always at index 0)
    Ok((tx_hash.unpack(), 0))
}

