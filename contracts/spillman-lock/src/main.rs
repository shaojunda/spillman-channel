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
use alloc::{ffi::CString, vec::Vec};
use ckb_std::{
    ckb_constants::Source,
    ckb_types::{
        bytes::Bytes,
        core::ScriptHashType,
        packed::{Script, Transaction},
        prelude::*,
    },
    error::SysError,
    high_level::{
        load_cell, load_cell_capacity, load_cell_data, load_input_since, load_script,
        load_transaction, load_witness, spawn_cell,
    },
    since::Since,
    syscalls::wait,
};
use hex::encode;

include!(concat!(env!("OUT_DIR"), "/auth_code_hash.rs"));
include!(concat!(env!("OUT_DIR"), "/secp256k1_code_hash.rs"));

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
    CommitmentMustHaveExactlyTwoOutputs,
    RefundMustHaveExactlyOneOutput,
    TimeoutNotReached,
    InvalidLockArgs,
    UserPubkeyHashMismatch,
    MerchantPubkeyHashMismatch,
    EmptyWitnessArgsError,
    ArgsLenError,
    AuthError,
    ExcessiveFee,
    TypeScriptMismatch,
    XudtAmountMismatch,
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
const ARGS_LEN: usize =
    MERCHANT_PUBKEY_HASH_LEN + USER_PUBKEY_HASH_LEN + TIMEOUT_EPOCH_LEN + VERSION_LEN; // 49 bytes

// Script args field offsets
const MERCHANT_PUBKEY_HASH_OFFSET: usize = 0;
const USER_PUBKEY_HASH_OFFSET: usize = MERCHANT_PUBKEY_HASH_OFFSET + MERCHANT_PUBKEY_HASH_LEN; // 20
const TIMEOUT_EPOCH_OFFSET: usize = USER_PUBKEY_HASH_OFFSET + USER_PUBKEY_HASH_LEN; // 40
const VERSION_OFFSET: usize = TIMEOUT_EPOCH_OFFSET + TIMEOUT_EPOCH_LEN; // 48

// Unlock type layout: [unlock_type(1)]
const UNLOCK_TYPE_COMMITMENT: u8 = 0x00; // Commitment Path
const UNLOCK_TYPE_TIMEOUT: u8 = 0x01; // Timeout Path
const UNLOCK_TYPE_LEN: usize = 1;

// Witness layout: [empty_witness_args(16)] + [unlock_type(1)] + [merchant_signature(65)] + [user_signature(65)]
const SIGNATURE_LEN: usize = 65; // Both merchant and user signatures are 65 bytes
const TOTAL_SIGNATURE_LEN: usize = SIGNATURE_LEN * 2;

// Maximum allowed transaction fee (1 CKB = 100,000,000 shannons)
const MAX_FEE: u64 = 100_000_000;

fn verify() -> Result<(), Error> {
    if load_input_since(1, Source::GroupInput).is_ok() {
        return Err(Error::MultipleInputs);
    }

    let mut witness = load_witness(0, Source::GroupInput)?;

    if witness.len() != EMPTY_WITNESS_ARGS.len() + UNLOCK_TYPE_LEN + TOTAL_SIGNATURE_LEN {
        return Err(Error::WitnessLenError);
    }

    // Verify and remove the empty WitnessArgs prefix (16 bytes)
    if witness
        .drain(0..EMPTY_WITNESS_ARGS.len())
        .collect::<Vec<_>>()
        != EMPTY_WITNESS_ARGS
    {
        return Err(Error::EmptyWitnessArgsError);
    }

    // Load transaction once and reuse it
    let tx = load_transaction()?;

    let message = {
        let raw_tx = tx.raw().as_builder().cell_deps(Default::default()).build();
        blake2b_256(raw_tx.as_slice())
    };

    let script = load_script()?;
    let args: Bytes = script.args().unpack();
    if args.len() != ARGS_LEN {
        return Err(Error::ArgsLenError);
    }

    let merchant_pubkey_hash = &args[MERCHANT_PUBKEY_HASH_OFFSET..USER_PUBKEY_HASH_OFFSET];
    let user_pubkey_hash = &args[USER_PUBKEY_HASH_OFFSET..TIMEOUT_EPOCH_OFFSET];
    let timeout_epoch = u64::from_le_bytes(
        args[TIMEOUT_EPOCH_OFFSET..VERSION_OFFSET]
            .try_into()
            .unwrap(),
    );
    let version = args[VERSION_OFFSET];

    if version != 0 {
        return Err(Error::UnsupportedVersion);
    }

    let unlock_type = witness.remove(0);

    match unlock_type {
        UNLOCK_TYPE_COMMITMENT => verify_commitment_path(
            merchant_pubkey_hash,
            user_pubkey_hash,
            message,
            witness,
            &tx,
        )?,
        UNLOCK_TYPE_TIMEOUT => verify_timeout_path(
            merchant_pubkey_hash,
            user_pubkey_hash,
            timeout_epoch,
            message,
            witness,
            &tx,
        )?,
        _ => return Err(Error::InvalidUnlockType),
    }
    Ok(())
}

fn verify_commitment_path(
    merchant_pubkey_hash: &[u8],
    user_pubkey_hash: &[u8],
    message: [u8; 32],
    witness: Vec<u8>,
    tx: &Transaction,
) -> Result<(), Error> {
    let (merchant_signature, user_signature) = witness.split_at(SIGNATURE_LEN);

    // Verify commitment output structure
    verify_commitment_output_structure(merchant_pubkey_hash, user_pubkey_hash, tx)?;

    // Verify user signature
    verify_signature_with_auth(user_pubkey_hash, &message, user_signature)?;

    // Verify merchant signature
    verify_signature_with_auth(merchant_pubkey_hash, &message, merchant_signature)?;

    Ok(())
}

fn verify_timeout_path(
    merchant_pubkey_hash: &[u8],
    user_pubkey_hash: &[u8],
    timeout_epoch: u64,
    message: [u8; 32],
    witness: Vec<u8>,
    tx: &Transaction,
) -> Result<(), Error> {
    let (merchant_signature, user_signature) = witness.split_at(SIGNATURE_LEN);

    let raw_since = load_input_since(0, Source::GroupInput)?;
    let since = Since::new(raw_since);
    let timeout = Since::new(timeout_epoch);

    if since < timeout {
        return Err(Error::TimeoutNotReached);
    }

    // Verify user signature
    verify_signature_with_auth(user_pubkey_hash, &message, user_signature)?;

    // Verify merchant signature
    verify_signature_with_auth(merchant_pubkey_hash, &message, merchant_signature)?;

    // Verify refund output structure
    verify_refund_output_structure(user_pubkey_hash, tx)?;

    Ok(())
}

fn verify_signature_with_auth(
    pubkey_hash: &[u8],
    message: &[u8; 32],
    signature: &[u8],
) -> Result<(), Error> {
    let algorithm_id_str = CString::new(encode([0u8])).unwrap(); // 0x00 = CKB/SECP256K1
    let signature_str = CString::new(encode(signature)).unwrap();
    let message_str = CString::new(encode(message)).unwrap();
    let pubkey_hash_str = CString::new(encode(pubkey_hash)).unwrap();

    let args = [
        algorithm_id_str.as_c_str(),
        signature_str.as_c_str(),
        message_str.as_c_str(),
        pubkey_hash_str.as_c_str(),
    ];

    // Spawn auth contract to verify signature
    let pid = spawn_cell(&AUTH_CODE_HASH, ScriptHashType::Data1, &args, &[])
        .map_err(|_| Error::AuthError)?;

    // Wait for auth contract to complete and check exit code
    let exit_code = wait(pid).map_err(|_| Error::AuthError)?;

    match exit_code {
        0 => Ok(()),
        _ => Err(Error::AuthError),
    }
}

fn verify_commitment_output_structure(
    merchant_pubkey_hash: &[u8],
    user_pubkey_hash: &[u8],
    tx: &Transaction,
) -> Result<(), Error> {
    let outputs = tx.raw().outputs();

    if outputs.len() != 2 {
        return Err(Error::CommitmentMustHaveExactlyTwoOutputs);
    }

    let user_output = outputs.get(0).unwrap();
    let expected_user_lock = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(user_pubkey_hash.pack())
        .build();

    if user_output.lock().calc_script_hash() != expected_user_lock.calc_script_hash() {
        return Err(Error::UserPubkeyHashMismatch);
    }

    let merchant_output = outputs.get(1).unwrap();
    let expected_merchant_lock = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(merchant_pubkey_hash.pack())
        .build();

    if merchant_output.lock().calc_script_hash() != expected_merchant_lock.calc_script_hash() {
        return Err(Error::MerchantPubkeyHashMismatch);
    }

    // Verify type script consistency for xUDT channels
    let input = load_cell(0, Source::GroupInput)?;
    let input_type = input.type_().to_opt();
    let user_output_type = user_output.type_().to_opt();
    let merchant_output_type = merchant_output.type_().to_opt();

    // If input has type script, both outputs must have the same type script
    if let Some(input_t) = input_type {
        let input_type_hash = input_t.calc_script_hash();

        // Verify user output type script
        match user_output_type {
            Some(user_t) if user_t.calc_script_hash() == input_type_hash => {}
            _ => return Err(Error::TypeScriptMismatch),
        }

        // Verify merchant output type script
        match merchant_output_type {
            Some(merchant_t) if merchant_t.calc_script_hash() == input_type_hash => {}
            _ => return Err(Error::TypeScriptMismatch),
        }
    } else {
        // If input has no type script, outputs should not have type script either
        if user_output_type.is_some() || merchant_output_type.is_some() {
            return Err(Error::TypeScriptMismatch);
        }
    }

    Ok(())
}

fn verify_refund_output_structure(user_pubkey_hash: &[u8], tx: &Transaction) -> Result<(), Error> {
    let outputs = tx.raw().outputs();

    if outputs.len() != 1 {
        return Err(Error::RefundMustHaveExactlyOneOutput);
    }

    let output = outputs.get(0).unwrap();

    // 1. Verify lock script (user address)
    let expected_user_lock = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(user_pubkey_hash.pack())
        .build();

    if output.lock().calc_script_hash() != expected_user_lock.calc_script_hash() {
        return Err(Error::UserPubkeyHashMismatch);
    }

    // 2. Load input cell to get type script
    let input = load_cell(0, Source::GroupInput)?;
    let input_type = input.type_().to_opt();
    let output_type = output.type_().to_opt();

    // 3. Verify type script consistency
    match (input_type, output_type) {
        (Some(input_t), Some(output_t)) => {
            // Both have type script, must be the same
            if input_t.calc_script_hash() != output_t.calc_script_hash() {
                return Err(Error::TypeScriptMismatch);
            }

            // 4. Verify xUDT amount consistency (full refund)
            let input_data = load_cell_data(0, Source::GroupInput)?;
            let output_data = tx.raw().outputs_data().get(0).unwrap();
            if input_data != output_data.raw_data() {
                return Err(Error::XudtAmountMismatch);
            }
        }
        (None, None) => {
            // Both have no type script, pure CKB channel, OK
        }
        _ => {
            // One has type script, one doesn't - error
            return Err(Error::TypeScriptMismatch);
        }
    }

    // 5. Verify CKB capacity fee is not excessive
    let input_capacity = load_cell_capacity(0, Source::GroupInput)?;
    let output_capacity: u64 = output.capacity().unpack();

    let fee = input_capacity.saturating_sub(output_capacity);
    if fee > MAX_FEE {
        return Err(Error::ExcessiveFee);
    }

    Ok(())
}
