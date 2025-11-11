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
        packed::{CellDepVec, Script},
        prelude::*,
    },
    error::SysError,
    high_level::{
        load_cell, load_cell_capacity, load_cell_data, load_cell_lock, load_cell_occupied_capacity,
        load_cell_type, load_input_since, load_script, load_transaction, load_witness, spawn_cell,
        QueryIter,
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
    RefundMustHaveOneOrTwoOutputs,
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
    MerchantCapacityExcessive,
    InvalidMultisigConfig,
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

// Auth algorithm IDs
const AUTH_ALGORITHM_CKB: u8 = 0; // CKB/SECP256K1 single-sig
const AUTH_ALGORITHM_CKB_MULTISIG_LEGACY: u8 = 6; // CKB multisig Legacy (hash_type = Type)
const AUTH_ALGORITHM_CKB_MULTISIG_V2: u8 = 7; // CKB multisig V2 (hash_type = Data1)

// Note: When calling ckb_auth, both LEGACY and V2 should use algorithm_id = 6
const AUTH_ALGORITHM_FOR_CKB_AUTH: u8 = 6;

// Script args layout (fixed 50 bytes):
// [merchant_lock_arg(20)] + [user_pubkey_hash(20)] + [timeout(8)] + [algorithm_id(1)] + [version(1)]
//
// Fields:
//   merchant_lock_arg: 20 bytes
//     - Single-sig (algorithm_id=0): blake160(pubkey)
//     - Multi-sig Legacy (algorithm_id=6): blake160(multisig_config)
//     - Multi-sig V2 (algorithm_id=7): blake160(multisig_config)
//       multisig_config format: S | R | M | N | PubKeyHash1 | PubKeyHash2 | ...
//         S = format_version (1 byte: 0=Legacy, 1=V2)
//         R = first_n (1 byte, at least R signatures must match first R pubkeys)
//         M = threshold (1 byte, require M signatures)
//         N = pubkey_cnt (1 byte, total N pubkeys)
//         PubKeyHashX = blake160(pubkey) (20 bytes each)
//   user_pubkey_hash: 20 bytes - blake160(user_pubkey)
//   timeout: 8 bytes - timestamp since value (little-endian u64)
//   algorithm_id: 1 byte
//     - 0: single-sig (CKB default)
//     - 6: multi-sig legacy (hash_type = Type)
//     - 7: multi-sig V2 (hash_type = Data1)
//   version: 1 byte - set to 0
const MERCHANT_LOCK_ARG_LEN: usize = 20;
const USER_PUBKEY_HASH_LEN: usize = 20;
const TIMEOUT_LEN: usize = 8;
const ALGORITHM_ID_LEN: usize = 1;
const VERSION_LEN: usize = 1;
const MULTISIG_HEADER_LEN: usize = 4; // S + R + M + N
const ARGS_LEN: usize =
    MERCHANT_LOCK_ARG_LEN + USER_PUBKEY_HASH_LEN + TIMEOUT_LEN + ALGORITHM_ID_LEN + VERSION_LEN; // 50 bytes

// Script args field offsets (removed - use direct indexing)

// Unlock type layout: [unlock_type(1)]
const UNLOCK_TYPE_COMMITMENT: u8 = 0x00; // Commitment Path
const UNLOCK_TYPE_TIMEOUT: u8 = 0x01; // Timeout Path
const UNLOCK_TYPE_LEN: usize = 1;

// Witness layout:
// Single-sig (algorithm_id=0):
//   [empty_witness_args(16)] + [unlock_type(1)] + [merchant_signature(65)] + [user_signature(65)]
//   Total: 16 + 1 + 65 + 65 = 147 bytes
//
// Multi-sig (algorithm_id=6):
//   [empty_witness_args(16)] + [unlock_type(1)] + [multisig_config(4+N*20)] + [merchant_signatures(M*65)] + [user_signature(65)]
//   multisig_config: S(1) + R(1) + M(1) + N(1) + PubKeyHash1(20) + ... + PubKeyHashN(20)
//   Total: 16 + 1 + (4+N*20) + M*65 + 65
const SIGNATURE_LEN: usize = 65; // Each signature is 65 bytes

// Maximum allowed transaction fee (1 CKB = 100,000,000 shannons)
const MAX_FEE: u64 = 100_000_000;

fn verify() -> Result<(), Error> {
    if load_input_since(1, Source::GroupInput).is_ok() {
        return Err(Error::MultipleInputs);
    }

    let mut witness = load_witness(0, Source::GroupInput)?;

    // Check minimum witness length
    if witness.len() < EMPTY_WITNESS_ARGS.len() + UNLOCK_TYPE_LEN + SIGNATURE_LEN {
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

    let message = {
        let raw_tx = load_transaction()?
            .raw()
            .as_builder()
            .cell_deps(CellDepVec::default())
            .build();
        blake2b_256(raw_tx.as_slice())
    };

    let script = load_script()?;
    let args: Bytes = script.args().unpack();

    // Verify args length (fixed 50 bytes)
    if args.len() != ARGS_LEN {
        return Err(Error::ArgsLenError);
    }

    // Parse args fields
    let merchant_lock_arg = &args[0..MERCHANT_LOCK_ARG_LEN];
    let user_pubkey_hash =
        &args[MERCHANT_LOCK_ARG_LEN..MERCHANT_LOCK_ARG_LEN + USER_PUBKEY_HASH_LEN];
    let timeout = u64::from_le_bytes(
        args[MERCHANT_LOCK_ARG_LEN + USER_PUBKEY_HASH_LEN
            ..MERCHANT_LOCK_ARG_LEN + USER_PUBKEY_HASH_LEN + TIMEOUT_LEN]
            .try_into()
            .map_err(|_| Error::LengthNotEnough)?,
    );
    let algorithm_id = args[MERCHANT_LOCK_ARG_LEN + USER_PUBKEY_HASH_LEN + TIMEOUT_LEN];
    let version =
        args[MERCHANT_LOCK_ARG_LEN + USER_PUBKEY_HASH_LEN + TIMEOUT_LEN + ALGORITHM_ID_LEN];

    if version != 0 {
        return Err(Error::UnsupportedVersion);
    }

    let unlock_type = witness.remove(0);

    // Determine merchant signature type based on algorithm_id
    // After removing empty_witness_args(16) and unlock_type(1), remaining witness is:
    // - Single-sig (algorithm_id=0): merchant_sig(65) + user_sig(65) = 130 bytes
    // - Multi-sig (algorithm_id=6 or 7): multisig_config(4+N*20) + merchant_sigs(M*65) + user_sig(65)
    let (merchant_algorithm_id, merchant_lock_arg_for_auth) = match algorithm_id {
        AUTH_ALGORITHM_CKB => {
            // Single-sig: witness should be exactly 130 bytes (merchant_sig + user_sig)
            if witness.len() != 130 {
                return Err(Error::WitnessLenError);
            }
            (AUTH_ALGORITHM_CKB, merchant_lock_arg.to_vec())
        }
        AUTH_ALGORITHM_CKB_MULTISIG_LEGACY | AUTH_ALGORITHM_CKB_MULTISIG_V2 => {
            // Multi-sig: extract and verify multisig_config from witness
            if witness.len() < MULTISIG_HEADER_LEN + SIGNATURE_LEN {
                return Err(Error::WitnessLenError);
            }

            // Verify multisig_config format version
            // Both Legacy and V2 use format_version=0 to support both
            if witness[0] != 0 {
                return Err(Error::InvalidMultisigConfig);
            }

            // Parse multisig header to determine config length
            let pubkey_cnt = witness[3] as usize;
            let multisig_config_len = MULTISIG_HEADER_LEN + pubkey_cnt * MERCHANT_LOCK_ARG_LEN;

            if witness.len() < multisig_config_len + SIGNATURE_LEN {
                return Err(Error::WitnessLenError);
            }

            // Extract multisig_config from witness
            let multisig_config = witness[0..multisig_config_len].to_vec();

            // Verify blake160(multisig_config) == merchant_lock_arg
            let multisig_hash = &blake2b_256(&multisig_config)[0..20];
            if multisig_hash != merchant_lock_arg {
                return Err(Error::InvalidMultisigConfig);
            }

            // Remove multisig_config from witness, leaving only signatures
            witness.drain(0..multisig_config_len);

            // Use the same algorithm_id for auth verification
            (algorithm_id, multisig_config)
        }
        _ => return Err(Error::InvalidLockArgs),
    };

    match unlock_type {
        UNLOCK_TYPE_COMMITMENT => verify_commitment_path(
            merchant_algorithm_id,
            &merchant_lock_arg_for_auth,
            user_pubkey_hash,
            message,
            witness,
        )?,
        UNLOCK_TYPE_TIMEOUT => verify_timeout_path(
            merchant_algorithm_id,
            &merchant_lock_arg_for_auth,
            user_pubkey_hash,
            timeout,
            message,
            witness,
        )?,
        _ => return Err(Error::InvalidUnlockType),
    }
    Ok(())
}

fn verify_commitment_path(
    merchant_algorithm_id: u8,
    merchant_lock_arg: &[u8],
    user_pubkey_hash: &[u8],
    message: [u8; 32],
    witness: Vec<u8>,
) -> Result<(), Error> {
    // Split witness into merchant part and user signature
    // - Single-sig: merchant_sig(65) + user_sig(65)
    // - Multi-sig: merchant_sigs(M*65) + user_sig(65)
    let merchant_sig_len = witness.len() - SIGNATURE_LEN;
    let (merchant_signature, user_signature) = witness.split_at(merchant_sig_len);

    // Verify commitment output structure
    verify_commitment_output_structure(merchant_lock_arg, user_pubkey_hash, merchant_algorithm_id)?;

    // Verify user signature (always single-sig)
    verify_signature_with_auth(
        AUTH_ALGORITHM_CKB,
        user_pubkey_hash,
        &message,
        user_signature,
    )?;

    // Verify merchant signature
    verify_merchant_signature(
        merchant_algorithm_id,
        merchant_lock_arg,
        merchant_signature,
        &message,
    )?;

    Ok(())
}

fn verify_timeout_path(
    merchant_algorithm_id: u8,
    merchant_lock_arg: &[u8],
    user_pubkey_hash: &[u8],
    timeout: u64,
    message: [u8; 32],
    witness: Vec<u8>,
) -> Result<(), Error> {
    // Split witness into merchant part and user signature
    // - Single-sig: merchant_sig(65) + user_sig(65)
    // - Multi-sig: merchant_sigs(M*65) + user_sig(65)
    let merchant_sig_len = witness.len() - SIGNATURE_LEN;
    let (merchant_signature, user_signature) = witness.split_at(merchant_sig_len);

    let raw_since = load_input_since(0, Source::GroupInput)?;
    let since = Since::new(raw_since);
    let timeout_since = Since::new(timeout);

    // Security: Only proceed with verification if since >= timeout
    if since >= timeout_since {
        // Verify refund output structure
        verify_refund_output_structure(merchant_lock_arg, user_pubkey_hash, merchant_algorithm_id)?;

        // Verify user signature (always single-sig)
        verify_signature_with_auth(
            AUTH_ALGORITHM_CKB,
            user_pubkey_hash,
            &message,
            user_signature,
        )?;

        // Verify merchant signature
        verify_merchant_signature(
            merchant_algorithm_id,
            merchant_lock_arg,
            merchant_signature,
            &message,
        )?;

        Ok(())
    } else {
        Err(Error::TimeoutNotReached)
    }
}

fn verify_merchant_signature(
    merchant_algorithm_id: u8,
    merchant_lock_arg: &[u8],
    merchant_signature: &[u8],
    message: &[u8; 32],
) -> Result<(), Error> {
    // For multisig (algorithm_id=6 or 7), auth expects:
    //   - lock_arg: blake160(multisig_config) - 20 bytes
    //   - signature: multisig_config + M*65 signatures
    if merchant_algorithm_id == AUTH_ALGORITHM_CKB_MULTISIG_LEGACY
        || merchant_algorithm_id == AUTH_ALGORITHM_CKB_MULTISIG_V2
    {
        // merchant_lock_arg contains the full multisig_config
        // merchant_signature contains M*65 bytes of signatures
        // Combine them for signature parameter: multisig_config + signatures
        let mut multisig_witness = merchant_lock_arg.to_vec();
        multisig_witness.extend_from_slice(merchant_signature);

        // Calculate blake160 hash for lock_arg parameter
        let multisig_hash = &blake2b_256(merchant_lock_arg)[0..20];
        verify_signature_with_auth(
            merchant_algorithm_id,
            multisig_hash,
            message,
            &multisig_witness,
        )
    } else {
        // Single-sig: signature is just 65 bytes
        verify_signature_with_auth(
            merchant_algorithm_id,
            merchant_lock_arg,
            message,
            merchant_signature,
        )
    }
}

fn verify_signature_with_auth(
    algorithm_id: u8,
    lock_arg: &[u8],
    message: &[u8; 32],
    signature: &[u8],
) -> Result<(), Error> {
    // Map algorithm_id for ckb_auth:
    // - Both Legacy (6) and V2 (7) multisig use algorithm_id = 6 in ckb_auth
    let auth_algorithm_id = if algorithm_id == AUTH_ALGORITHM_CKB_MULTISIG_V2 {
        AUTH_ALGORITHM_FOR_CKB_AUTH
    } else {
        algorithm_id
    };

    let algorithm_id_str = CString::new(encode([auth_algorithm_id])).unwrap();
    let signature_str = CString::new(encode(signature)).unwrap();
    let message_str = CString::new(encode(message)).unwrap();
    let lock_arg_str = CString::new(encode(lock_arg)).unwrap();

    let args = [
        algorithm_id_str.as_c_str(),
        signature_str.as_c_str(),
        message_str.as_c_str(),
        lock_arg_str.as_c_str(),
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
    merchant_lock_data: &[u8],
    user_pubkey_hash: &[u8],
    algorithm_id: u8,
) -> Result<(), Error> {
    // Verify that there are exactly two outputs
    if load_cell(2, Source::Output).is_ok() {
        return Err(Error::CommitmentMustHaveExactlyTwoOutputs);
    }

    // Verify that there is a merchant output
    if load_cell(1, Source::Output).is_err() {
        return Err(Error::CommitmentMustHaveExactlyTwoOutputs);
    }

    let user_lock = load_cell_lock(0, Source::Output)?;

    let expected_user_lock = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type)
        .args(user_pubkey_hash.pack())
        .build();

    if user_lock != expected_user_lock {
        return Err(Error::UserPubkeyHashMismatch);
    }

    // Build expected merchant lock based on algorithm_id
    // Note: merchant_lock_data parameter contains:
    //   - Single-sig (algorithm_id=0): 20 bytes blake160(pubkey) from args
    //   - Multi-sig (algorithm_id=6 or 7): 4+N*20 bytes full multisig_config from witness
    let expected_merchant_lock = if merchant_lock_data.len() == MERCHANT_LOCK_ARG_LEN {
        // Single-sig output: code_hash=SECP256K1, args=blake160(pubkey) (20 bytes)
        Script::new_builder()
            .code_hash(SECP256K1_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type)
            .args(merchant_lock_data.pack())
            .build()
    } else {
        // Multi-sig output: code_hash=SECP256K1_MULTISIG, args=blake160(multisig_config) (20 bytes)
        // Need to hash the full multisig_config to get the 20-byte args
        let multisig_hash = &blake2b_256(merchant_lock_data)[0..20];

        // Determine code_hash and hash_type based on algorithm_id:
        // - algorithm_id = 6: Legacy multisig (code_hash = SECP256K1_MULTISIG_CODE_HASH, hash_type = Type)
        // - algorithm_id = 7: V2 multisig (code_hash = SECP256K1_MULTISIG_V2_CODE_HASH, hash_type = Data1)
        let (code_hash, hash_type) = if algorithm_id == AUTH_ALGORITHM_CKB_MULTISIG_V2 {
            (SECP256K1_MULTISIG_V2_CODE_HASH, ScriptHashType::Data1)
        } else {
            (SECP256K1_MULTISIG_CODE_HASH, ScriptHashType::Type)
        };

        Script::new_builder()
            .code_hash(code_hash.pack())
            .hash_type(hash_type)
            .args(multisig_hash.pack())
            .build()
    };

    let merchant_lock = load_cell_lock(1, Source::Output)?;

    if merchant_lock != expected_merchant_lock {
        return Err(Error::MerchantPubkeyHashMismatch);
    }

    // Verify type script consistency for xUDT channels
    let type_script = load_cell_type(0, Source::GroupInput)?;
    let user_output_type = load_cell_type(0, Source::Output)?;
    let merchant_output_type = load_cell_type(1, Source::Output)?;

    // If input has type script, both outputs must have the same type script
    if let Some(input_t) = type_script {
        // Verify user output type script
        if let Some(user_t) = user_output_type {
            if user_t != input_t {
                return Err(Error::TypeScriptMismatch);
            }
        }

        // Verify merchant output type script and xUDT amount
        if let Some(merchant_t) = merchant_output_type {
            // Verify type script matches input
            if merchant_t != input_t {
                return Err(Error::TypeScriptMismatch);
            }
            // Merchant has type script: verify xUDT amount > 0 (merchant receives payment)
            let merchant_output_data = load_cell_data(1, Source::Output)?;
            // xUDT amount is stored in first 16 bytes (u128 little-endian)
            if merchant_output_data.len() < 16 {
                return Err(Error::XudtAmountMismatch);
            }
            if merchant_output_data[0..16] == [0u8; 16] {
                return Err(Error::XudtAmountMismatch);
            }
        }
    } else {
        // If input has no type script, outputs should not have type script either
        if user_output_type.is_some() || merchant_output_type.is_some() {
            return Err(Error::TypeScriptMismatch);
        }
    }

    Ok(())
}

fn verify_refund_output_structure(
    merchant_lock_data: &[u8],
    user_pubkey_hash: &[u8],
    algorithm_id: u8,
) -> Result<(), Error> {
    // Refund can have 1 or 2 outputs
    // 1 output: user funded alone
    // 2 outputs: user + merchant co-funded (merchant gets capacity back)
    if load_cell(2, Source::Output).is_ok() {
        return Err(Error::RefundMustHaveOneOrTwoOutputs);
    }

    // 1. Verify Output 0 is user address
    let user_lock = load_cell_lock(0, Source::Output)?;
    let expected_user_lock = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type)
        .args(user_pubkey_hash.pack())
        .build();

    if user_lock != expected_user_lock {
        return Err(Error::UserPubkeyHashMismatch);
    }

    // 2. If there's Output 1, verify it's merchant address and capacity is exact
    if let Ok(merchant_output) = load_cell(1, Source::Output) {
        // Build expected merchant lock based on algorithm_id
        // Note: merchant_lock_data parameter contains:
        //   - Single-sig (algorithm_id=0): 20 bytes blake160(pubkey) from args
        //   - Multi-sig (algorithm_id=6 or 7): 4+N*20 bytes full multisig_config from witness
        let expected_merchant_lock = if merchant_lock_data.len() == MERCHANT_LOCK_ARG_LEN {
            // Single-sig output: code_hash=SECP256K1, args=blake160(pubkey) (20 bytes)
            Script::new_builder()
                .code_hash(SECP256K1_CODE_HASH.pack())
                .hash_type(ScriptHashType::Type)
                .args(merchant_lock_data.pack())
                .build()
        } else {
            // Multi-sig output: code_hash=SECP256K1_MULTISIG, args=blake160(multisig_config) (20 bytes)
            // Need to hash the full multisig_config to get the 20-byte args
            let multisig_hash = &blake2b_256(merchant_lock_data)[0..20];

            // Determine code_hash and hash_type based on algorithm_id:
            // - algorithm_id = 6: Legacy multisig (code_hash = SECP256K1_MULTISIG_CODE_HASH, hash_type = Type)
            // - algorithm_id = 7: V2 multisig (code_hash = SECP256K1_MULTISIG_V2_CODE_HASH, hash_type = Data1)
            let (code_hash, hash_type) = if algorithm_id == AUTH_ALGORITHM_CKB_MULTISIG_V2 {
                (SECP256K1_MULTISIG_V2_CODE_HASH, ScriptHashType::Data1)
            } else {
                (SECP256K1_MULTISIG_CODE_HASH, ScriptHashType::Type)
            };

            Script::new_builder()
                .code_hash(code_hash.pack())
                .hash_type(hash_type)
                .args(multisig_hash.pack())
                .build()
        };

        if merchant_output.lock() != expected_merchant_lock {
            return Err(Error::MerchantPubkeyHashMismatch);
        }

        // Verify merchant output capacity equals exactly the occupied capacity
        // Merchant can only take back what's needed for cell occupation (no more, no less)
        let min_capacity = load_cell_occupied_capacity(1, Source::Output)?;
        let actual_capacity: u64 = merchant_output.capacity().unpack();

        if actual_capacity != min_capacity {
            return Err(Error::MerchantCapacityExcessive);
        }
    }

    // 3. Load input cell to get type script
    let input_type = load_cell_type(0, Source::GroupInput)?;

    // 4. Verify type script consistency and xUDT amounts
    if let Some(input_t) = input_type {
        // Verify user output (Output 0) has same type script and all xUDT
        // Use load_cell_type API for reliable checking
        let user_output_type = load_cell_type(0, Source::Output)?;
        if let Some(user_t) = user_output_type {
            if user_t != input_t {
                return Err(Error::TypeScriptMismatch);
            }
        }

        // Verify user gets all xUDT (full refund)
        let input_data = load_cell_data(0, Source::GroupInput)?;
        let user_output_data = load_cell_data(0, Source::Output)?;
        if input_data != user_output_data {
            return Err(Error::XudtAmountMismatch);
        }

        // If there's merchant output (Output 1), verify type script and xUDT amount = 0
        if let Ok(_merchant_output) = load_cell(1, Source::Output) {
            let merchant_output_type = load_cell_type(1, Source::Output)?;
            if let Some(merchant_t) = merchant_output_type {
                if merchant_t != input_t {
                    return Err(Error::TypeScriptMismatch);
                }
                // Verify merchant xUDT amount is 0 (only gets CKB capacity back)
                let merchant_output_data = load_cell_data(1, Source::Output)?;
                // xUDT amount is stored in first 16 bytes (u128 little-endian)
                if merchant_output_data.len() < 16 {
                    return Err(Error::XudtAmountMismatch);
                }
                // Check if amount is zero
                if merchant_output_data[0..16] != [0u8; 16] {
                    return Err(Error::XudtAmountMismatch);
                }
            }
        }
    } else {
        // Pure CKB channel: no outputs should have type script
        // Use load_cell_type API for reliable checking
        let user_output_type = load_cell_type(0, Source::Output)?;
        if user_output_type.is_some() {
            return Err(Error::TypeScriptMismatch);
        }

        if let Ok(_merchant_output) = load_cell(1, Source::Output) {
            let merchant_output_type = load_cell_type(1, Source::Output)?;
            if merchant_output_type.is_some() {
                return Err(Error::TypeScriptMismatch);
            }
        }
    }

    // 5. Verify CKB capacity fee is not excessive
    let input_capacity = load_cell_capacity(0, Source::GroupInput)?;

    // Collect all outputs capacity (1 or 2 outputs)
    let total_output_capacity = QueryIter::new(load_cell_capacity, Source::Output).sum();

    let fee = input_capacity.saturating_sub(total_output_capacity);
    if fee > MAX_FEE {
        return Err(Error::ExcessiveFee);
    }

    Ok(())
}
