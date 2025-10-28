#![cfg_attr(not(any(feature = "library", test)), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(any(feature = "library", test))]
extern crate alloc;

use ckb_hash::blake2b_256;

#[cfg(not(any(feature = "library", test)))]
ckb_std::entry!(program_entry);
#[cfg(not(any(feature = "library", test)))]
// By default, the following heap configuration is used:
// * 16KB fixed heap
// * 1.2MB(rounded up to be 16-byte aligned) dynamic heap
// * Minimal memory block in dynamic heap is 64 bytes
// For more details, please refer to ckb-std's default_alloc macro
// and the buddy-alloc alloc implementation.
ckb_std::default_alloc!(16384, 1258306, 64);
use alloc::{vec::Vec};
use ckb_std::{
    ckb_constants::Source,
    ckb_types::{bytes::Bytes, prelude::*},
    error::SysError,
    high_level::{load_input_since, load_transaction, load_witness, load_script},
};

#[repr(i8)]
pub enum Error {
    IndexOutOfBound = 1,
    ItemMissing,
    LengthNotEnough,
    Encoding,
    // Add customized errors here...
    MultipleInputs,
    WitnessLenError,
    UnsupportedVersion,
    InvalidUnlockType,
    EmptyWitnessArgsError,
    ArgsLenError,
    AuthError,
}

impl From<SysError> for Error {
    fn from(err: SysError) -> Self {
        match err {
            SysError::IndexOutOfBound => Self::IndexOutOfBound,
            SysError::ItemMissing => Self::ItemMissing,
            SysError::LengthNotEnough(_) => Self::LengthNotEnough,
            SysError::Encoding => Self::Encoding,
            _ => panic!("unexpected sys error"),
        }
    }
}


pub fn program_entry() -> i8 {
   match verify() {
    Ok(_) => 0,
    Err(err) => err as i8,
   }
}

// a placeholder for empty witness args, to resolve the issue of xudt compatibility
const EMPTY_WITNESS_ARGS: [u8; 16] = [16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0];

// Script args layout: [merchant_pubkey_hash(20)] + [user_pubkey_hash(20)] + [timeout_epoch(8)] + [version(1)]
const MERCHANT_PUBKEY_HASH_LEN: usize = 20;
const USER_PUBKEY_HASH_LEN: usize = 20;
const TIMEOUT_EPOCH_LEN: usize = 8;
const VERSION_LEN: usize = 1;
const ARGS_LEN: usize = MERCHANT_PUBKEY_HASH_LEN + USER_PUBKEY_HASH_LEN + TIMEOUT_EPOCH_LEN + VERSION_LEN; // 49 bytes

// Script args field offsets
const MERCHANT_PUBKEY_HASH_OFFSET: usize = 0;
const USER_PUBKEY_HASH_OFFSET: usize = MERCHANT_PUBKEY_HASH_OFFSET + MERCHANT_PUBKEY_HASH_LEN; // 20
const TIMEOUT_EPOCH_OFFSET: usize = USER_PUBKEY_HASH_OFFSET + USER_PUBKEY_HASH_LEN; // 40
const VERSION_OFFSET: usize = TIMEOUT_EPOCH_OFFSET + TIMEOUT_EPOCH_LEN; // 48

// Unlock type layout: [unlock_type(1)]
const UNLOCK_TYPE_COMMITMENT: u8 = 0x00;  // Commitment Path
const UNLOCK_TYPE_TIMEOUT: u8 = 0x01;     // Timeout Path
const UNLOCK_TYPE_LEN: usize = 1;

// Witness layout: [empty_witness_args(16)] + [unlock_type(1)] + [merchant_signature(65)] + [user_signature(65)]
const SIGNATURE_LEN: usize = 65;  // Both merchant and user signatures are 65 bytes
const TOTAL_SIGNATURE_LEN: usize = SIGNATURE_LEN * 2;

fn verify() -> Result<(), Error> {
    if load_input_since(1, Source::GroupInput).is_ok() {
        return Err(Error::MultipleInputs);
    }

    let mut witness = load_witness(0, Source::GroupInput)?;
    if witness.len() != EMPTY_WITNESS_ARGS.len() + UNLOCK_TYPE_LEN + TOTAL_SIGNATURE_LEN {
        return Err(Error::WitnessLenError);
    }

    // Verify and remove the empty WitnessArgs prefix (16 bytes)
    if witness.drain(0..EMPTY_WITNESS_ARGS.len()).collect::<Vec<_>>() != EMPTY_WITNESS_ARGS {
        return Err(Error::EmptyWitnessArgsError);
    }

    let message = {
        let tx = load_transaction()?
            .raw()
            .as_builder()
            .cell_deps(Default::default())
            .build();
        blake2b_256(tx.as_slice())
    };

    let script = load_script()?;
    let args: Bytes = script.args().unpack();
    if args.len() != ARGS_LEN {
        return Err(Error::ArgsLenError);
    }

    let merchant_pubkey_hash = &args[MERCHANT_PUBKEY_HASH_OFFSET..USER_PUBKEY_HASH_OFFSET];
    let user_pubkey_hash = &args[USER_PUBKEY_HASH_OFFSET..TIMEOUT_EPOCH_OFFSET];
    let timeout_epoch = u64::from_le_bytes(args[TIMEOUT_EPOCH_OFFSET..VERSION_OFFSET].try_into().unwrap());
    let version = args[VERSION_OFFSET];

    if version != 0 {
        return Err(Error::UnsupportedVersion);
    }

    let unlock_type = witness.remove(0);

    match unlock_type {
        UNLOCK_TYPE_COMMITMENT => verify_commitment_path(merchant_pubkey_hash, user_pubkey_hash, message, witness)?,
        UNLOCK_TYPE_TIMEOUT => verify_timeout_path(merchant_pubkey_hash, user_pubkey_hash, timeout_epoch, message, witness)?,
        _ => return Err(Error::InvalidUnlockType),
    }
    Ok(())
}

fn verify_commitment_path(merchant_pubkey_hash: &[u8], user_pubkey_hash: &[u8], message: [u8; 32], witness: Vec<u8>) -> Result<(), Error> {
    let (merchant_signature, user_signature) = witness.split_at(SIGNATURE_LEN);

    Ok(())
}

fn verify_timeout_path(merchant_pubkey_hash: &[u8], user_pubkey_hash: &[u8], timeout_epoch: u64, message: [u8; 32], witness: Vec<u8>) -> Result<(), Error> {
    let (merchant_signature, user_signature) = witness.split_at(SIGNATURE_LEN);

    Ok(())
}
