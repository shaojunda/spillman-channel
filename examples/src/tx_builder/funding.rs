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

