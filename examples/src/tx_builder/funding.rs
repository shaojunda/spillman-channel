use anyhow::Result;
use ckb_types::packed::Script;

use crate::utils::config::Config;

/// Build funding transaction (placeholder)
/// TODO: Implement using SDK's CapacityTransferBuilder
///
/// Reference: https://github.com/nervosnetwork/ckb-sdk-rust/blob/master/README.md#build-transaction-by-manual
///
/// This will use:
/// - CapacityTransferBuilder to build the transaction
/// - DefaultCellCollector to collect user's cells
/// - CapacityBalancer to balance capacity
/// - SecpSighashUnlocker to unlock cells
pub fn build_funding_transaction_template(
    _config: &Config,
    _user_lock_script: Script,
    _spillman_lock_script: Script,
    channel_capacity_shannon: u64,
) -> Result<String> {
    println!("  📋 Building funding transaction...");
    println!("    ✓ Channel capacity: {} CKB", channel_capacity_shannon / 100_000_000);
    println!("    ⚠️  TODO: Implement using SDK's CapacityTransferBuilder");

    Ok("funding_tx_placeholder".to_string())
}

