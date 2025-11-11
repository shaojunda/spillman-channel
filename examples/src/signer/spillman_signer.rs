use anyhow::{anyhow, Result};
use ckb_crypto::secp::{Message, Privkey, Signature};
use ckb_hash::new_blake2b;
use ckb_types::{packed, prelude::*};

use crate::utils::crypto::{SpillmanWitness, UnlockType};

/// Spillman transaction signer
pub struct SpillmanSigner;

impl SpillmanSigner {
    /// Calculate SIGHASH_ALL message for a transaction
    /// This follows the CKB transaction signing standard
    pub fn calculate_sighash_all(
        tx: &packed::Transaction,
        witness_index: usize,
    ) -> Result<[u8; 32]> {
        // Serialize transaction for signing
        let mut hasher = new_blake2b();

        // Hash tx hash
        let tx_hash = tx.calc_tx_hash();
        hasher.update(tx_hash.as_slice());

        // Hash witness at the signing index
        let witnesses = tx.witnesses();
        if witness_index >= witnesses.len() {
            return Err(anyhow!(
                "Witness index {} out of bounds (total: {})",
                witness_index,
                witnesses.len()
            ));
        }

        let witness = witnesses
            .get(witness_index)
            .ok_or_else(|| anyhow!("Failed to get witness at index {}", witness_index))?;

        hasher.update(witness.as_slice());

        let mut message = [0u8; 32];
        hasher.finalize(&mut message);
        Ok(message)
    }

    /// Sign a message with private key
    pub fn sign_message(privkey: &Privkey, message: &[u8; 32]) -> Result<[u8; 65]> {
        let msg = Message::from_slice(message)?;
        let signature = privkey.sign_recoverable(&msg)?;

        let mut sig_bytes = [0u8; 65];
        sig_bytes.copy_from_slice(&signature.serialize());

        Ok(sig_bytes)
    }

    /// Verify a signature
    pub fn verify_signature(
        signature: &[u8; 65],
        message: &[u8; 32],
        expected_pubkey: &ckb_crypto::secp::Pubkey,
    ) -> Result<bool> {
        let msg = Message::from_slice(message)?;
        let sig = Signature::from_slice(signature)?;

        let recovered_pubkey = sig.recover(&msg)?;
        Ok(recovered_pubkey.serialize() == expected_pubkey.serialize())
    }

    /// Sign a transaction as user (first signature for commitment, second for timeout)
    pub fn sign_as_user(
        tx: &packed::Transaction,
        privkey: &Privkey,
        witness_index: usize,
        unlock_type: UnlockType,
    ) -> Result<SpillmanWitness> {
        // Calculate signing message
        let message = Self::calculate_sighash_all(tx, witness_index)?;

        // Sign the message
        let signature = Self::sign_message(privkey, &message)?;

        // Create witness with user signature
        let mut witness = SpillmanWitness::new(unlock_type);
        witness.set_user_signature(signature);

        Ok(witness)
    }

    /// Add merchant signature to an existing witness
    pub fn sign_as_merchant(
        tx: &packed::Transaction,
        privkey: &Privkey,
        witness_index: usize,
        mut witness: SpillmanWitness,
    ) -> Result<SpillmanWitness> {
        // Verify this is a commitment transaction (merchant doesn't sign timeout)
        if witness.unlock_type != UnlockType::Commitment {
            return Err(anyhow!(
                "Merchant can only sign commitment transactions, got: {:?}",
                witness.unlock_type
            ));
        }

        // Calculate signing message
        let message = Self::calculate_sighash_all(tx, witness_index)?;

        // Sign the message
        let signature = Self::sign_message(privkey, &message)?;

        // Add merchant signature
        witness.set_merchant_signature(signature);

        Ok(witness)
    }

    /// Sign commitment transaction (user first)
    /// Returns partially signed witness with only user signature
    pub fn sign_commitment_as_user(
        tx: &packed::Transaction,
        user_privkey: &Privkey,
        witness_index: usize,
    ) -> Result<SpillmanWitness> {
        Self::sign_as_user(tx, user_privkey, witness_index, UnlockType::Commitment)
    }

    /// Complete commitment transaction signing (merchant second)
    /// Takes partially signed witness and adds merchant signature
    pub fn sign_commitment_as_merchant(
        tx: &packed::Transaction,
        merchant_privkey: &Privkey,
        witness_index: usize,
        witness: SpillmanWitness,
    ) -> Result<SpillmanWitness> {
        Self::sign_as_merchant(tx, merchant_privkey, witness_index, witness)
    }

    /// Sign refund transaction (timeout path)
    /// User signs after timeout, merchant signature already included
    pub fn sign_refund_as_user(
        tx: &packed::Transaction,
        user_privkey: &Privkey,
        witness_index: usize,
    ) -> Result<SpillmanWitness> {
        Self::sign_as_user(tx, user_privkey, witness_index, UnlockType::Timeout)
    }

    /// Merchant pre-signs refund transaction during setup
    /// This ensures user can always get their funds back after timeout
    pub fn presign_refund_as_merchant(
        tx: &packed::Transaction,
        merchant_privkey: &Privkey,
        witness_index: usize,
    ) -> Result<SpillmanWitness> {
        // Calculate signing message
        let message = Self::calculate_sighash_all(tx, witness_index)?;

        // Sign the message
        let signature = Self::sign_message(merchant_privkey, &message)?;

        // Create witness with merchant signature for timeout
        let mut witness = SpillmanWitness::new(UnlockType::Timeout);
        witness.set_merchant_signature(signature);

        Ok(witness)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_crypto::secp::Generator;

    #[test]
    fn test_sign_and_verify() {
        let gen = Generator::random_keypair();
        let privkey = gen.0;
        let pubkey = gen.1;

        let message = [0u8; 32];
        let signature = SpillmanSigner::sign_message(&privkey, &message).unwrap();

        let verified = SpillmanSigner::verify_signature(&signature, &message, &pubkey).unwrap();
        assert!(verified);
    }

    #[test]
    fn test_witness_signatures() {
        let gen = Generator::random_keypair();
        let privkey = gen.0;

        let mut witness = SpillmanWitness::new(UnlockType::Commitment);

        let message = [0u8; 32];
        let signature = SpillmanSigner::sign_message(&privkey, &message).unwrap();

        witness.set_user_signature(signature);
        assert_eq!(witness.user_signature, signature);
    }
}
