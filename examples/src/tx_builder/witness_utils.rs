/// Witness size calculation utilities shared across tx_builder modules
///
/// This module provides common functions for calculating witness sizes
/// for different signature types (single-sig vs multisig) used in
/// Spillman Channel transactions.
use ckb_sdk::unlock::MultisigConfig;

/// Size of a single ECDSA signature (r + s + v)
pub const SIGNATURE_SIZE: usize = 65;

/// Size of empty witness args placeholder
pub const EMPTY_WITNESS_ARGS_SIZE: usize = 16;

/// Size of unlock type byte
pub const UNLOCK_TYPE_SIZE: usize = 1;

/// Calculate the size of merchant signature in witness
///
/// Returns:
/// - Single-sig: SIGNATURE_SIZE bytes (one signature)
/// - Multisig: config_data.len() + threshold * SIGNATURE_SIZE bytes
///
/// # Arguments
/// * `merchant_multisig_config` - Optional multisig configuration for merchant
///
/// # Examples
/// ```ignore
/// // Single-sig merchant
/// let size = calculate_merchant_signature_size(None);
/// assert_eq!(size, SIGNATURE_SIZE);
///
/// // Multisig merchant (2-of-3)
/// let config = MultisigConfig::new(...);
/// let size = calculate_merchant_signature_size(Some(&config));
/// // size = config_data.len() + 2 * SIGNATURE_SIZE
/// ```
pub fn calculate_merchant_signature_size(
    merchant_multisig_config: Option<&MultisigConfig>,
) -> usize {
    if let Some(config) = merchant_multisig_config {
        // Multisig: config_data + threshold * SIGNATURE_SIZE bytes signatures
        let config_data = config.to_witness_data();
        config_data.len() + (config.threshold() as usize) * SIGNATURE_SIZE
    } else {
        // Single-sig: SIGNATURE_SIZE bytes signature
        SIGNATURE_SIZE
    }
}

/// Calculate the total size of refund witness
///
/// Refund witness structure:
/// - EMPTY_WITNESS_ARGS: 16 bytes
/// - UNLOCK_TYPE_TIMEOUT: 1 byte
/// - Merchant signature: variable (calculated by calculate_merchant_signature_size)
/// - User signature: SIGNATURE_SIZE bytes
///
/// # Arguments
/// * `merchant_multisig_config` - Optional multisig configuration for merchant
///
/// # Returns
/// Total witness size in bytes
pub fn calculate_refund_witness_size(merchant_multisig_config: Option<&MultisigConfig>) -> usize {
    let base_size = EMPTY_WITNESS_ARGS_SIZE + UNLOCK_TYPE_SIZE;
    let user_sig_size = SIGNATURE_SIZE;
    let merchant_sig_size = calculate_merchant_signature_size(merchant_multisig_config);

    base_size + merchant_sig_size + user_sig_size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_sig_merchant_size() {
        let size = calculate_merchant_signature_size(None);
        assert_eq!(size, SIGNATURE_SIZE);
    }

    #[test]
    fn test_refund_witness_size_single_sig() {
        // Single-sig: EMPTY_WITNESS_ARGS_SIZE + UNLOCK_TYPE_SIZE + SIGNATURE_SIZE + SIGNATURE_SIZE = 147 bytes
        let size = calculate_refund_witness_size(None);
        assert_eq!(
            size,
            EMPTY_WITNESS_ARGS_SIZE + UNLOCK_TYPE_SIZE + SIGNATURE_SIZE + SIGNATURE_SIZE
        );
    }
}
