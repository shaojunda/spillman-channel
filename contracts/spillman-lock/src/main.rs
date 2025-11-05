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
        core::{ScriptHashType},
        packed::{Script, Transaction},
        prelude::*,
    },
    error::SysError,
    high_level::{
        load_cell, load_cell_capacity, load_cell_data, load_cell_occupied_capacity,
        load_input_since, load_script, load_transaction, load_witness, spawn_cell, QueryIter,
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
const AUTH_ALGORITHM_CKB: u8 = 0;                 // CKB/SECP256K1 single-sig
const AUTH_ALGORITHM_CKB_MULTISIG_LEGACY: u8 = 6; // CKB multisig Legacy (hash_type = Type)
const AUTH_ALGORITHM_CKB_MULTISIG_V2: u8 = 7;     // CKB multisig V2 (hash_type = Data1)

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

    // Load transaction once and reuse it
    let tx = load_transaction()?;

    let message = {
        use ckb_std::ckb_types::packed::CellDepVec;
        let raw_tx = tx.raw().as_builder().cell_deps(CellDepVec::default()).build();
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
    let user_pubkey_hash = &args[MERCHANT_LOCK_ARG_LEN..MERCHANT_LOCK_ARG_LEN + USER_PUBKEY_HASH_LEN];
    let timeout = u64::from_le_bytes(
        args[MERCHANT_LOCK_ARG_LEN + USER_PUBKEY_HASH_LEN
            ..MERCHANT_LOCK_ARG_LEN + USER_PUBKEY_HASH_LEN + TIMEOUT_LEN]
            .try_into()
            .unwrap(),
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
            &tx,
        )?,
        UNLOCK_TYPE_TIMEOUT => verify_timeout_path(
            merchant_algorithm_id,
            &merchant_lock_arg_for_auth,
            user_pubkey_hash,
            timeout,
            message,
            witness,
            &tx,
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
    tx: &Transaction,
) -> Result<(), Error> {
    // Split witness into merchant part and user signature
    // - Single-sig: merchant_sig(65) + user_sig(65)
    // - Multi-sig: merchant_sigs(M*65) + user_sig(65)
    let merchant_sig_len = witness.len() - SIGNATURE_LEN;
    let (merchant_signature, user_signature) = witness.split_at(merchant_sig_len);

    // Verify commitment output structure
    verify_commitment_output_structure(merchant_lock_arg, user_pubkey_hash, merchant_algorithm_id, tx)?;

    // Verify user signature (always single-sig)
    verify_signature_with_auth(AUTH_ALGORITHM_CKB, user_pubkey_hash, &message, user_signature)?;

    // Verify merchant signature
    verify_merchant_signature(merchant_algorithm_id, merchant_lock_arg, merchant_signature, &message)?;

    Ok(())
}

fn verify_timeout_path(
    merchant_algorithm_id: u8,
    merchant_lock_arg: &[u8],
    user_pubkey_hash: &[u8],
    timeout: u64,
    message: [u8; 32],
    witness: Vec<u8>,
    tx: &Transaction,
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
        // Verify user signature (always single-sig)
        verify_signature_with_auth(AUTH_ALGORITHM_CKB, user_pubkey_hash, &message, user_signature)?;

        // Verify merchant signature
        verify_merchant_signature(merchant_algorithm_id, merchant_lock_arg, merchant_signature, &message)?;

        // Verify refund output structure
        verify_refund_output_structure(merchant_lock_arg, user_pubkey_hash, merchant_algorithm_id, tx)?;

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
        || merchant_algorithm_id == AUTH_ALGORITHM_CKB_MULTISIG_V2 {
        // merchant_lock_arg contains the full multisig_config
        // merchant_signature contains M*65 bytes of signatures
        // Combine them for signature parameter: multisig_config + signatures
        let mut multisig_witness = merchant_lock_arg.to_vec();
        multisig_witness.extend_from_slice(merchant_signature);

        // Calculate blake160 hash for lock_arg parameter
        let multisig_hash = &blake2b_256(merchant_lock_arg)[0..20];
        verify_signature_with_auth(merchant_algorithm_id, multisig_hash, message, &multisig_witness)
    } else {
        // Single-sig: signature is just 65 bytes
        verify_signature_with_auth(merchant_algorithm_id, merchant_lock_arg, message, merchant_signature)
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
    tx: &Transaction,
) -> Result<(), Error> {
    let outputs = tx.raw().outputs();

    if outputs.len() != 2 {
        return Err(Error::CommitmentMustHaveExactlyTwoOutputs);
    }

    let user_output = outputs.get(0).unwrap();
    let expected_user_lock = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type)
        .args(user_pubkey_hash.pack())
        .build();

    if user_output.lock().code_hash() != expected_user_lock.code_hash()
    || user_output.lock().hash_type() != expected_user_lock.hash_type()
    || user_output.lock().args() != expected_user_lock.args()
    {
        return Err(Error::UserPubkeyHashMismatch);
    }

    let merchant_output = outputs.get(1).unwrap();

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

        // Determine hash_type based on algorithm_id:
        // - algorithm_id = 6: Legacy multisig (hash_type = Type)
        // - algorithm_id = 7: V2 multisig (hash_type = Data1)
        let hash_type = if algorithm_id == AUTH_ALGORITHM_CKB_MULTISIG_V2 {
            ScriptHashType::Data1
        } else {
            ScriptHashType::Type
        };

        Script::new_builder()
            .code_hash(SECP256K1_MULTISIG_CODE_HASH.pack())
            .hash_type(hash_type)
            .args(multisig_hash.pack())
            .build()
    };

    if merchant_output.lock().code_hash() != expected_merchant_lock.code_hash()
    || merchant_output.lock().hash_type() != expected_merchant_lock.hash_type()
    || merchant_output.lock().args() != expected_merchant_lock.args()
    {
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

fn verify_refund_output_structure(
    merchant_lock_data: &[u8],
    user_pubkey_hash: &[u8],
    algorithm_id: u8,
    tx: &Transaction,
) -> Result<(), Error> {
    let outputs = tx.raw().outputs();
    let outputs_data = tx.raw().outputs_data();

    // Refund can have 1 or 2 outputs
    // 1 output: user funded alone
    // 2 outputs: user + merchant co-funded (merchant gets capacity back)
    if outputs.len() > 2 {
        return Err(Error::RefundMustHaveOneOrTwoOutputs);
    }

    // 1. Verify Output 0 is user address
    let user_output = outputs.get(0).unwrap();
    let expected_user_lock = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type)
        .args(user_pubkey_hash.pack())
        .build();

    if user_output.lock().code_hash() != expected_user_lock.code_hash()
    || user_output.lock().hash_type() != expected_user_lock.hash_type()
    || user_output.lock().args() != expected_user_lock.args()
    {
        return Err(Error::UserPubkeyHashMismatch);
    }

    // 2. If there's Output 1, verify it's merchant address and capacity is exact
    if outputs.len() == 2 {
        let merchant_output = outputs.get(1).unwrap();

        // Build expected merchant lock based on whether it's single-sig or multi-sig
        // Note: merchant_lock_data parameter contains:
        //   - Single-sig: 20 bytes blake160(pubkey) from args
        //   - Multi-sig: 4+N*20 bytes full multisig_config from witness
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

            // Determine hash_type based on algorithm_id:
            // - algorithm_id = 6: Legacy multisig (hash_type = Type)
            // - algorithm_id = 7: V2 multisig (hash_type = Data1)
            let hash_type = if algorithm_id == AUTH_ALGORITHM_CKB_MULTISIG_V2 {
                ScriptHashType::Data1
            } else {
                ScriptHashType::Type
            };

            Script::new_builder()
                .code_hash(SECP256K1_MULTISIG_CODE_HASH.pack())
                .hash_type(hash_type)
                .args(multisig_hash.pack())
                .build()
        };

        if merchant_output.lock().code_hash() != expected_merchant_lock.code_hash()
        || merchant_output.lock().hash_type() != expected_merchant_lock.hash_type()
        || merchant_output.lock().args() != expected_merchant_lock.args()
        {
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
    let input = load_cell(0, Source::GroupInput)?;
    let input_type = input.type_().to_opt();

    // 4. Verify type script consistency and xUDT amounts
    if let Some(input_t) = input_type {
        let input_type_hash = input_t.calc_script_hash();

        // Verify user output (Output 0) has same type script and all xUDT
        let user_output_type = user_output.type_().to_opt();
        match user_output_type {
            Some(user_t) if user_t.calc_script_hash() == input_type_hash => {
                // Verify user gets all xUDT (full refund)
                let input_data = load_cell_data(0, Source::GroupInput)?;
                let user_output_data = outputs_data.get(0).unwrap();
                if input_data != user_output_data.raw_data() {
                    return Err(Error::XudtAmountMismatch);
                }
            }
            _ => return Err(Error::TypeScriptMismatch),
        }

        // If there's merchant output (Output 1), verify type script and xUDT amount = 0
        if outputs.len() == 2 {
            let merchant_output = outputs.get(1).unwrap();
            let merchant_output_type = merchant_output.type_().to_opt();

            match merchant_output_type {
                Some(merchant_t) if merchant_t.calc_script_hash() == input_type_hash => {
                    // Verify merchant xUDT amount is 0 (only gets CKB capacity back)
                    let merchant_output_data = outputs_data.get(1).unwrap();

                    // xUDT amount is stored in first 16 bytes (u128 little-endian)
                    // Check if it's all zeros
                    if merchant_output_data.len() < 16
                        || merchant_output_data.raw_data()[0..16] != [0u8; 16]
                    {
                        return Err(Error::XudtAmountMismatch);
                    }
                }
                _ => return Err(Error::TypeScriptMismatch),
            }
        }
    } else {
        // Pure CKB channel: no outputs should have type script
        if user_output.type_().to_opt().is_some() {
            return Err(Error::TypeScriptMismatch);
        }

        if outputs.len() == 2 {
            let merchant_output = outputs.get(1).unwrap();
            if merchant_output.type_().to_opt().is_some() {
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
