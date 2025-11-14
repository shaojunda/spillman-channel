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
/// - Merchant signature (variable length, placeholder, filled by merchant during settle)
///   - Single-sig: 65 bytes
///   - Multisig: multisig_config + threshold * 65 bytes
/// - User signature (65 bytes, signed by user)
///
/// Total: Variable length based on merchant's signature type
///
/// # Signing Flow
///
/// 1. **User creates commitment**: User signs the transaction with their payment
/// 2. **Merchant settles**: Merchant adds their signature and broadcasts to chain
use anyhow::{anyhow, Result};
use ckb_crypto::secp::Privkey;
use ckb_hash::blake2b_256;
use ckb_sdk::{constants::ONE_CKB, unlock::MultisigConfig};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, DepType, TransactionView},
    packed::{CellDep, CellDepVec, CellInput, CellOutput, OutPoint, Script, Transaction, Uint64},
    prelude::*,
    H256,
};
use std::str::FromStr;

use crate::{tx_builder::funding_v2::build_multisig_config, utils::config::Config};

use crate::tx_builder::witness_utils::{EMPTY_WITNESS_ARGS_SIZE, SIGNATURE_SIZE, UNLOCK_TYPE_SIZE};

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
/// * `fee_rate` - Fee rate in shannons per KB (default: 1000)
/// * `output_path` - Path to save the transaction JSON
/// * `xudt_type_script` - Optional xUDT type script (for xUDT channels)
/// * `xudt_total_amount` - Optional total xUDT amount in Spillman Lock cell
/// * `xudt_payment_amount` - Optional xUDT amount to pay to merchant
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
    fee_rate: u64,
    output_path: &str,
    xudt_type_script: Option<Script>,
    xudt_total_amount: Option<u128>,
    xudt_payment_amount: Option<u128>,
) -> Result<(H256, TransactionView)> {
    println!("📝 构建 Commitment 交易...");

    // Parse user private key from config
    let user_privkey = Privkey::from_str(
        config
            .user
            .private_key
            .as_ref()
            .expect("User private_key is required"),
    )
    .map_err(|e| anyhow!("Failed to parse user private key: {:?}", e))?;

    // Check if merchant uses multisig and build config if needed
    let merchant_multisig_config = if config.merchant.is_multisig() {
        let threshold = config
            .merchant
            .multisig_threshold
            .ok_or_else(|| anyhow!("Merchant multisig_threshold is required"))?;
        let total = config
            .merchant
            .multisig_total
            .ok_or_else(|| anyhow!("Merchant multisig_total is required"))?;

        // Parse merchant public keys (not private keys, just for config structure)
        let privkeys = config
            .merchant
            .private_keys
            .as_ref()
            .ok_or_else(|| anyhow!("Merchant private_keys is required for multisig"))?;

        let parsed_keys: Result<Vec<secp256k1::SecretKey>> = privkeys
            .iter()
            .map(|key_str| {
                let key_bytes = hex::decode(key_str.trim_start_matches("0x"))
                    .map_err(|e| anyhow!("Failed to decode private key: {}", e))?;
                secp256k1::SecretKey::from_slice(&key_bytes)
                    .map_err(|e| anyhow!("Invalid private key: {}", e))
            })
            .collect();

        let keys = parsed_keys?;
        Some(build_multisig_config(&keys, threshold, total)?)
    } else {
        None
    };

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

    // Build xUDT cell dep if this is an xUDT channel
    let xudt_cell_dep = if xudt_type_script.is_some() {
        if let Some(ref usdi_config) = config.usdi {
            let xudt_tx_hash = hex::decode(usdi_config.tx_hash.trim_start_matches("0x"))?;
            let xudt_out_point = OutPoint::new_builder()
                .tx_hash(ckb_types::packed::Byte32::from_slice(&xudt_tx_hash)?)
                .index(usdi_config.index)
                .build();
            Some(
                CellDep::new_builder()
                    .out_point(xudt_out_point)
                    .dep_type(DepType::Code)
                    .build(),
            )
        } else {
            return Err(anyhow!("xUDT channel detected but usdi config not found"));
        }
    } else {
        None
    };

    // Build transaction with iterative fee calculation
    let (tx, actual_fee) = build_commitment_transaction_internal(
        spillman_lock_outpoint,
        spillman_lock_capacity,
        spillman_lock_script,
        user_lock_script,
        merchant_lock_script,
        payment_amount,
        merchant_min_capacity,
        spillman_lock_dep,
        auth_dep,
        xudt_cell_dep,
        &user_privkey,
        merchant_multisig_config.as_ref(),
        fee_rate,
        xudt_type_script,
        xudt_total_amount,
        xudt_payment_amount,
    )?;

    let tx_hash = tx.hash();

    // Print summary
    let merchant_total_capacity = payment_amount + merchant_min_capacity;
    let change_amount = spillman_lock_capacity - merchant_total_capacity - actual_fee;

    println!("✓ Commitment transaction built");
    println!("  - Transaction hash: {:#x}", tx_hash);
    println!(
        "  - Payment to merchant: {} CKB (payment) + {} CKB (min capacity) = {} CKB",
        payment_amount / ONE_CKB,
        merchant_min_capacity / ONE_CKB,
        merchant_total_capacity / ONE_CKB
    );
    println!("  - Change to user: {} CKB", change_amount / ONE_CKB);
    println!(
        "  - Transaction fee: {} CKB",
        actual_fee as f64 / ONE_CKB as f64
    );

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

/// Internal function to build and sign commitment transaction with iterative fee calculation
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
    xudt_cell_dep: Option<CellDep>,
    user_privkey: &Privkey,
    merchant_multisig_config: Option<&MultisigConfig>,
    fee_rate: u64,
    xudt_type_script: Option<Script>,
    xudt_total_amount: Option<u128>,
    xudt_payment_amount: Option<u128>,
) -> Result<(TransactionView, u64)> {
    // Calculate merchant's total capacity (payment + minimum occupied capacity)
    let merchant_total_capacity = payment_amount + merchant_min_capacity;

    // Iteratively calculate fee to stabilize transaction size
    let max_iterations = 10;
    let mut current_fee = 1000u64; // Initial estimate
    let mut final_tx = None;

    for iteration in 0..max_iterations {
        // Calculate change amount with current fee
        let change_amount = spillman_lock_capacity
            .checked_sub(merchant_total_capacity)
            .and_then(|v| v.checked_sub(current_fee))
            .ok_or_else(|| {
                anyhow!(
                    "Insufficient capacity: need {} (merchant) + {} (fee) CKB, have {} CKB",
                    merchant_total_capacity / ONE_CKB,
                    current_fee / ONE_CKB,
                    spillman_lock_capacity / ONE_CKB
                )
            })?;

        // Build inputs: Spillman Lock cell
        let input = CellInput::new_builder()
            .previous_output(spillman_lock_outpoint.clone())
            .since(Uint64::from(0u64)) // No time lock for commitment path
            .build();

        // Build outputs with xUDT support
        let (user_output, user_output_data, merchant_output, merchant_output_data) =
            if let Some(ref type_script) = xudt_type_script {
                // xUDT channel: add type script and xUDT amounts
                let xudt_total =
                    xudt_total_amount.ok_or_else(|| anyhow!("xUDT total amount required"))?;
                let xudt_payment =
                    xudt_payment_amount.ok_or_else(|| anyhow!("xUDT payment amount required"))?;
                let xudt_change = xudt_total
                    .checked_sub(xudt_payment)
                    .ok_or_else(|| anyhow!("xUDT payment exceeds total amount"))?;

                // Output 0: User's address (change with xUDT)
                let user_output = CellOutput::new_builder()
                    .lock(user_lock_script.clone())
                    .type_(Some(type_script.clone()).pack())
                    .capacity(Capacity::shannons(change_amount).pack())
                    .build();
                let user_data = Bytes::from(xudt_change.to_le_bytes().to_vec());

                // Output 1: Merchant's address (payment with xUDT)
                let merchant_output = CellOutput::new_builder()
                    .lock(merchant_lock_script.clone())
                    .type_(Some(type_script.clone()).pack())
                    .capacity(Capacity::shannons(merchant_total_capacity).pack())
                    .build();
                let merchant_data = Bytes::from(xudt_payment.to_le_bytes().to_vec());

                (user_output, user_data, merchant_output, merchant_data)
            } else {
                // Regular CKB channel
                let user_output = CellOutput::new_builder()
                    .lock(user_lock_script.clone())
                    .capacity(Capacity::shannons(change_amount).pack())
                    .build();

                let merchant_output = CellOutput::new_builder()
                    .lock(merchant_lock_script.clone())
                    .capacity(Capacity::shannons(merchant_total_capacity).pack())
                    .build();

                (user_output, Bytes::new(), merchant_output, Bytes::new())
            };

        // Calculate merchant placeholder size based on multisig config
        let merchant_placeholder_size =
            crate::tx_builder::witness_utils::calculate_merchant_signature_size(
                merchant_multisig_config,
            );

        // Calculate total witness size
        let witness_size =
            EMPTY_WITNESS_ARGS_SIZE + UNLOCK_TYPE_SIZE + merchant_placeholder_size + SIGNATURE_SIZE;

        // Build witness with placeholder signatures (will be replaced after signing)
        let mut witness_data = Vec::with_capacity(witness_size);
        witness_data.extend_from_slice(&EMPTY_WITNESS_ARGS);
        witness_data.push(UNLOCK_TYPE_COMMITMENT);
        // Placeholder for merchant signature (zeros)
        witness_data.extend_from_slice(&vec![0u8; merchant_placeholder_size]);
        // Placeholder for user signature (65 bytes of zeros)
        witness_data.extend_from_slice(&[0u8; SIGNATURE_SIZE]);

        let witness = Bytes::from(witness_data);

        // Build cell_deps with xUDT dep if needed
        let mut cell_deps_builder = CellDepVec::new_builder()
            .push(spillman_lock_dep.clone())
            .push(auth_dep.clone());

        if let Some(ref xudt_dep) = xudt_cell_dep {
            cell_deps_builder = cell_deps_builder.push(xudt_dep.clone());
        }

        let cell_deps = cell_deps_builder.build();

        // Build transaction
        let tx = Transaction::default()
            .as_advanced_builder()
            .cell_deps(cell_deps)
            .input(input)
            .output(user_output)
            .output(merchant_output)
            .output_data(user_output_data.pack())
            .output_data(merchant_output_data.pack())
            .witness(witness.pack())
            .build();

        // Convert to TransactionView
        let tx_view: TransactionView = tx;

        // Sign the transaction with user's key
        let signed_tx =
            sign_commitment_transaction(tx_view, user_privkey, merchant_placeholder_size)?;

        // Calculate actual fee for this transaction
        let tx_size = signed_tx.data().as_reader().serialized_size_in_block() as u64;
        let actual_fee = (tx_size * fee_rate).div_ceil(1000); // Round up

        println!("Iteration {}: Actual fee: {} CKB", iteration, actual_fee);

        // Check if fee has stabilized
        if actual_fee == current_fee {
            final_tx = Some((signed_tx, current_fee));
            break;
        }

        current_fee = actual_fee;

        if iteration == max_iterations - 1 {
            final_tx = Some((signed_tx, current_fee));
        }
    }

    let (tx, fee) = final_tx.ok_or_else(|| anyhow!("Failed to build commitment transaction"))?;
    Ok((tx, fee))
}

/// Sign the commitment transaction with user's private key
fn sign_commitment_transaction(
    tx: TransactionView,
    user_privkey: &Privkey,
    merchant_placeholder_size: usize,
) -> Result<TransactionView> {
    // Prepare signing message
    let signing_message = compute_signing_message(&tx);

    // Sign with user's key (following refund_v2.rs pattern)
    let user_sig = user_privkey
        .sign_recoverable(&signing_message.into())
        .map_err(|e| anyhow!("Failed to sign with user key: {:?}", e))?
        .serialize();

    // Replace user signature in witness (keep merchant signature as placeholder)
    let witness = tx
        .witnesses()
        .get(0)
        .ok_or_else(|| anyhow!("Missing witness"))?;

    let witness_data = witness.raw_data();
    let expected_size =
        EMPTY_WITNESS_ARGS_SIZE + UNLOCK_TYPE_SIZE + merchant_placeholder_size + SIGNATURE_SIZE;

    if witness_data.len() != expected_size {
        return Err(anyhow!(
            "Invalid witness size: expected {}, got {}",
            expected_size,
            witness_data.len()
        ));
    }

    // Build new witness with user signature
    let merchant_sig_end = EMPTY_WITNESS_ARGS_SIZE + UNLOCK_TYPE_SIZE + merchant_placeholder_size;
    let mut new_witness = Vec::with_capacity(expected_size);
    new_witness.extend_from_slice(&witness_data[..merchant_sig_end]); // Copy EMPTY_WITNESS_ARGS + UNLOCK_TYPE + merchant_sig_placeholder
    new_witness.extend_from_slice(&user_sig); // Add user signature

    // Build new transaction with signed witness
    let new_tx = tx
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from(new_witness).pack()])
        .build();

    let new_tx_view: TransactionView = new_tx;

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
