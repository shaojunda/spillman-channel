use anyhow::{anyhow, Result};
use ckb_hash::blake2b_256;
use ckb_sdk::transaction::builder::FeeCalculator;
use ckb_types::{
    bytes::Bytes,
    core::{TransactionBuilder, TransactionView},
    packed::{CellInput, CellOutput, OutPoint, Script},
    prelude::*,
    H256,
};

use crate::utils::config::Config;

// Constants for witness structure
const EMPTY_WITNESS_ARGS: [u8; 16] = [16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0];
const UNLOCK_TYPE_TIMEOUT: u8 = 0x01;

/// Build refund transaction
///
/// This transaction allows the user (and merchant in co-fund mode) to reclaim funds after timeout
///
/// Structure:
/// - Input: Spillman Lock cell (from funding tx output 0)
/// - Output 0: User's address (full amount or user's portion)
/// - Output 1 (co-fund only): Merchant's address (merchant's portion)
/// - Since: timeout_epoch (time lock)
/// - Witness: Empty placeholder (to be filled with signatures)
///
/// Signing order:
/// 1. Merchant pre-signs during setup (guarantees user can refund)
/// 2. User adds signature after timeout and broadcasts
pub fn build_refund_transaction(
    config: &Config,
    funding_tx_hash: H256,
    funding_tx: &TransactionView,
    user_lock_script: Script,
    merchant_lock_script: Option<Script>,
    timeout_epoch: u64,
    output_path: &str,
) -> Result<TransactionView> {
    println!("  📋 构建 Refund 交易...");

    // Get Spillman Lock cell from funding tx output 0
    let spillman_cell = funding_tx
        .outputs()
        .get(0)
        .ok_or_else(|| anyhow!("Funding transaction has no output 0"))?;

    let spillman_capacity: u64 = Unpack::<u64>::unpack(&spillman_cell.capacity());

    println!("    - Spillman Lock cell capacity: {} CKB", spillman_capacity as f64 / 100_000_000.0);
    println!("    - Funding tx hash: {:#x}", funding_tx_hash);

    // Build input from Spillman Lock cell
    let input = CellInput::new_builder()
        .previous_output(
            OutPoint::new_builder()
                .tx_hash(funding_tx_hash.pack())
                .index(0u32.pack())
                .build(),
        )
        .since(timeout_epoch.pack()) // Time lock
        .build();

    println!("    - Input since (timeout): {}", timeout_epoch);

    // Setup fee calculator
    let fee_rate = 1000u64; // shannon per KB
    let fee_calculator = FeeCalculator::new(fee_rate);

    // Get Spillman Lock cell dep from config
    let spillman_tx_hash = hex::decode(config.spillman_lock.tx_hash.trim_start_matches("0x"))
        .map_err(|e| anyhow!("Failed to decode spillman lock tx_hash: {}", e))?;

    let spillman_out_point = OutPoint::new_builder()
        .tx_hash(ckb_types::packed::Byte32::from_slice(&spillman_tx_hash)?)
        .index(config.spillman_lock.index.pack())
        .build();

    let spillman_dep = ckb_types::packed::CellDep::new_builder()
        .out_point(spillman_out_point)
        .dep_type(ckb_types::core::DepType::Code.into())
        .build();

    // Get Auth cell dep from config
    let auth_tx_hash = hex::decode(config.auth.tx_hash.trim_start_matches("0x"))
        .map_err(|e| anyhow!("Failed to decode auth tx_hash: {}", e))?;

    let auth_out_point = OutPoint::new_builder()
        .tx_hash(ckb_types::packed::Byte32::from_slice(&auth_tx_hash)?)
        .index(config.auth.index.pack())
        .build();

    let auth_dep = ckb_types::packed::CellDep::new_builder()
        .out_point(auth_out_point)
        .dep_type(ckb_types::core::DepType::Code.into())
        .build();

    // Calculate merchant's minimum occupied capacity (for co-fund mode)
    let merchant_capacity = if let Some(ref merchant_lock) = merchant_lock_script {
        let merchant_cell = CellOutput::new_builder()
            .capacity(0u64.pack())
            .lock(merchant_lock.clone())
            .build();

        merchant_cell
            .occupied_capacity(ckb_types::core::Capacity::bytes(0).unwrap())
            .unwrap()
            .as_u64()
    } else {
        0
    };

    // Helper function to build transaction with given user capacity
    let build_tx = |user_cap: u64| {
        let mut builder = TransactionBuilder::default();

        // Add input
        builder = builder.input(input.clone());

        // Add cell deps
        builder = builder
            .cell_dep(spillman_dep.clone())
            .cell_dep(auth_dep.clone());

        // Add outputs based on mode
        if merchant_lock_script.is_some() {
            // Co-fund mode: 2 outputs
            // Output 0: User
            builder = builder
                .output(
                    CellOutput::new_builder()
                        .capacity(user_cap.pack())
                        .lock(user_lock_script.clone())
                        .build(),
                )
                .output_data(Bytes::new().pack());

            // Output 1: Merchant
            builder = builder
                .output(
                    CellOutput::new_builder()
                        .capacity(merchant_capacity.pack())
                        .lock(merchant_lock_script.as_ref().unwrap().clone())
                        .build(),
                )
                .output_data(Bytes::new().pack());
        } else {
            // Single fund mode: 1 output
            // Output 0: User (full refund)
            builder = builder
                .output(
                    CellOutput::new_builder()
                        .capacity(user_cap.pack())
                        .lock(user_lock_script.clone())
                        .build(),
                )
                .output_data(Bytes::new().pack());
        }

        // Add witness placeholder
        builder = builder.witness(Bytes::new().pack());

        builder.build()
    };

    // Helper function to calculate fee from a transaction
    let calculate_tx_fee = |tx: &TransactionView| -> u64 {
        let tx_size = tx.data().as_reader().serialized_size_in_block() as u64;
        fee_calculator.fee(tx_size)
    };

    // Step 1: Build temporary transaction to estimate fee
    // Use a temporary user capacity (will be recalculated)
    let temp_user_capacity = if merchant_lock_script.is_some() {
        spillman_capacity - merchant_capacity - 1000
    } else {
        spillman_capacity - 1000
    };

    let temp_tx = build_tx(temp_user_capacity);
    let estimated_fee = calculate_tx_fee(&temp_tx);

    println!("  - 估算手续费: {} shannon ({} CKB)", estimated_fee, estimated_fee as f64 / 100_000_000.0);

    // Step 2: Calculate actual user capacity
    let user_capacity = if merchant_lock_script.is_some() {
        // Co-fund mode
        println!("    - Mode: Co-fund (2 outputs)");

        let user_cap = spillman_capacity
            .checked_sub(merchant_capacity)
            .and_then(|c| c.checked_sub(estimated_fee))
            .ok_or_else(|| anyhow!("Not enough capacity for refund outputs"))?;

        println!("      - User refund: {} CKB", user_cap as f64 / 100_000_000.0);
        println!("      - Merchant refund: {} CKB", merchant_capacity as f64 / 100_000_000.0);

        user_cap
    } else {
        // Single fund mode
        println!("    - Mode: Single fund (1 output)");

        let user_cap = spillman_capacity
            .checked_sub(estimated_fee)
            .ok_or_else(|| anyhow!("Not enough capacity for refund"))?;

        println!("      - User refund: {} CKB", user_cap as f64 / 100_000_000.0);

        user_cap
    };

    // Step 3: Build final transaction with accurate capacity
    let tx = build_tx(user_capacity);

    println!("  ✓ Refund 交易构建完成（未签名）");
    println!("    - Inputs: {}", tx.inputs().len());
    println!("    - Outputs: {}", tx.outputs().len());
    println!("    - Estimated fee: {} shannon", estimated_fee);

    // Sign transaction (both merchant and user)
    println!("\n  🔐 签名交易...");

    // Parse private keys
    let user_privkey_hex = &config.user.private_key;
    let user_privkey_hex_trimmed = user_privkey_hex.trim_start_matches("0x");
    let user_privkey_bytes = hex::decode(user_privkey_hex_trimmed)
        .map_err(|e| anyhow!("Failed to decode user private key hex: {}", e))?;
    let user_key = secp256k1::SecretKey::from_slice(&user_privkey_bytes)
        .map_err(|e| anyhow!("Invalid user private key: {}", e))?;

    let merchant_privkey_hex = &config.merchant.private_key;
    let merchant_privkey_hex_trimmed = merchant_privkey_hex.trim_start_matches("0x");
    let merchant_privkey_bytes = hex::decode(merchant_privkey_hex_trimmed)
        .map_err(|e| anyhow!("Failed to decode merchant private key hex: {}", e))?;
    let merchant_key = secp256k1::SecretKey::from_slice(&merchant_privkey_bytes)
        .map_err(|e| anyhow!("Invalid merchant private key: {}", e))?;

    // Compute signing message (raw tx without cell_deps)
    let signing_message = compute_signing_message(&tx);

    println!("    - Signing message: {}", hex::encode(&signing_message));

    // Sign with secp256k1 (merchant first, then user)
    let secp = secp256k1::Secp256k1::new();

    let merchant_message = secp256k1::Message::from_digest_slice(&signing_message)
        .map_err(|e| anyhow!("Failed to create merchant message: {}", e))?;
    let merchant_signature = secp.sign_ecdsa_recoverable(&merchant_message, &merchant_key);
    let (merchant_recid, merchant_sig_bytes) = merchant_signature.serialize_compact();
    let mut merchant_sig = [0u8; 65];
    merchant_sig[0..64].copy_from_slice(&merchant_sig_bytes);
    merchant_sig[64] = merchant_recid as i32 as u8;

    println!("    ✓ Merchant 签名完成");

    let user_message = secp256k1::Message::from_digest_slice(&signing_message)
        .map_err(|e| anyhow!("Failed to create user message: {}", e))?;
    let user_signature = secp.sign_ecdsa_recoverable(&user_message, &user_key);
    let (user_recid, user_sig_bytes) = user_signature.serialize_compact();
    let mut user_sig = [0u8; 65];
    user_sig[0..64].copy_from_slice(&user_sig_bytes);
    user_sig[64] = user_recid as i32 as u8;

    println!("    ✓ User 签名完成");

    // Build witness: EMPTY_WITNESS_ARGS + UNLOCK_TYPE_TIMEOUT + merchant_sig + user_sig
    let witness_data = [
        &EMPTY_WITNESS_ARGS[..],
        &[UNLOCK_TYPE_TIMEOUT][..],
        &merchant_sig[..],
        &user_sig[..],
    ]
    .concat();

    println!("    ✓ Witness 构建完成 ({} bytes)", witness_data.len());

    // Rebuild transaction with witness (replace all witnesses)
    let signed_tx = tx
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from(witness_data).pack()])
        .build();

    println!("\n  ✓ Refund 交易签名完成");

    // Save to file
    let tx_json = ckb_jsonrpc_types::TransactionView::from(signed_tx.clone());
    let json_str = serde_json::to_string_pretty(&tx_json)?;

    if let Some(parent) = std::path::Path::new(output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, json_str)?;

    println!("  ✓ Refund 交易已保存: {}", output_path);

    Ok(signed_tx)
}

/// Compute signing message for Spillman Lock
/// This is the same as in test cases: blake2b_256(raw_tx without cell_deps)
fn compute_signing_message(tx: &TransactionView) -> [u8; 32] {
    let raw_tx = tx
        .data()
        .raw()
        .as_builder()
        .cell_deps(Default::default())
        .build();
    blake2b_256(raw_tx.as_slice())
}
