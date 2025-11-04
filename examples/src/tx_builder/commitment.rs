/// Commitment transaction builder for Spillman Channel
///
/// # Commitment Transaction Structure
///
/// ## Inputs
/// - Spillman Lock cell (from funding transaction)
/// - Since: 0 (no time lock on commitment path)
///
/// ## Outputs
/// - Output 0: User's cell (change/refund)
/// - Output 1: Merchant's cell (payment amount)
///
/// ## Witness
/// - EMPTY_WITNESS_ARGS (16 bytes)
/// - UNLOCK_TYPE_COMMITMENT (1 byte, 0x00)
/// - Merchant signature (65 bytes, placeholder, filled by merchant during settle)
/// - User signature (65 bytes, signed by user)
///
/// Total: 147 bytes
///
/// # Signing Flow
///
/// 1. **User creates commitment**: User signs the transaction with their payment
/// 2. **Merchant settles**: Merchant adds their signature and broadcasts to chain

use anyhow::{anyhow, Result};
use ckb_crypto::secp::Privkey;
use ckb_hash::blake2b_256;
use ckb_sdk::constants::ONE_CKB;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, DepType, TransactionView},
    packed::{CellDep, CellDepVec, CellInput, CellOutput, OutPoint, Script, Transaction, Uint64},
    prelude::*,
    H256,
};
use std::str::FromStr;

use crate::utils::config::Config;

// Constants for witness structure
const EMPTY_WITNESS_ARGS: [u8; 16] = [16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0];
const UNLOCK_TYPE_COMMITMENT: u8 = 0x00;

/// Build commitment transaction (high-level API)
///
/// This function:
/// - Creates cell deps from config
/// - Parses user private key from config
/// - Builds the commitment transaction
/// - Signs with user's key
/// - Saves to file
/// - Returns (tx_hash, TransactionView)
///
/// # Arguments
/// * `config` - Configuration
/// * `funding_tx_hash` - The funding transaction hash
/// * `funding_output_index` - The index of Spillman Lock cell in funding tx
/// * `spillman_lock_capacity` - The capacity of the Spillman Lock cell
/// * `spillman_lock_script` - The Spillman Lock script
/// * `user_lock_script` - User's lock script (for change output)
/// * `merchant_lock_script` - Merchant's lock script (for payment output)
/// * `payment_amount` - Amount to pay to merchant (in shannons, excluding minimum occupied capacity)
/// * `merchant_min_capacity` - Merchant cell's minimum occupied capacity (in shannons)
/// * `output_path` - Path to save the transaction JSON
pub fn build_commitment_transaction(
    config: &Config,
    funding_tx_hash: H256,
    funding_output_index: u32,
    spillman_lock_capacity: u64,
    spillman_lock_script: Script,
    user_lock_script: Script,
    merchant_lock_script: Script,
    payment_amount: u64,
    merchant_min_capacity: u64,
    output_path: &str,
) -> Result<(H256, TransactionView)> {
    println!("📝 构建 Commitment 交易...");

    // Parse user private key from config
    let user_privkey = Privkey::from_str(&config.user.private_key)
        .map_err(|e| anyhow!("Failed to parse user private key: {:?}", e))?;

    // Build Spillman Lock outpoint
    let spillman_lock_outpoint = OutPoint::new_builder()
        .tx_hash(funding_tx_hash.pack())
        .index(funding_output_index)
        .build();

    // Build cell deps from config
    let spillman_tx_hash = hex::decode(config.spillman_lock.tx_hash.trim_start_matches("0x"))?;
    let spillman_out_point = OutPoint::new_builder()
        .tx_hash(ckb_types::packed::Byte32::from_slice(&spillman_tx_hash)?)
        .index(config.spillman_lock.index)
        .build();
    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_out_point)
        .dep_type(DepType::Code)
        .build();

    let auth_tx_hash = hex::decode(config.auth.tx_hash.trim_start_matches("0x"))?;
    let auth_out_point = OutPoint::new_builder()
        .tx_hash(ckb_types::packed::Byte32::from_slice(&auth_tx_hash)?)
        .index(config.auth.index)
        .build();
    let auth_dep = CellDep::new_builder()
        .out_point(auth_out_point)
        .dep_type(DepType::Code)
        .build();

    // Build transaction
    let tx = build_commitment_transaction_internal(
        spillman_lock_outpoint,
        spillman_lock_capacity,
        spillman_lock_script,
        user_lock_script,
        merchant_lock_script,
        payment_amount,
        merchant_min_capacity,
        spillman_lock_dep,
        auth_dep,
        &user_privkey,
    )?;

    let tx_hash = tx.hash();

    // Print summary
    let merchant_total_capacity = payment_amount + merchant_min_capacity;
    let fee_estimate = 1000u64;
    let change_amount = spillman_lock_capacity - merchant_total_capacity - fee_estimate;

    println!("✓ Commitment transaction built");
    println!("  - Transaction hash: {:#x}", tx_hash);
    println!("  - Payment to merchant: {} CKB (payment) + {} CKB (min capacity) = {} CKB",
        payment_amount / ONE_CKB,
        merchant_min_capacity / ONE_CKB,
        merchant_total_capacity / ONE_CKB);
    println!("  - Change to user: {} CKB", change_amount / ONE_CKB);
    println!("  - Estimated fee: 0.00001 CKB");

    // Save transaction
    let tx_json = ckb_jsonrpc_types::TransactionView::from(tx.clone());
    let json_str = serde_json::to_string_pretty(&tx_json)?;

    if let Some(parent) = std::path::Path::new(output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, json_str)?;

    println!("✓ Commitment transaction saved: {}", output_path);
    println!("  ✅ Transaction is signed by user and ready for merchant to settle");

    Ok((tx_hash.unpack(), tx))
}

/// Internal function to build and sign commitment transaction
fn build_commitment_transaction_internal(
    spillman_lock_outpoint: OutPoint,
    spillman_lock_capacity: u64,
    _spillman_lock_script: Script,
    user_lock_script: Script,
    merchant_lock_script: Script,
    payment_amount: u64,
    merchant_min_capacity: u64,
    spillman_lock_dep: CellDep,
    auth_dep: CellDep,
    user_privkey: &Privkey,
) -> Result<TransactionView> {
    // Calculate merchant's total capacity (payment + minimum occupied capacity)
    let merchant_total_capacity = payment_amount + merchant_min_capacity;

    // Calculate change amount (spillman_capacity - merchant_total - fee estimate)
    let fee_estimate = 1000u64; // 0.00001 CKB
    let change_amount = spillman_lock_capacity
        .checked_sub(merchant_total_capacity)
        .and_then(|v| v.checked_sub(fee_estimate))
        .ok_or_else(|| anyhow!(
            "Insufficient capacity: need {} (merchant) + {} (fee) CKB, have {} CKB",
            merchant_total_capacity / ONE_CKB,
            fee_estimate / ONE_CKB,
            spillman_lock_capacity / ONE_CKB
        ))?;

    // Build inputs: Spillman Lock cell
    let input = CellInput::new_builder()
        .previous_output(spillman_lock_outpoint)
        .since(Uint64::from(0u64)) // No time lock for commitment path
        .build();

    // Build outputs
    // Output 0: User's address (change)
    let user_output = CellOutput::new_builder()
        .lock(user_lock_script)
        .capacity(Capacity::shannons(change_amount).pack())
        .build();

    // Output 1: Merchant's address (payment + minimum occupied capacity)
    let merchant_output = CellOutput::new_builder()
        .lock(merchant_lock_script)
        .capacity(Capacity::shannons(merchant_total_capacity).pack())
        .build();

    // Build witness with placeholder signatures (will be replaced after signing)
    let mut witness_data = Vec::with_capacity(147);
    witness_data.extend_from_slice(&EMPTY_WITNESS_ARGS);
    witness_data.push(UNLOCK_TYPE_COMMITMENT);
    // Placeholder for merchant signature (65 bytes of zeros)
    witness_data.extend_from_slice(&[0u8; 65]);
    // Placeholder for user signature (65 bytes of zeros)
    witness_data.extend_from_slice(&[0u8; 65]);

    let witness = Bytes::from(witness_data);

    // Build cell_deps
    let cell_deps = CellDepVec::new_builder()
        .push(spillman_lock_dep)
        .push(auth_dep)
        .build();

    // Build transaction
    let tx = Transaction::default()
        .as_advanced_builder()
        .cell_deps(cell_deps)
        .input(input)
        .output(user_output)
        .output(merchant_output)
        .output_data(Bytes::new().pack())
        .output_data(Bytes::new().pack())
        .witness(witness.pack())
        .build();

    // Convert to TransactionView
    let tx_view: TransactionView = tx.into();

    // Sign the transaction with user's key
    let signed_tx = sign_commitment_transaction(tx_view, user_privkey)?;

    Ok(signed_tx)
}

/// Sign the commitment transaction with user's private key
fn sign_commitment_transaction(
    tx: TransactionView,
    user_privkey: &Privkey,
) -> Result<TransactionView> {
    // Prepare signing message
    let signing_message = compute_signing_message(&tx);

    // Sign with user's key (following refund_v2.rs pattern)
    let user_sig = user_privkey
        .sign_recoverable(&signing_message.into())
        .map_err(|e| anyhow!("Failed to sign with user key: {:?}", e))?
        .serialize();

    // Replace user signature in witness (keep merchant signature as placeholder)
    let witness = tx.witnesses().get(0)
        .ok_or_else(|| anyhow!("Missing witness"))?;

    let witness_data = witness.raw_data();
    if witness_data.len() != 147 {
        return Err(anyhow!("Invalid witness size: expected 147, got {}", witness_data.len()));
    }

    // Build new witness with user signature
    let mut new_witness = Vec::with_capacity(147);
    new_witness.extend_from_slice(&witness_data[..82]); // Copy EMPTY_WITNESS_ARGS + UNLOCK_TYPE + merchant_sig_placeholder
    new_witness.extend_from_slice(&user_sig); // Add user signature

    // Build new transaction with signed witness
    let new_tx = tx.as_advanced_builder()
        .set_witnesses(vec![Bytes::from(new_witness).pack()])
        .build();

    let new_tx_view: TransactionView = new_tx.into();

    Ok(new_tx_view)
}

/// Compute the signing message for a commitment transaction
/// This follows the same pattern as refund_v2.rs
fn compute_signing_message(tx: &TransactionView) -> [u8; 32] {
    // Clear cell_deps for signing (following CKB's signing convention)
    let raw_tx = tx
        .data()
        .raw()
        .as_builder()
        .cell_deps(CellDepVec::default())
        .build();

    blake2b_256(raw_tx.as_slice())
}
