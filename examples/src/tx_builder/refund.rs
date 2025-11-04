use anyhow::{anyhow, Result};
use ckb_crypto::secp::Privkey;
use ckb_hash::blake2b_256;
use ckb_sdk::transaction::builder::FeeCalculator;
use ckb_types::{
    bytes::Bytes,
    core::{TransactionBuilder, TransactionView},
    packed::{CellInput, CellOutput, OutPoint, Script},
    prelude::*,
    H256,
};
use std::str::FromStr;

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
/// - Since: timeout_timestamp (time lock)
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
    _timeout_timestamp: u64,  // Not used - we read from Spillman Lock args
    output_path: &str,
) -> Result<TransactionView> {
    println!("  📋 构建 Refund 交易...");

    // Get Spillman Lock cell from funding tx output 0
    let spillman_cell = funding_tx
        .outputs()
        .get(0)
        .ok_or_else(|| anyhow!("Funding transaction has no output 0"))?;

    let spillman_capacity: u64 = Unpack::<u64>::unpack(&spillman_cell.capacity());

    // Parse timeout_since from Spillman Lock args
    // Args structure (50 bytes): merchant_lock_arg(20) + user_pubkey_hash(20) + timeout_since(8) + algorithm_id(1) + version(1)
    // Note: timeout_since is already a Since-encoded value (absolute epoch-based)
    let lock_script = spillman_cell.lock();
    let args_bytes: Bytes = lock_script.args().unpack();
    if args_bytes.len() != 50 {
        return Err(anyhow!("Invalid Spillman Lock args length: expected 50, got {}", args_bytes.len()));
    }

    // Extract timeout_since from args (bytes 40-48)
    // This is already a Since-encoded value, use it directly for input.since
    let timeout_since = u64::from_le_bytes(
        args_bytes[40..48]
            .try_into()
            .map_err(|_| anyhow!("Failed to parse timeout_since from args"))?
    );

    println!("    - Spillman Lock cell capacity: {} CKB", spillman_capacity as f64 / 100_000_000.0);
    println!("    - Funding tx hash: {:#x}", funding_tx_hash);
    println!("    - Timeout since (from Spillman Lock args): 0x{:x}", timeout_since);

    // Build input from Spillman Lock cell
    // Use the timeout_since value directly (already Since-encoded)
    let input = CellInput::new_builder()
        .previous_output(
            OutPoint::new_builder()
                .tx_hash(funding_tx_hash.pack())
                .index(0u32)
                .build(),
        )
        .since(timeout_since) // Already encoded as absolute epoch-based time lock
        .build();

    println!("    - Input since: 0x{:x}", timeout_since);

    // Setup fee calculator
    let fee_rate = 1000u64; // shannon per KB
    let fee_calculator = FeeCalculator::new(fee_rate);

    // Get Spillman Lock cell dep from config
    let spillman_tx_hash = hex::decode(config.spillman_lock.tx_hash.trim_start_matches("0x"))
        .map_err(|e| anyhow!("Failed to decode spillman lock tx_hash: {}", e))?;

    let spillman_out_point = OutPoint::new_builder()
        .tx_hash(ckb_types::packed::Byte32::from_slice(&spillman_tx_hash)?)
        .index(config.spillman_lock.index)
        .build();

    let spillman_dep = ckb_types::packed::CellDep::new_builder()
        .out_point(spillman_out_point)
        .dep_type(ckb_types::core::DepType::Code)
        .build();

    // Get Auth cell dep from config
    let auth_tx_hash = hex::decode(config.auth.tx_hash.trim_start_matches("0x"))
        .map_err(|e| anyhow!("Failed to decode auth tx_hash: {}", e))?;

    let auth_out_point = OutPoint::new_builder()
        .tx_hash(ckb_types::packed::Byte32::from_slice(&auth_tx_hash)?)
        .index(config.auth.index)
        .build();

    let auth_dep = ckb_types::packed::CellDep::new_builder()
        .out_point(auth_out_point)
        .dep_type(ckb_types::core::DepType::Code)
        .build();

    // Calculate merchant's minimum occupied capacity (for co-fund mode)
    let merchant_capacity = if let Some(ref merchant_lock) = merchant_lock_script {
        let merchant_cell = CellOutput::new_builder()
            .capacity(0u64)
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
                        .capacity(user_cap)
                        .lock(user_lock_script.clone())
                        .build(),
                )
                .output_data(Bytes::new().pack());

            // Output 1: Merchant
            builder = builder
                .output(
                    CellOutput::new_builder()
                        .capacity(merchant_capacity)
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
                        .capacity(user_cap)
                        .lock(user_lock_script.clone())
                        .build(),
                )
                .output_data(Bytes::new().pack());
        }

        // Add witness placeholder with correct size
        // Spillman Lock timeout witness: EMPTY_WITNESS_ARGS(16) + UNLOCK_TYPE(1) + merchant_sig(65) + user_sig(65) = 147 bytes
        let dummy_witness = vec![0u8; 147];
        builder = builder.witness(Bytes::from(dummy_witness).pack());

        builder.build()
    };

    // Helper function to calculate fee from a transaction
    let calculate_tx_fee = |tx: &TransactionView| -> u64 {
        let tx_size = tx.data().as_reader().serialized_size_in_block() as u64;
        fee_calculator.fee(tx_size)
    };

    // Iteratively calculate fee until stable
    let max_iterations = 10;
    let mut current_fee = 0u64;
    let mut final_user_capacity = 0u64;
    let mut final_tx: Option<TransactionView> = None;

    for iteration in 0..max_iterations {
        // Calculate user capacity based on current fee
        let user_cap = if merchant_lock_script.is_some() {
            // Co-fund mode
            spillman_capacity
                .checked_sub(merchant_capacity)
                .and_then(|c| c.checked_sub(current_fee))
                .ok_or_else(|| anyhow!("Not enough capacity for refund outputs and fee"))?
        } else {
            // Single fund mode
            spillman_capacity
                .checked_sub(current_fee)
                .ok_or_else(|| anyhow!("Not enough capacity for refund and fee"))?
        };

        // Build transaction with calculated capacity
        let temp_tx = build_tx(user_cap);

        // Calculate actual fee for this transaction
        let actual_fee = calculate_tx_fee(&temp_tx);

        if iteration == 0 {
            println!("  - 初始手续费估算: {} shannon ({} CKB)", actual_fee, actual_fee as f64 / 100_000_000.0);
        }

        // Check if fee has stabilized
        if actual_fee == current_fee {
            println!("  - 手续费已稳定: {} shannon ({} CKB) (迭代 {} 次)", actual_fee, actual_fee as f64 / 100_000_000.0, iteration + 1);
            final_user_capacity = user_cap;
            final_tx = Some(temp_tx);
            break;
        }

        // Update for next iteration
        current_fee = actual_fee;
        final_user_capacity = user_cap;

        if iteration == max_iterations - 1 {
            println!("  - ⚠️  达到最大迭代次数，使用最后计算的手续费: {} shannon", current_fee);
            final_tx = Some(temp_tx);
        }
    }

    let tx = final_tx.ok_or_else(|| anyhow!("Failed to build transaction"))?;

    // Print refund mode and amounts
    if merchant_lock_script.is_some() {
        println!("    - Mode: Co-fund (2 outputs)");
        println!("      - User refund: {} CKB", final_user_capacity as f64 / 100_000_000.0);
        println!("      - Merchant refund: {} CKB", merchant_capacity as f64 / 100_000_000.0);
    } else {
        println!("    - Mode: Single fund (1 output)");
        println!("      - User refund: {} CKB", final_user_capacity as f64 / 100_000_000.0);
    }

    println!("\n  ✓ Refund 交易构建完成（未签名）");
    println!("    - Inputs: {}", tx.inputs().len());
    println!("    - Outputs: {}", tx.outputs().len());
    println!("    - Final fee: {} shannon", current_fee);

    // Sign transaction (both merchant and user)
    println!("\n  🔐 签名交易...");

    // Parse private keys using ckb-crypto
    use crate::utils::crypto::pubkey_hash;

    let user_privkey_hex = &config.user.private_key;
    let user_privkey = Privkey::from_str(user_privkey_hex)
        .map_err(|e| anyhow!("Failed to parse user private key: {:?}", e))?;
    let user_pubkey = user_privkey.pubkey().map_err(|e| anyhow!("Failed to get user pubkey: {:?}", e))?;
    let user_pubkey_hash_from_privkey = pubkey_hash(&user_pubkey);

    let merchant_privkey_hex = &config.merchant.private_key;
    let merchant_privkey = Privkey::from_str(merchant_privkey_hex)
        .map_err(|e| anyhow!("Failed to parse merchant private key: {:?}", e))?;
    let merchant_pubkey = merchant_privkey.pubkey().map_err(|e| anyhow!("Failed to get merchant pubkey: {:?}", e))?;
    let merchant_pubkey_hash_from_privkey = pubkey_hash(&merchant_pubkey);

    // Verify pubkey hashes match Spillman Lock args
    let expected_merchant_hash = &args_bytes[0..20];
    let expected_user_hash = &args_bytes[20..40];

    if merchant_pubkey_hash_from_privkey != expected_merchant_hash {
        return Err(anyhow!("Merchant pubkey hash mismatch! The private key in config.toml doesn't match the Spillman Lock args."));
    }
    if user_pubkey_hash_from_privkey != expected_user_hash {
        return Err(anyhow!("User pubkey hash mismatch! The private key in config.toml doesn't match the Spillman Lock args."));
    }

    // Compute signing message (raw tx without cell_deps)
    let signing_message = compute_signing_message(&tx);

    // Sign with ckb-crypto (merchant first, then user)
    let merchant_sig = merchant_privkey
        .sign_recoverable(&signing_message.into())
        .map_err(|e| anyhow!("Failed to sign with merchant key: {:?}", e))?
        .serialize();

    println!("    ✓ Merchant 签名完成");

    let user_sig = user_privkey
        .sign_recoverable(&signing_message.into())
        .map_err(|e| anyhow!("Failed to sign with user key: {:?}", e))?
        .serialize();

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
    use ckb_types::packed::CellDepVec;
    let raw_tx = tx
        .data()
        .raw()
        .as_builder()
        .cell_deps(CellDepVec::default())
        .build();

    blake2b_256(raw_tx.as_slice())
}
