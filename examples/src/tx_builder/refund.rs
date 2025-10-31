use anyhow::Result;
use ckb_types::packed::{OutPoint, Script};

use crate::utils::config::Config;

/// Build refund transaction (placeholder)
/// TODO: Implement proper refund transaction construction
///
/// This transaction allows the user to reclaim funds after timeout
///
/// Structure:
/// - Input: Spillman Lock cell (will reference funding tx)
/// - Output: User's address (full refund minus fees)
/// - Witness: Spillman Witness with Timeout unlock type (0x01)
///
/// Signing order:
/// 1. Merchant pre-signs during setup (guarantees user can refund)
/// 2. User adds signature after timeout and broadcasts
pub fn build_refund_transaction(
    _config: &Config,
    _spillman_lock_outpoint: OutPoint,
    spillman_lock_capacity: u64,
    _user_lock_script: Script,
) -> Result<String> {
    println!("  📋 Building refund transaction...");
    println!("    ✓ Refund capacity: {} CKB", spillman_lock_capacity / 100_000_000);
    println!("    ⚠️  TODO: Implement refund transaction construction");

    Ok("refund_tx_placeholder".to_string())
}
