use anyhow::Result;
use ckb_types::packed::{OutPoint, Script};

use crate::utils::config::Config;

/// Build commitment transaction (placeholder)
/// TODO: Implement proper commitment transaction construction
///
/// This transaction represents an off-chain payment from user to merchant
///
/// Structure:
/// - Input: Spillman Lock cell
/// - Output 0: User's address (change)
/// - Output 1: Merchant's address (payment)
/// - Witness: Spillman Witness with Commitment unlock type (0x00)
///
/// Signing order:
/// 1. User signs first (creates commitment)
/// 2. Merchant signs second to broadcast (settles payment)
///
/// Important: Each new commitment must pay MORE to merchant than previous one
pub fn build_commitment_transaction(
    _config: &Config,
    _spillman_lock_outpoint: OutPoint,
    spillman_lock_capacity: u64,
    _user_lock_script: Script,
    _merchant_lock_script: Script,
    payment_amount: u64,
) -> Result<String> {
    println!("  📋 Building commitment transaction...");
    println!("      Payment: {} CKB to merchant", payment_amount / 100_000_000);
    println!("      User change: {} CKB", (spillman_lock_capacity - payment_amount) / 100_000_000);
    println!("    ⚠️  TODO: Implement commitment transaction construction");

    Ok("commitment_tx_placeholder".to_string())
}
