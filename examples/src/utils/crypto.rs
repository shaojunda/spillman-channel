use anyhow::{anyhow, Result};
use ckb_crypto::secp::{Privkey, Pubkey};
use ckb_sdk::util::blake160;

/// Calculate pubkey hash using Blake160 (CKB standard)
pub fn pubkey_hash(pubkey: &Pubkey) -> [u8; 20] {
    blake160(&pubkey.serialize()).into()
}

/// Parse private key from hex string
pub fn parse_privkey(hex: &str) -> Result<Privkey> {
    let hex = hex.trim_start_matches("0x");
    let bytes = hex::decode(hex)?;
    if bytes.len() != 32 {
        return Err(anyhow!(
            "Invalid private key length: expected 32 bytes, got {}",
            bytes.len()
        ));
    }
    Ok(Privkey::from_slice(&bytes))
}

/// Spillman Lock Args structure (50 bytes)
/// Layout: merchant_lock_arg(20) + user_pubkey_hash(20) + timeout_timestamp(8) + algorithm_id(1) + version(1)
#[derive(Debug, Clone)]
pub struct SpillmanLockArgs {
    pub merchant_pubkey_hash: [u8; 20],
    pub user_pubkey_hash: [u8; 20],
    pub timeout_timestamp: u64,
    pub algorithm_id: u8,  // 0 for single-sig, 6 for multi-sig
    pub version: u8,
}

impl SpillmanLockArgs {
    pub fn new(
        merchant_pubkey_hash: [u8; 20],
        user_pubkey_hash: [u8; 20],
        timeout_timestamp: u64,
    ) -> Self {
        Self {
            merchant_pubkey_hash,
            user_pubkey_hash,
            timeout_timestamp,
            algorithm_id: 0,  // Single-sig mode
            version: 0,
        }
    }

    pub fn new_with_algorithm(
        merchant_pubkey_hash: [u8; 20],
        user_pubkey_hash: [u8; 20],
        timeout_timestamp: u64,
        algorithm_id: u8,
    ) -> Self {
        Self {
            merchant_pubkey_hash,
            user_pubkey_hash,
            timeout_timestamp,
            algorithm_id,
            version: 0,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(50);
        bytes.extend_from_slice(&self.merchant_pubkey_hash);
        bytes.extend_from_slice(&self.user_pubkey_hash);
        bytes.extend_from_slice(&self.timeout_timestamp.to_le_bytes());
        bytes.push(self.algorithm_id);
        bytes.push(self.version);
        bytes
    }
}

/// Unlock type for Spillman Witness
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnlockType {
    /// Commitment path - both signatures required
    Commitment = 0x00,
    /// Timeout path - user signature only after timeout
    Timeout = 0x01,
}

impl UnlockType {
    pub fn to_byte(&self) -> u8 {
        *self as u8
    }

    pub fn from_byte(byte: u8) -> Result<Self> {
        match byte {
            0x00 => Ok(UnlockType::Commitment),
            0x01 => Ok(UnlockType::Timeout),
            _ => Err(anyhow!("Invalid unlock type: {}", byte)),
        }
    }
}

/// Spillman Witness structure (147 bytes)
/// Structure:
/// - empty_witness_args: 16 bytes (WitnessArgs placeholder)
/// - unlock_type: 1 byte (0x00 = Commitment, 0x01 = Timeout)
/// - merchant_signature: 65 bytes (ECDSA signature, may be empty for timeout)
/// - user_signature: 65 bytes (ECDSA signature)
#[derive(Debug, Clone)]
pub struct SpillmanWitness {
    pub empty_witness_args: [u8; 16],
    pub unlock_type: UnlockType,
    pub merchant_signature: [u8; 65],
    pub user_signature: [u8; 65],
}

impl SpillmanWitness {
    /// Create a new empty witness with specified unlock type
    pub fn new(unlock_type: UnlockType) -> Self {
        Self {
            empty_witness_args: [0u8; 16],
            unlock_type,
            merchant_signature: [0u8; 65],
            user_signature: [0u8; 65],
        }
    }

    /// Create a witness with signatures
    pub fn with_signatures(
        unlock_type: UnlockType,
        merchant_signature: Option<[u8; 65]>,
        user_signature: [u8; 65],
    ) -> Self {
        Self {
            empty_witness_args: [0u8; 16],
            unlock_type,
            merchant_signature: merchant_signature.unwrap_or([0u8; 65]),
            user_signature,
        }
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(147);
        bytes.extend_from_slice(&self.empty_witness_args);
        bytes.push(self.unlock_type.to_byte());
        bytes.extend_from_slice(&self.merchant_signature);
        bytes.extend_from_slice(&self.user_signature);
        bytes
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 147 {
            return Err(anyhow!(
                "Invalid witness length: expected 147 bytes, got {}",
                bytes.len()
            ));
        }

        let mut empty_witness_args = [0u8; 16];
        empty_witness_args.copy_from_slice(&bytes[0..16]);

        let unlock_type = UnlockType::from_byte(bytes[16])?;

        let mut merchant_signature = [0u8; 65];
        merchant_signature.copy_from_slice(&bytes[17..82]);

        let mut user_signature = [0u8; 65];
        user_signature.copy_from_slice(&bytes[82..147]);

        Ok(Self {
            empty_witness_args,
            unlock_type,
            merchant_signature,
            user_signature,
        })
    }

    /// Set merchant signature
    pub fn set_merchant_signature(&mut self, signature: [u8; 65]) {
        self.merchant_signature = signature;
    }

    /// Set user signature
    pub fn set_user_signature(&mut self, signature: [u8; 65]) {
        self.user_signature = signature;
    }
}
