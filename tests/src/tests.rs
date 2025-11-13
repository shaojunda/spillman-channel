use crate::Loader;
use ckb_sdk::util::blake160;
use ckb_std::since::{EpochNumberWithFraction, Since};
use ckb_testtool::context::Context;
use ckb_testtool::{
    ckb_crypto::secp::Generator,
    ckb_hash::blake2b_256,
    ckb_types::{
        bytes::Bytes,
        core::{ScriptHashType, TransactionBuilder, TransactionView},
        packed::*,
        prelude::*,
    },
};

const EMPTY_WITNESS_ARGS: [u8; 16] = [16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0];
const UNLOCK_TYPE_COMMITMENT: u8 = 0x00;
const UNLOCK_TYPE_TIMEOUT: u8 = 0x01;

// Mainnet/Testnet secp256k1_blake160_sighash_all code_hash
const SECP256K1_CODE_HASH: [u8; 32] = [
    0x9b, 0xd7, 0xe0, 0x6f, 0x3e, 0xcf, 0x4b, 0xe0, 0xf2, 0xfc, 0xd2, 0x18, 0x8b, 0x23, 0xf1, 0xb9,
    0xfc, 0xc8, 0x8e, 0x5d, 0x4b, 0x65, 0xa8, 0x63, 0x7b, 0x17, 0x72, 0x3b, 0xbd, 0xa3, 0xcc, 0xe8,
];

// Mainnet/Testnet secp256k1_blake160_multisig_all code_hash
const SECP256K1_MULTISIG_CODE_HASH: [u8; 32] = [
    0x5c, 0x50, 0x69, 0xeb, 0x08, 0x57, 0xef, 0xc6, 0x5e, 0x1b, 0xca, 0x0c, 0x07, 0xdf, 0x34, 0xc3,
    0x16, 0x63, 0xb3, 0x62, 0x2f, 0xd3, 0x87, 0x6c, 0x87, 0x63, 0x20, 0xfc, 0x96, 0x34, 0xe2, 0xa8,
];

// Include your tests here
// See https://github.com/xxuejie/ckb-native-build-sample/blob/main/tests/src/tests.rs for more examples

// generated unit test for contract spillman-lock
#[test]
fn test_spillman_lock_commitment_path() {
    // deploy contract
    let mut context = Context::default();
    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);

    let mut generator = Generator::new();
    let user_key = generator.gen_keypair();
    let merchant_key = generator.gen_keypair();

    // Build SpillmanLockArgs according to design doc
    // struct SpillmanLockArgs {
    //     merchant_pubkey_hash: [u8; 20],  // 0..20
    //     user_pubkey_hash: [u8; 20],      // 20..40
    //     timeout: [u8; 8],                // 40..48 (u64 little-endian, timestamp since)
    //     version: u8,                     // 48
    // }
    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64; // 2025-01-01 00:00:00 UTC
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");
    let algorithm_id: u8 = 0; // Single-sig
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(), // 0..20: merchant lock arg (blake160(pubkey))
        user_pubkey_hash.as_ref(),     // 20..40: user pubkey hash
        &timeout_since.as_u64().to_le_bytes(), // 40..48: timeout timestamp (little-endian)
        &[algorithm_id],               // 48: algorithm_id (0=single-sig)
        &[version],                    // 49: version
    ]
    .concat();

    // prepare scripts
    let lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(args))
        .expect("script");

    // Build lock scripts for user and merchant using mainnet secp256k1 code_hash
    // Note: We manually construct these scripts instead of deploying secp256k1 binary
    // because we only need to verify the output lock script structure, not execute it
    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let merchant_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(merchant_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    // prepare cell deps
    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let cell_deps = vec![spillman_lock_dep, auth_dep].pack();

    // prepare cells
    let input_out_point = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack()) // 1001 CKB
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );

    // total capacity = 1001 CKB
    // input capacity = 1001 CKB
    // output capacity = user refund 500 CKB + merchant payment 500 CKB = 1000 CKB
    // fee = 1 CKB

    let input = CellInput::new_builder()
        .previous_output(input_out_point)
        .build();
    let outputs = vec![
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack()) // 500 CKB
            .lock(user_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack()) // 500 CKB
            .lock(merchant_lock_script)
            .build(),
    ];

    let outputs_data = vec![Bytes::new(); 2];

    // build transaction (base tx without witness)
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps.clone())
        .input(input.clone())
        .outputs(outputs.clone())
        .outputs_data(outputs_data.clone().pack())
        .build();

    let success_tx = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        outputs,
        outputs_data,
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &merchant_key,
    );

    // run
    let cycles = context
        .verify_tx(&success_tx, 10_000_000)
        .expect("pass verification");
    println!("consume cycles: {}", cycles);

    // wrong user signature should fail verification
    let wrong_user_signature = [0u8; 65];
    let merchant_signature = merchant_key
        .0
        .sign_recoverable(&compute_signing_message(&tx).into())
        .unwrap()
        .serialize();
    let wrong_witness = [
        &EMPTY_WITNESS_ARGS[..],
        &[UNLOCK_TYPE_COMMITMENT][..],
        &merchant_signature[..],
        &wrong_user_signature[..],
    ]
    .concat();
    let fail_tx = tx
        .as_advanced_builder()
        .witness(wrong_witness.pack())
        .build();

    // run
    let err = context
        .verify_tx(&fail_tx, 10_000_000)
        .expect_err("wrong user signature should fail verification");
    println!("error: {:?}", err);
}

#[test]
fn test_spillman_lock_timeout_path() {
    // deploy contract
    let mut context = Context::default();
    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);

    let mut generator = Generator::new();
    let user_key = generator.gen_keypair();
    let merchant_key = generator.gen_keypair();

    // Build SpillmanLockArgs with timeout timestamp
    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64; // 2025-01-01 00:00:00 UTC
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");
    let algorithm_id: u8 = 0; // Single-sig
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[algorithm_id],
        &[version],
    ]
    .concat();

    // prepare scripts
    let lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(args))
        .expect("script");

    // Build lock script for user refund using mainnet secp256k1 code_hash
    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    // prepare cell deps
    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let cell_deps = vec![spillman_lock_dep, auth_dep].pack();

    // prepare cells
    let input_out_point = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack()) // 1001 CKB
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );

    // For timeout path: only one output (user refund)
    // Set since to a value greater than timeout_timestamp to simulate timeout
    let since_timestamp = timeout_timestamp + 86400; // 1 day after timeout
    let since_value = Since::from_timestamp(since_timestamp, true).expect("valid since");

    let input = CellInput::new_builder()
        .previous_output(input_out_point.clone())
        .since(since_value.as_u64().pack())
        .build();

    let outputs = vec![CellOutput::new_builder()
        .capacity(100_000_000_000u64.pack()) // 1000 CKB refund to user, 1 CKB fee
        .lock(user_lock_script.clone())
        .build()];

    let outputs_data = vec![Bytes::new(); 1];

    // build transaction
    let success_tx = build_and_sign_tx(
        cell_deps,
        input.clone(),
        outputs,
        outputs_data,
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    // run
    let cycles = context
        .verify_tx(&success_tx, 10_000_000)
        .expect("pass verification");
    println!("consume cycles: {}", cycles);

    // Test: timeout not reached should fail
    let early_timestamp = timeout_timestamp - 3600; // 1 hour before timeout
    let early_since = Since::from_timestamp(early_timestamp, true).expect("valid since");
    let early_input = success_tx
        .inputs()
        .get(0)
        .unwrap()
        .as_builder()
        .since(early_since.as_u64().pack())
        .build();

    let early_tx = success_tx
        .as_advanced_builder()
        .set_inputs(vec![early_input])
        .build();

    let err = context
        .verify_tx(&early_tx, 10_000_000)
        .expect_err("timeout not reached should fail verification");
    println!("error (timeout not reached): {:?}", err);

    // Test: incomparable since types should fail (block-based since vs epoch-based timeout)
    // This tests the security fix: since >= timeout properly rejects incomparable types
    let block_based_since = Since::from_block_number(1000, false).unwrap(); // Block-based since
    let incomparable_input = success_tx
        .inputs()
        .get(0)
        .unwrap()
        .as_builder()
        .since(block_based_since.as_u64().pack())
        .build();

    let incomparable_tx = success_tx
        .as_advanced_builder()
        .set_inputs(vec![incomparable_input])
        .build();

    let err = context
        .verify_tx(&incomparable_tx, 10_000_000)
        .expect_err("incomparable since types should fail verification");
    println!("error (incomparable since types): {:?}", err);

    // Test: invalid unlock type should fail
    let invalid_unlock_type = 0x02; // not COMMITMENT(0x00) or TIMEOUT(0x01)
    let merchant_signature = merchant_key
        .0
        .sign_recoverable(&compute_signing_message(&success_tx).into())
        .unwrap()
        .serialize();
    let user_signature = user_key
        .0
        .sign_recoverable(&compute_signing_message(&success_tx).into())
        .unwrap()
        .serialize();
    let invalid_witness = [
        &EMPTY_WITNESS_ARGS[..],
        &[invalid_unlock_type][..],
        &merchant_signature[..],
        &user_signature[..],
    ]
    .concat();

    let invalid_tx = success_tx
        .as_advanced_builder()
        .set_witnesses(vec![invalid_witness.pack()])
        .build();

    let err = context
        .verify_tx(&invalid_tx, 10_000_000)
        .expect_err("invalid unlock type should fail verification");
    println!("error (invalid unlock type): {:?}", err);

    // Test: excessive fee should fail
    // Create a transaction with small output (high fee) and re-sign it
    // Input: 1001 CKB, Output: 0.5 CKB, Fee: 1000.5 CKB >> MAX_FEE (1 CKB)
    let small_output = CellOutput::new_builder()
        .capacity(50_000_000u64.pack()) // 0.5 CKB
        .lock(user_lock_script.clone())
        .build();

    let excessive_fee_tx = build_and_sign_tx(
        success_tx.cell_deps(),
        input.clone(),
        vec![small_output],
        vec![Bytes::new()],
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&excessive_fee_tx, 10_000_000)
        .expect_err("excessive fee should fail verification");
    println!("error (excessive fee): {:?}", err);
}

#[test]
fn test_spillman_lock_timeout_path_with_co_funding() {
    // Test co-funding scenario: merchant pre-funds their receiving cell capacity
    // Refund transaction should have 2 outputs:
    // - Output 0: user gets their funds back
    // - Output 1: merchant gets their pre-funded capacity back

    let mut context = Context::default();
    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);

    let mut generator = Generator::new();
    let user_key = generator.gen_keypair();
    let merchant_key = generator.gen_keypair();

    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64; // 2025-01-01 00:00:00 UTC
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");
    let algorithm_id: u8 = 0; // Single-sig
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[algorithm_id],
        &[version],
    ]
    .concat();

    let lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(args))
        .expect("script");

    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let merchant_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(merchant_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let cell_deps = vec![spillman_lock_dep, auth_dep].pack();

    // Calculate merchant cell's exact occupied capacity
    // This is what merchant pre-funds and will get back in refund
    let merchant_cell = CellOutput::new_builder()
        .capacity(0u64.pack()) // will calculate
        .lock(merchant_lock_script.clone())
        .build();
    let merchant_occupied = merchant_cell
        .occupied_capacity(ckb_testtool::ckb_types::core::Capacity::bytes(0).unwrap())
        .unwrap(); // 0 data size
    let merchant_capacity_u64: u64 = merchant_occupied.as_u64();

    // Funding cell total: user 1000 CKB + merchant occupied capacity
    let total_capacity = 100_000_000_000u64 + merchant_capacity_u64;

    let input_out_point = context.create_cell(
        CellOutput::new_builder()
            .capacity(total_capacity.pack())
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );

    let since_timestamp = timeout_timestamp + 86400; // 1 day after timeout
    let since_value = Since::from_timestamp(since_timestamp, true).expect("valid since");

    let input = CellInput::new_builder()
        .previous_output(input_out_point.clone())
        .since(since_value.as_u64().pack())
        .build();

    // Co-funding refund: 2 outputs
    // Output 0: User gets 1000 CKB back (minus fee)
    // Output 1: Merchant gets exact occupied capacity back
    // Fee: 1 CKB
    let outputs = vec![
        CellOutput::new_builder()
            .capacity((total_capacity - merchant_capacity_u64 - 100_000_000).pack()) // user refund minus fee
            .lock(user_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(merchant_capacity_u64.pack()) // exact occupied capacity
            .lock(merchant_lock_script.clone())
            .build(),
    ];

    let outputs_data = vec![Bytes::new(); 2];

    let success_tx = build_and_sign_tx(
        cell_deps,
        input.clone(),
        outputs,
        outputs_data,
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let cycles = context
        .verify_tx(&success_tx, 10_000_000)
        .expect("pass verification");
    println!("consume cycles (co-funding refund): {}", cycles);

    // Test: wrong merchant output (not merchant's address) should fail
    let wrong_merchant_lock = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(vec![0u8; 20]).pack()) // wrong pubkey hash
        .build();

    let wrong_outputs = vec![
        CellOutput::new_builder()
            .capacity((total_capacity - merchant_capacity_u64 - 100_000_000).pack())
            .lock(user_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(merchant_capacity_u64.pack())
            .lock(wrong_merchant_lock)
            .build(),
    ];

    let wrong_tx = build_and_sign_tx(
        success_tx.cell_deps(),
        input.clone(),
        wrong_outputs,
        vec![Bytes::new(); 2],
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&wrong_tx, 10_000_000)
        .expect_err("wrong merchant output should fail verification");
    println!("error (wrong merchant output): {:?}", err);

    // Test: merchant capacity exceeds occupied capacity should fail
    let excessive_capacity = merchant_capacity_u64 + 100_000_000; // 1 CKB more than needed
    let excessive_outputs = vec![
        CellOutput::new_builder()
            .capacity((total_capacity - excessive_capacity - 100_000_000).pack())
            .lock(user_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(excessive_capacity.pack()) // merchant takes more than needed!
            .lock(merchant_lock_script.clone())
            .build(),
    ];

    let excessive_tx = build_and_sign_tx(
        success_tx.cell_deps(),
        input.clone(),
        excessive_outputs,
        vec![Bytes::new(); 2],
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&excessive_tx, 10_000_000)
        .expect_err("excessive merchant capacity should fail verification");
    println!("error (excessive merchant capacity): {:?}", err);
}

#[test]
fn test_spillman_lock_timeout_path_with_xudt() {
    // Test xUDT channel refund: user gets all xUDT back

    let mut context = Context::default();
    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let simple_udt_bin: Bytes = loader.load_binary("../../deps/simple_udt");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);
    let simple_udt_out_point = context.deploy_cell(simple_udt_bin);

    let mut generator = Generator::new();
    let user_key = generator.gen_keypair();
    let merchant_key = generator.gen_keypair();

    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64; // 2025-01-01 00:00:00 UTC
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");
    let algorithm_id: u8 = 0; // Single-sig
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[algorithm_id],
        &[version],
    ]
    .concat();

    let lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(args))
        .expect("script");

    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    // Create xUDT type script with owner lock hash
    let udt_owner_lock_hash = [42u8; 32];
    let type_script = context
        .build_script(&simple_udt_out_point, udt_owner_lock_hash.to_vec().into())
        .expect("script");

    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let simple_udt_dep = CellDep::new_builder()
        .out_point(simple_udt_out_point)
        .build();
    let cell_deps = vec![spillman_lock_dep, auth_dep, simple_udt_dep].pack();

    // xUDT amount: 1000 tokens
    let xudt_amount = 1000u128;

    // Create Spillman Lock cell with xUDT
    let input_out_point = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack()) // 1001 CKB
            .lock(lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build(),
        xudt_amount.to_le_bytes().to_vec().into(),
    );

    let since_timestamp = timeout_timestamp + 86400; // 1 day after timeout
    let since_value = Since::from_timestamp(since_timestamp, true).expect("valid since");

    let input = CellInput::new_builder()
        .previous_output(input_out_point.clone())
        .since(since_value.as_u64().pack())
        .build();

    // Refund: user gets all xUDT back
    let outputs = vec![CellOutput::new_builder()
        .capacity(100_000_000_000u64.pack()) // 1000 CKB refund to user, 1 CKB fee
        .lock(user_lock_script.clone())
        .type_(Some(type_script.clone()).pack())
        .build()];

    let outputs_data: Vec<Bytes> = vec![xudt_amount.to_le_bytes().to_vec().into()];

    let success_tx = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        outputs,
        outputs_data,
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let cycles = context
        .verify_tx(&success_tx, 10_000_000)
        .expect("pass verification");
    println!("consume cycles (xUDT refund): {}", cycles);

    // Test: wrong xUDT amount (user doesn't get all) should fail
    let wrong_xudt_amount = 500u128; // only half!
    let wrong_outputs = vec![CellOutput::new_builder()
        .capacity(100_000_000_000u64.pack())
        .lock(user_lock_script.clone())
        .type_(Some(type_script.clone()).pack())
        .build()];

    let wrong_outputs_data: Vec<Bytes> = vec![wrong_xudt_amount.to_le_bytes().to_vec().into()];

    let wrong_tx = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        wrong_outputs,
        wrong_outputs_data,
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&wrong_tx, 10_000_000)
        .expect_err("wrong xUDT amount should fail verification");
    println!("error (wrong xUDT amount): {:?}", err);
}

#[test]
fn test_spillman_lock_timeout_path_with_xudt_co_funding() {
    // Test xUDT channel with co-funding refund
    // User gets all xUDT, merchant gets capacity back with 0 xUDT

    let mut context = Context::default();
    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let simple_udt_bin: Bytes = loader.load_binary("../../deps/simple_udt");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);
    let simple_udt_out_point = context.deploy_cell(simple_udt_bin);

    let mut generator = Generator::new();
    let user_key = generator.gen_keypair();
    let merchant_key = generator.gen_keypair();

    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64; // 2025-01-01 00:00:00 UTC
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");
    let algorithm_id: u8 = 0; // Single-sig
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[algorithm_id],
        &[version],
    ]
    .concat();

    let lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(args))
        .expect("script");

    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let merchant_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(merchant_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    // Create xUDT type script
    let udt_owner_lock_hash = [42u8; 32];
    let type_script = context
        .build_script(
            &simple_udt_out_point.clone(),
            udt_owner_lock_hash.to_vec().into(),
        )
        .expect("script");

    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let simple_udt_dep = CellDep::new_builder()
        .out_point(simple_udt_out_point.clone())
        .build();
    let cell_deps = vec![spillman_lock_dep, auth_dep, simple_udt_dep].pack();

    // Calculate merchant cell's exact occupied capacity with xUDT type script
    let merchant_cell = CellOutput::new_builder()
        .capacity(0u64.pack())
        .lock(merchant_lock_script.clone())
        .type_(Some(type_script.clone()).pack())
        .build();
    let merchant_occupied = merchant_cell
        .occupied_capacity(ckb_testtool::ckb_types::core::Capacity::bytes(16).unwrap()) // 16 bytes for u128
        .unwrap();
    let merchant_capacity_u64: u64 = merchant_occupied.as_u64();

    let xudt_amount = 1000u128;
    let total_capacity = 100_000_000_000u64 + merchant_capacity_u64;

    // Create Spillman Lock cell with xUDT
    let input_out_point = context.create_cell(
        CellOutput::new_builder()
            .capacity(total_capacity.pack())
            .lock(lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build(),
        xudt_amount.to_le_bytes().to_vec().into(),
    );

    let since_timestamp = timeout_timestamp + 86400; // 1 day after timeout
    let since_value = Since::from_timestamp(since_timestamp, true).expect("valid since");

    let input = CellInput::new_builder()
        .previous_output(input_out_point.clone())
        .since(since_value.as_u64().pack())
        .build();

    // Co-funding refund with xUDT:
    // Output 0: User gets all xUDT (1000 tokens)
    // Output 1: Merchant gets capacity back with 0 xUDT
    let outputs = vec![
        CellOutput::new_builder()
            .capacity((total_capacity - merchant_capacity_u64 - 100_000_000).pack())
            .lock(user_lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build(),
        CellOutput::new_builder()
            .capacity(merchant_capacity_u64.pack())
            .lock(merchant_lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build(),
    ];

    let outputs_data: Vec<Bytes> = vec![
        xudt_amount.to_le_bytes().to_vec().into(), // user gets all xUDT
        0u128.to_le_bytes().to_vec().into(),       // merchant gets 0 xUDT
    ];

    let success_tx = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        outputs,
        outputs_data,
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let cycles = context
        .verify_tx(&success_tx, 10_000_000)
        .expect("pass verification");
    println!("consume cycles (xUDT co-funding refund): {}", cycles);

    // Test 1: user output missing type script should fail
    let wrong_outputs_1 = vec![
        CellOutput::new_builder()
            .capacity((total_capacity - merchant_capacity_u64 - 100_000_000).pack())
            .lock(user_lock_script.clone())
            // Missing type script!
            .build(),
        CellOutput::new_builder()
            .capacity(merchant_capacity_u64.pack())
            .lock(merchant_lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build(),
    ];

    let wrong_outputs_data_1: Vec<Bytes> = vec![
        Bytes::new(), // no xUDT data
        0u128.to_le_bytes().to_vec().into(),
    ];

    let wrong_tx_1 = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        wrong_outputs_1,
        wrong_outputs_data_1,
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&wrong_tx_1, 10_000_000)
        .expect_err("user output missing type script should fail");
    println!("error (user missing type script): {:?}", err);

    // Test 2: merchant output missing type script should fail
    let wrong_outputs_2 = vec![
        CellOutput::new_builder()
            .capacity((total_capacity - merchant_capacity_u64 - 100_000_000).pack())
            .lock(user_lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build(),
        CellOutput::new_builder()
            .capacity(merchant_capacity_u64.pack())
            .lock(merchant_lock_script.clone())
            // Missing type script!
            .build(),
    ];

    let wrong_outputs_data_2: Vec<Bytes> = vec![
        xudt_amount.to_le_bytes().to_vec().into(),
        Bytes::new(), // no xUDT data
    ];

    let wrong_tx_2 = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        wrong_outputs_2,
        wrong_outputs_data_2,
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&wrong_tx_2, 10_000_000)
        .expect_err("merchant output missing type script should fail");
    println!("error (merchant missing type script): {:?}", err);

    // Test 3: different type script should fail
    let different_type_script = context
        .build_script(&simple_udt_out_point.clone(), vec![99u8; 32].into())
        .expect("script");

    let wrong_outputs_3 = vec![
        CellOutput::new_builder()
            .capacity((total_capacity - merchant_capacity_u64 - 100_000_000).pack())
            .lock(user_lock_script.clone())
            .type_(Some(different_type_script.clone()).pack()) // Different type script!
            .build(),
        CellOutput::new_builder()
            .capacity(merchant_capacity_u64.pack())
            .lock(merchant_lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build(),
    ];

    let wrong_outputs_data_3: Vec<Bytes> = vec![
        xudt_amount.to_le_bytes().to_vec().into(),
        0u128.to_le_bytes().to_vec().into(),
    ];

    let wrong_tx_3 = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        wrong_outputs_3,
        wrong_outputs_data_3,
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&wrong_tx_3, 10_000_000)
        .expect_err("different type script should fail");
    println!("error (different type script): {:?}", err);

    // Test 4: merchant xUDT amount not zero should fail
    let wrong_outputs_4 = vec![
        CellOutput::new_builder()
            .capacity((total_capacity - merchant_capacity_u64 - 100_000_000).pack())
            .lock(user_lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build(),
        CellOutput::new_builder()
            .capacity(merchant_capacity_u64.pack())
            .lock(merchant_lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build(),
    ];

    let wrong_outputs_data_4: Vec<Bytes> = vec![
        500u128.to_le_bytes().to_vec().into(), // user gets half
        500u128.to_le_bytes().to_vec().into(), // merchant gets half (should be 0!)
    ];

    let wrong_tx_4 = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        wrong_outputs_4,
        wrong_outputs_data_4,
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&wrong_tx_4, 10_000_000)
        .expect_err("merchant xUDT amount not zero should fail");
    println!("error (merchant xUDT not zero): {:?}", err);
}

#[test]
fn test_spillman_lock_commitment_path_with_multisig_merchant() {
    // Test commitment path with 2-of-3 multisig merchant
    let mut context = Context::default();
    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);

    let mut generator = Generator::new();
    let user_key = generator.gen_keypair();

    // Generate 3 merchant keys for 2-of-3 multisig
    let merchant_key1 = generator.gen_keypair();
    let merchant_key2 = generator.gen_keypair();
    let merchant_key3 = generator.gen_keypair();

    // Build multisig script: S | R | M | N | PubKeyHash1 | PubKeyHash2 | PubKeyHash3
    let merchant_pubkey_hash1 = blake160(&merchant_key1.1.serialize());
    let merchant_pubkey_hash2 = blake160(&merchant_key2.1.serialize());
    let merchant_pubkey_hash3 = blake160(&merchant_key3.1.serialize());

    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64; // 2025-01-01 00:00:00 UTC
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");
    let algorithm_id: u8 = 6; // Multi-sig
    let version: u8 = 0;

    // Multisig config: S=0, R=0, M=2, N=3
    let multisig_config = [
        &[0u8][..],                     // S: format version
        &[0u8][..],                     // R: first_n (0 means any 2 of 3)
        &[2u8][..],                     // M: threshold (need 2 signatures)
        &[3u8][..],                     // N: total pubkeys (3 pubkeys)
        merchant_pubkey_hash1.as_ref(), // PubKeyHash1
        merchant_pubkey_hash2.as_ref(), // PubKeyHash2
        merchant_pubkey_hash3.as_ref(), // PubKeyHash3
    ]
    .concat();

    // Calculate blake160(multisig_config) for args
    let merchant_lock_arg = &blake2b_256(&multisig_config)[0..20];

    // Build args: merchant_lock_arg(20) + user(20) + timeout(8) + algorithm_id(1) + version(1) = 50 bytes
    let args = [
        merchant_lock_arg,
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[algorithm_id],
        &[version],
    ]
    .concat();

    // Verify args length: 20 + 20 + 8 + 1 + 1 = 50 bytes
    assert_eq!(args.len(), 50);

    let lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(args))
        .expect("script");

    // User lock script (single-sig)
    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    // Merchant lock script (multisig with blake160(multisig_config))
    let merchant_lock_script = Script::new_builder()
        .code_hash(SECP256K1_MULTISIG_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(merchant_lock_arg.to_vec()).pack())
        .build();

    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let cell_deps = vec![spillman_lock_dep, auth_dep].pack();

    let input_out_point = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack()) // 1001 CKB
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );

    let input = CellInput::new_builder()
        .previous_output(input_out_point)
        .build();

    let outputs = vec![
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack()) // 500 CKB
            .lock(user_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack()) // 500 CKB
            .lock(merchant_lock_script)
            .build(),
    ];

    let outputs_data = vec![Bytes::new(); 2];

    // Build and sign with multisig (use merchant_key1 and merchant_key2)
    let success_tx = build_and_sign_tx_multisig(
        cell_deps.clone(),
        input.clone(),
        outputs,
        outputs_data,
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &[&merchant_key1, &merchant_key2], // Use 2 of 3 keys
        &multisig_config,                  // Pass multisig config
    );

    let cycles = context
        .verify_tx(&success_tx, 10_000_000)
        .expect("pass verification");
    println!("consume cycles (multisig commitment): {}", cycles);

    // Test: using only 1 signature should fail (need M=2 signatures)
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps.clone())
        .input(input.clone())
        .outputs(success_tx.outputs())
        .outputs_data(success_tx.outputs_data())
        .build();

    let message = compute_signing_message(&tx);
    let user_signature = user_key
        .0
        .sign_recoverable(&message.into())
        .unwrap()
        .serialize();
    let merchant_signature1 = merchant_key1
        .0
        .sign_recoverable(&message.into())
        .unwrap()
        .serialize();

    // Only 1 merchant signature (should fail, need 2)
    let insufficient_witness = [
        &EMPTY_WITNESS_ARGS[..],
        &[UNLOCK_TYPE_COMMITMENT][..],
        &multisig_config[..],     // Must include multisig_config
        &merchant_signature1[..], // Only 1 signature (need 2)!
        &user_signature[..],
    ]
    .concat();

    let fail_tx = tx
        .as_advanced_builder()
        .witness(insufficient_witness.pack())
        .build();

    let err = context
        .verify_tx(&fail_tx, 10_000_000)
        .expect_err("insufficient signatures should fail");
    println!("error (insufficient signatures): {:?}", err);
}

#[test]
fn test_spillman_lock_timeout_path_with_multisig_merchant() {
    let mut context = Context::default();

    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);

    // Generate 3 merchant keys for 2-of-3 multisig
    let merchant_key1 = Generator::random_keypair();
    let merchant_key2 = Generator::random_keypair();
    let merchant_key3 = Generator::random_keypair();
    let user_key = Generator::random_keypair();

    let merchant_pubkey_hash1 = blake160(&merchant_key1.1.serialize());
    let merchant_pubkey_hash2 = blake160(&merchant_key2.1.serialize());
    let merchant_pubkey_hash3 = blake160(&merchant_key3.1.serialize());

    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64; // 2025-01-01 00:00:00 UTC
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");
    let algorithm_id: u8 = 6; // Multi-sig
    let version: u8 = 0;

    // Multisig config: S=0, R=0, M=2, N=3
    let multisig_config = [
        &[0u8][..],                     // S: format version
        &[0u8][..],                     // R: first_n (0 means any 2 of 3)
        &[2u8][..],                     // M: threshold (need 2 signatures)
        &[3u8][..],                     // N: total pubkeys (3 pubkeys)
        merchant_pubkey_hash1.as_ref(), // PubKeyHash1
        merchant_pubkey_hash2.as_ref(), // PubKeyHash2
        merchant_pubkey_hash3.as_ref(), // PubKeyHash3
    ]
    .concat();

    // Calculate blake160(multisig_config) for args
    let merchant_lock_arg = &blake2b_256(&multisig_config)[0..20];

    // Build args: merchant_lock_arg(20) + user(20) + timeout(8) + algorithm_id(1) + version(1) = 50 bytes
    let args = [
        merchant_lock_arg,
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[algorithm_id],
        &[version],
    ]
    .concat();

    let lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(args))
        .expect("script");

    // User lock script (single-sig)
    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let cell_deps = vec![spillman_lock_dep, auth_dep].pack();

    let input_out_point = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack()) // 1001 CKB
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );

    let since_timestamp = timeout_timestamp + 86400; // 1 day after timeout
    let since_value = Since::from_timestamp(since_timestamp, true).expect("valid since");

    let input = CellInput::new_builder()
        .previous_output(input_out_point.clone())
        .since(since_value.as_u64().pack())
        .build();

    // Refund: all funds go back to user
    let outputs = vec![CellOutput::new_builder()
        .capacity(100_000_000_000u64.pack()) // 1000 CKB (1 CKB fee)
        .lock(user_lock_script.clone())
        .build()];

    let outputs_data = vec![Bytes::new(); 1];

    // Build and sign with multisig (use merchant_key1 and merchant_key2)
    let success_tx = build_and_sign_tx_multisig(
        cell_deps.clone(),
        input.clone(),
        outputs,
        outputs_data,
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &[&merchant_key1, &merchant_key2], // Use 2 of 3 keys
        &multisig_config,                  // Pass multisig config
    );

    let cycles = context
        .verify_tx(&success_tx, 10_000_000)
        .expect("pass verification");
    println!("consume cycles (multisig timeout): {}", cycles);

    // Test: timeout not reached should fail
    let input_without_since = CellInput::new_builder()
        .previous_output(input_out_point.clone())
        .since(0u64.pack()) // No timeout set
        .build();

    let fail_tx = TransactionBuilder::default()
        .cell_deps(cell_deps.clone())
        .input(input_without_since)
        .outputs(success_tx.outputs())
        .outputs_data(success_tx.outputs_data())
        .witness(success_tx.witnesses().get(0).unwrap())
        .build();

    let err = context
        .verify_tx(&fail_tx, 10_000_000)
        .expect_err("timeout not reached should fail");
    println!("error (timeout not reached): {:?}", err);
}

#[test]
fn test_spillman_lock_multisig_error_scenarios() {
    let mut context = Context::default();

    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);

    // Generate 3 merchant keys for 2-of-3 multisig
    let merchant_key1 = Generator::random_keypair();
    let merchant_key2 = Generator::random_keypair();
    let merchant_key3 = Generator::random_keypair();
    let user_key = Generator::random_keypair();

    let merchant_pubkey_hash1 = blake160(&merchant_key1.1.serialize());
    let merchant_pubkey_hash2 = blake160(&merchant_key2.1.serialize());
    let merchant_pubkey_hash3 = blake160(&merchant_key3.1.serialize());

    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64; // 2025-01-01 00:00:00 UTC
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");
    let algorithm_id: u8 = 6; // Multi-sig
    let version: u8 = 0;

    // Multisig config: S=0, R=0, M=2, N=3
    let multisig_config = [
        &[0u8][..],                     // S: format version
        &[0u8][..],                     // R: first_n (0 means any 2 of 3)
        &[2u8][..],                     // M: threshold (need 2 signatures)
        &[3u8][..],                     // N: total pubkeys (3 pubkeys)
        merchant_pubkey_hash1.as_ref(), // PubKeyHash1
        merchant_pubkey_hash2.as_ref(), // PubKeyHash2
        merchant_pubkey_hash3.as_ref(), // PubKeyHash3
    ]
    .concat();

    // Calculate blake160(multisig_config) for args
    let merchant_lock_arg = &blake2b_256(&multisig_config)[0..20];

    // Build args
    let args = [
        merchant_lock_arg,
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[algorithm_id],
        &[version],
    ]
    .concat();

    let lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(args))
        .expect("script");

    // User lock script (single-sig)
    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    // Merchant lock script (multisig with blake160(multisig_config))
    let merchant_lock_script = Script::new_builder()
        .code_hash(SECP256K1_MULTISIG_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(merchant_lock_arg.to_vec()).pack())
        .build();

    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let cell_deps = vec![spillman_lock_dep, auth_dep].pack();

    let input_out_point = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack())
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );

    let input = CellInput::new_builder()
        .previous_output(input_out_point.clone())
        .build();

    // Test 1: Wrong merchant output - using single-sig code_hash instead of multisig
    let wrong_merchant_lock = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack()) // Wrong! Should be SECP256K1_MULTISIG_CODE_HASH
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(merchant_lock_arg.to_vec()).pack())
        .build();

    let outputs = vec![
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(user_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(wrong_merchant_lock) // Wrong lock!
            .build(),
    ];

    let outputs_data = vec![Bytes::new(); 2];

    let fail_tx = build_and_sign_tx_multisig(
        cell_deps.clone(),
        input.clone(),
        outputs.clone(),
        outputs_data.clone(),
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &[&merchant_key1, &merchant_key2],
        &multisig_config,
    );

    let err = context
        .verify_tx(&fail_tx, 10_000_000)
        .expect_err("wrong merchant output code_hash should fail");
    println!("error (wrong code_hash): {:?}", err);

    // Test 2: Mismatched multisig_config hash
    // Create a different multisig config but use it with the original lock_arg
    let wrong_multisig_config = [
        &[0u8][..],
        &[0u8][..],
        &[1u8][..], // M=1 instead of 2
        &[2u8][..], // N=2 instead of 3
        merchant_pubkey_hash1.as_ref(),
        merchant_pubkey_hash2.as_ref(),
    ]
    .concat();

    let correct_outputs = vec![
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(user_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(merchant_lock_script.clone())
            .build(),
    ];

    let fail_tx2 = build_and_sign_tx_multisig(
        cell_deps.clone(),
        input.clone(),
        correct_outputs,
        outputs_data,
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &[&merchant_key1],      // Only 1 signature for the wrong config
        &wrong_multisig_config, // Wrong config! Hash doesn't match args
    );

    let err2 = context
        .verify_tx(&fail_tx2, 10_000_000)
        .expect_err("mismatched multisig_config hash should fail");
    println!("error (mismatched config): {:?}", err2);
}

// Helper function to build and sign transaction with multisig merchant
fn build_and_sign_tx_multisig(
    cell_deps: CellDepVec,
    input: CellInput,
    outputs: Vec<CellOutput>,
    outputs_data: Vec<Bytes>,
    unlock_type: u8,
    user_key: &(
        ckb_testtool::ckb_crypto::secp::Privkey,
        ckb_testtool::ckb_crypto::secp::Pubkey,
    ),
    merchant_keys: &[&(
        ckb_testtool::ckb_crypto::secp::Privkey,
        ckb_testtool::ckb_crypto::secp::Pubkey,
    )],
    multisig_config: &[u8],
) -> TransactionView {
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .input(input)
        .outputs(outputs)
        .outputs_data(outputs_data.pack())
        .build();

    let message = compute_signing_message(&tx);

    // Collect all merchant signatures
    let mut merchant_signatures = Vec::new();
    for key in merchant_keys {
        let signature = key.0.sign_recoverable(&message.into()).unwrap().serialize();
        merchant_signatures.extend_from_slice(&signature);
    }

    let user_signature = user_key
        .0
        .sign_recoverable(&message.into())
        .unwrap()
        .serialize();

    // Witness format for multisig: empty_witness_args + unlock_type + multisig_config + merchant_signatures + user_signature
    let witness = [
        &EMPTY_WITNESS_ARGS[..],
        &[unlock_type][..],
        multisig_config,          // Full multisig config (4+N*20 bytes)
        &merchant_signatures[..], // M signatures (M * 65 bytes)
        &user_signature[..],      // 1 user signature (65 bytes)
    ]
    .concat();

    tx.as_advanced_builder().witness(witness.pack()).build()
}

// Helper function to build and sign transaction
fn build_and_sign_tx(
    cell_deps: CellDepVec,
    input: CellInput,
    outputs: Vec<CellOutput>,
    outputs_data: Vec<Bytes>,
    unlock_type: u8,
    user_key: &(
        ckb_testtool::ckb_crypto::secp::Privkey,
        ckb_testtool::ckb_crypto::secp::Pubkey,
    ),
    merchant_key: &(
        ckb_testtool::ckb_crypto::secp::Privkey,
        ckb_testtool::ckb_crypto::secp::Pubkey,
    ),
) -> TransactionView {
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .input(input)
        .outputs(outputs)
        .outputs_data(outputs_data.pack())
        .build();

    let message = compute_signing_message(&tx);
    let user_signature = user_key
        .0
        .sign_recoverable(&message.into())
        .unwrap()
        .serialize();
    let merchant_signature = merchant_key
        .0
        .sign_recoverable(&message.into())
        .unwrap()
        .serialize();
    let witness = [
        &EMPTY_WITNESS_ARGS[..],
        &[unlock_type][..],
        &merchant_signature[..],
        &user_signature[..],
    ]
    .concat();

    tx.as_advanced_builder().witness(witness.pack()).build()
}

fn compute_signing_message(tx: &TransactionView) -> [u8; 32] {
    let tx = tx
        .data()
        .raw()
        .as_builder()
        .cell_deps(Default::default())
        .build();
    blake2b_256(tx.as_slice())
}

/// Test timeout path with timestamp-based since (instead of epoch-based)
/// This tests the recommendation to use timestamp for better UX
#[test]
fn test_spillman_lock_timeout_path_with_timestamp() {
    // deploy contract
    let mut context = Context::default();
    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);

    let mut generator = Generator::new();
    let user_key = generator.gen_keypair();
    let merchant_key = generator.gen_keypair();

    // Use timestamp instead of epoch
    // Simulating "7 days from now" timeout
    // In real scenario: now + 7 * 24 * 60 * 60
    // For testing: use a fixed timestamp
    let timeout_timestamp = 1735689600u64; // 2025-01-01 00:00:00 UTC
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");

    // Build SpillmanLockArgs with timestamp
    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let algorithm_id: u8 = 0; // Single-sig
    let version: u8 = 0;

    let spillman_lock_args = [
        merchant_pubkey_hash.as_ref(),         // 0..20: merchant lock arg
        user_pubkey_hash.as_ref(),             // 20..40: user pubkey hash
        &timeout_since.as_u64().to_le_bytes(), // 40..48: timeout timestamp (little-endian)
        &[algorithm_id],                       // 48: algorithm_id
        &[version],                            // 49: version
    ]
    .concat();

    // Create merchant lock script (secp256k1_blake160_sighash_all)
    let merchant_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(merchant_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    println!(
        "\n=== Timestamp-based Timeout Test ===\n  Timeout: {} (Unix timestamp)\n  Since value: 0x{:016x}",
        timeout_timestamp,
        timeout_since.as_u64()
    );

    let spillman_lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(spillman_lock_args))
        .expect("script");

    // prepare cells
    let cell_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_cell_dep = CellDep::new_builder()
        .out_point(auth_out_point.clone())
        .build();

    let input_out_point = context.create_cell(
        CellOutput::new_builder()
            .capacity(500_0000_0000u64.pack()) // 500 CKB
            .lock(spillman_lock_script.clone())
            .build(),
        Bytes::new(),
    );

    // Build refund transaction with timestamp since
    let input = CellInput::new_builder()
        .previous_output(input_out_point.clone())
        .since(timeout_since.as_u64().pack()) // Use timestamp since!
        .build();

    // Calculate capacities
    let total_capacity = 500_0000_0000u64; // 500 CKB
    let merchant_lock_cell_capacity = {
        use ckb_testtool::ckb_types::core::Capacity;
        CellOutput::new_builder()
            .capacity(0u64.pack())
            .lock(merchant_lock_script.clone())
            .build()
            .occupied_capacity(Capacity::bytes(0).unwrap())
            .unwrap()
            .as_u64()
    };

    let outputs = vec![
        // User output (gets most of the funds)
        CellOutput::new_builder()
            .capacity((total_capacity - merchant_lock_cell_capacity).pack())
            .lock(
                Script::new_builder()
                    .code_hash(SECP256K1_CODE_HASH.pack())
                    .hash_type(ScriptHashType::Type.into())
                    .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
                    .build(),
            )
            .build(),
        // Merchant output (minimal capacity)
        CellOutput::new_builder()
            .capacity(merchant_lock_cell_capacity.pack())
            .lock(merchant_lock_script.clone())
            .build(),
    ];

    let outputs_data: Vec<Bytes> = vec![Bytes::new(), Bytes::new()];

    // Prepare cell_deps
    let cell_deps = CellDepVec::new_builder()
        .push(cell_dep.clone())
        .push(auth_cell_dep.clone())
        .build();

    // Build and sign the transaction
    let success_tx = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        outputs.clone(),
        outputs_data.clone(),
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    println!("  Testing successful unlock with timestamp since >= timeout...");
    let cycles = context
        .verify_tx(&success_tx, 10_000_000)
        .expect("timestamp since should pass when >= timeout");
    println!("  ✓ Success! Cycles consumed: {}", cycles);

    // Test: timeout not reached (using earlier timestamp)
    println!("\n  Testing early unlock (should fail)...");
    let early_timestamp = timeout_timestamp - 3600; // 1 hour before timeout
    let early_since = Since::from_timestamp(early_timestamp, true).unwrap();
    let early_input = CellInput::new_builder()
        .previous_output(input_out_point.clone())
        .since(early_since.as_u64().pack())
        .build();

    let early_tx = build_and_sign_tx(
        cell_deps.clone(),
        early_input,
        outputs.clone(),
        outputs_data.clone(),
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&early_tx, 10_000_000)
        .expect_err("early timestamp should fail");
    println!("  ✓ Correctly rejected! Error: {:?}", err);

    // Test: incomparable types (timestamp vs epoch)
    println!("\n  Testing incomparable types (timestamp vs epoch)...");
    let epoch_since = Since::from_epoch(EpochNumberWithFraction::new(42, 0, 1), true);
    let incomparable_input = CellInput::new_builder()
        .previous_output(input_out_point.clone())
        .since(epoch_since.as_u64().pack())
        .build();

    let incomparable_tx = build_and_sign_tx(
        cell_deps.clone(),
        incomparable_input,
        outputs.clone(),
        outputs_data.clone(),
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&incomparable_tx, 10_000_000)
        .expect_err("timestamp timeout vs epoch since should fail");
    println!(
        "  ✓ Correctly rejected incomparable types! Error: {:?}",
        err
    );

    // Test: timestamp in the future (should succeed)
    println!("\n  Testing future timestamp (should succeed)...");
    let future_timestamp = timeout_timestamp + 86400; // 1 day after timeout
    let future_since = Since::from_timestamp(future_timestamp, true).unwrap();
    let future_input = CellInput::new_builder()
        .previous_output(input_out_point)
        .since(future_since.as_u64().pack())
        .build();

    let future_tx = build_and_sign_tx(
        cell_deps,
        future_input,
        outputs,
        outputs_data,
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let cycles = context
        .verify_tx(&future_tx, 10_000_000)
        .expect("future timestamp should pass");
    println!("  ✓ Success! Cycles consumed: {}", cycles);

    println!("\n=== All Timestamp Since Tests Passed! ===\n");
}

#[test]
fn test_spillman_lock_commitment_path_with_xudt() {
    // Test commitment path with xUDT: merchant receives xUDT payment
    let mut context = Context::default();
    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let simple_udt_bin: Bytes = loader.load_binary("../../deps/simple_udt");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);
    let simple_udt_out_point = context.deploy_cell(simple_udt_bin);

    let mut generator = Generator::new();
    let user_key = generator.gen_keypair();
    let merchant_key = generator.gen_keypair();

    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64;
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");
    let algorithm_id: u8 = 0;
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[algorithm_id],
        &[version],
    ]
    .concat();

    let lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(args))
        .expect("script");

    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let merchant_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(merchant_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    // Create xUDT type script
    let udt_owner_lock_hash = [42u8; 32];
    let type_script = context
        .build_script(&simple_udt_out_point, udt_owner_lock_hash.to_vec().into())
        .expect("script");

    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let simple_udt_dep = CellDep::new_builder()
        .out_point(simple_udt_out_point)
        .build();
    let cell_deps = vec![spillman_lock_dep, auth_dep, simple_udt_dep].pack();

    let xudt_amount = 1000u128;

    let input_out_point = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack())
            .lock(lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build(),
        xudt_amount.to_le_bytes().to_vec().into(),
    );

    let input = CellInput::new_builder()
        .previous_output(input_out_point)
        .build();

    // Commitment: user gets 300 xUDT, merchant gets 700 xUDT
    let outputs = vec![
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(user_lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build(),
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(merchant_lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build(),
    ];

    let outputs_data: Vec<Bytes> = vec![
        300u128.to_le_bytes().to_vec().into(),
        700u128.to_le_bytes().to_vec().into(),
    ];

    let success_tx = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        outputs,
        outputs_data,
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &merchant_key,
    );

    let cycles = context
        .verify_tx(&success_tx, 10_000_000)
        .expect("pass verification");
    println!("consume cycles (commitment with xUDT): {}", cycles);

    // Test: merchant xUDT amount is 0 should fail
    let wrong_outputs = vec![
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(user_lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build(),
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(merchant_lock_script.clone()) // Use correct merchant lock!
            .type_(Some(type_script.clone()).pack())
            .build(),
    ];

    let wrong_outputs_data: Vec<Bytes> = vec![
        1000u128.to_le_bytes().to_vec().into(),
        0u128.to_le_bytes().to_vec().into(), // merchant gets 0 xUDT (should fail!)
    ];

    let wrong_tx = build_and_sign_tx(
        cell_deps,
        input,
        wrong_outputs,
        wrong_outputs_data,
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&wrong_tx, 10_000_000)
        .expect_err("merchant xUDT amount 0 should fail");
    println!("error (merchant xUDT is 0): {:?}", err);
}

#[test]
fn test_spillman_lock_commitment_path_output_structure_errors() {
    // Test various output structure errors in commitment path
    let mut context = Context::default();
    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);

    let mut generator = Generator::new();
    let user_key = generator.gen_keypair();
    let merchant_key = generator.gen_keypair();

    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64;
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");
    let algorithm_id: u8 = 0;
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[algorithm_id],
        &[version],
    ]
    .concat();

    let lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(args))
        .expect("script");

    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let merchant_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(merchant_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let cell_deps = vec![spillman_lock_dep, auth_dep].pack();

    let input_out_point = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack())
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );

    let input = CellInput::new_builder()
        .previous_output(input_out_point)
        .build();

    // Test 1: Only 1 output (should fail, need exactly 2)
    let outputs_1 = vec![CellOutput::new_builder()
        .capacity(100_000_000_000u64.pack())
        .lock(user_lock_script.clone())
        .build()];

    let fail_tx_1 = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        outputs_1,
        vec![Bytes::new()],
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&fail_tx_1, 10_000_000)
        .expect_err("commitment with 1 output should fail");
    println!("error (1 output): {:?}", err);

    // Test 2: 3 outputs (should fail, need exactly 2)
    let outputs_3 = vec![
        CellOutput::new_builder()
            .capacity(33_333_333_333u64.pack())
            .lock(user_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(33_333_333_333u64.pack())
            .lock(merchant_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(33_333_333_333u64.pack())
            .lock(user_lock_script.clone())
            .build(),
    ];

    let fail_tx_3 = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        outputs_3,
        vec![Bytes::new(); 3],
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&fail_tx_3, 10_000_000)
        .expect_err("commitment with 3 outputs should fail");
    println!("error (3 outputs): {:?}", err);

    // Test 3: Output 0 is not user address (merchant instead)
    let outputs_wrong_user = vec![
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(merchant_lock_script.clone()) // Wrong! Should be user
            .build(),
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(merchant_lock_script.clone())
            .build(),
    ];

    let fail_tx_wrong_user = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        outputs_wrong_user,
        vec![Bytes::new(); 2],
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&fail_tx_wrong_user, 10_000_000)
        .expect_err("Output 0 not user address should fail");
    println!("error (Output 0 wrong): {:?}", err);

    // Test 4: Output 1 is not merchant address (user instead)
    let outputs_wrong_merchant = vec![
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(user_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(user_lock_script.clone()) // Wrong! Should be merchant
            .build(),
    ];

    let fail_tx_wrong_merchant = build_and_sign_tx(
        cell_deps.clone(),
        input.clone(),
        outputs_wrong_merchant,
        vec![Bytes::new(); 2],
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&fail_tx_wrong_merchant, 10_000_000)
        .expect_err("Output 1 not merchant address should fail");
    println!("error (Output 1 wrong): {:?}", err);
}

#[test]
fn test_spillman_lock_ommitment_path_witness_format_errors() {
    // Test various witness format errors
    let mut context = Context::default();
    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);

    let mut generator = Generator::new();
    let user_key = generator.gen_keypair();
    let merchant_key = generator.gen_keypair();

    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64;
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");
    let algorithm_id: u8 = 0;
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[algorithm_id],
        &[version],
    ]
    .concat();

    let lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(args))
        .expect("script");

    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let merchant_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(merchant_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let cell_deps = vec![spillman_lock_dep, auth_dep].pack();

    let input_out_point = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack())
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );

    let input = CellInput::new_builder()
        .previous_output(input_out_point)
        .build();

    let outputs = vec![
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(user_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(merchant_lock_script)
            .build(),
    ];

    let outputs_data = vec![Bytes::new(); 2];

    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps.clone())
        .input(input.clone())
        .outputs(outputs.clone())
        .outputs_data(outputs_data.clone().pack())
        .build();

    // Test 1: Witness too short (less than min length)
    let short_witness = vec![0u8; 10]; // Way too short
    let fail_tx_1 = tx
        .as_advanced_builder()
        .witness(short_witness.pack())
        .build();

    let err = context
        .verify_tx(&fail_tx_1, 10_000_000)
        .expect_err("short witness should fail");
    println!("error (witness too short): {:?}", err);

    // Test 2: Wrong empty_witness_args prefix
    let message = compute_signing_message(&tx);
    let user_signature = user_key
        .0
        .sign_recoverable(&message.into())
        .unwrap()
        .serialize();
    let merchant_signature = merchant_key
        .0
        .sign_recoverable(&message.into())
        .unwrap()
        .serialize();

    let wrong_empty_witness_args = [99u8; 16]; // Wrong prefix
    let wrong_witness = [
        &wrong_empty_witness_args[..],
        &[UNLOCK_TYPE_COMMITMENT][..],
        &merchant_signature[..],
        &user_signature[..],
    ]
    .concat();

    let fail_tx_2 = tx
        .as_advanced_builder()
        .witness(wrong_witness.pack())
        .build();

    let err = context
        .verify_tx(&fail_tx_2, 10_000_000)
        .expect_err("wrong empty_witness_args should fail");
    println!("error (wrong empty_witness_args): {:?}", err);
}

#[test]
fn test_spillman_lock_ommitment_path_args_validation_errors() {
    // Test various args validation errors
    let mut context = Context::default();
    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);

    let mut generator = Generator::new();
    let user_key = generator.gen_keypair();
    let merchant_key = generator.gen_keypair();

    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64;
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");

    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let merchant_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(merchant_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point.clone())
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let cell_deps = vec![spillman_lock_dep, auth_dep].pack();

    // Test 1: Args too short (not 50 bytes)
    let short_args = vec![0u8; 20]; // Only 20 bytes
    let lock_script_1 = context
        .build_script(&spillman_lock_out_point, Bytes::from(short_args))
        .expect("script");

    let input_out_point_1 = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack())
            .lock(lock_script_1.clone())
            .build(),
        Bytes::new(),
    );

    let input_1 = CellInput::new_builder()
        .previous_output(input_out_point_1)
        .build();

    let outputs = vec![
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(user_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(merchant_lock_script.clone())
            .build(),
    ];

    let fail_tx_1 = build_and_sign_tx(
        cell_deps.clone(),
        input_1,
        outputs.clone(),
        vec![Bytes::new(); 2],
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&fail_tx_1, 10_000_000)
        .expect_err("args too short should fail");
    println!("error (args too short): {:?}", err);

    // Test 2: Args too long
    let long_args = vec![0u8; 100]; // 100 bytes
    let lock_script_2 = context
        .build_script(&spillman_lock_out_point, Bytes::from(long_args))
        .expect("script");

    let input_out_point_2 = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack())
            .lock(lock_script_2.clone())
            .build(),
        Bytes::new(),
    );

    let input_2 = CellInput::new_builder()
        .previous_output(input_out_point_2)
        .build();

    let fail_tx_2 = build_and_sign_tx(
        cell_deps.clone(),
        input_2,
        outputs.clone(),
        vec![Bytes::new(); 2],
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&fail_tx_2, 10_000_000)
        .expect_err("args too long should fail");
    println!("error (args too long): {:?}", err);

    // Test 3: Unsupported version (not 0)
    let bad_version: u8 = 1; // Wrong version
    let args_bad_version = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[0u8][..], // algorithm_id = 0
        &[bad_version][..],
    ]
    .concat();

    let lock_script_3 = context
        .build_script(&spillman_lock_out_point, Bytes::from(args_bad_version))
        .expect("script");

    let input_out_point_3 = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack())
            .lock(lock_script_3.clone())
            .build(),
        Bytes::new(),
    );

    let input_3 = CellInput::new_builder()
        .previous_output(input_out_point_3)
        .build();

    let fail_tx_3 = build_and_sign_tx(
        cell_deps.clone(),
        input_3,
        outputs.clone(),
        vec![Bytes::new(); 2],
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&fail_tx_3, 10_000_000)
        .expect_err("unsupported version should fail");
    println!("error (unsupported version): {:?}", err);

    // Test 4: Invalid algorithm_id
    let invalid_algorithm_id: u8 = 99; // Not 0, 6, or 7
    let args_bad_algorithm = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[invalid_algorithm_id][..],
        &[0u8][..], // version = 0
    ]
    .concat();

    let lock_script_4 = context
        .build_script(&spillman_lock_out_point, Bytes::from(args_bad_algorithm))
        .expect("script");

    let input_out_point_4 = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack())
            .lock(lock_script_4.clone())
            .build(),
        Bytes::new(),
    );

    let input_4 = CellInput::new_builder()
        .previous_output(input_out_point_4)
        .build();

    let fail_tx_4 = build_and_sign_tx(
        cell_deps,
        input_4,
        outputs,
        vec![Bytes::new(); 2],
        UNLOCK_TYPE_COMMITMENT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&fail_tx_4, 10_000_000)
        .expect_err("invalid algorithm_id should fail");
    println!("error (invalid algorithm_id): {:?}", err);
}

#[test]
fn test_spillman_lock_commitment_path_multiple_inputs() {
    // Test multiple inputs (should fail with Error::MultipleInputs)
    let mut context = Context::default();
    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);

    let mut generator = Generator::new();
    let user_key = generator.gen_keypair();
    let merchant_key = generator.gen_keypair();

    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64;
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");
    let algorithm_id: u8 = 0;
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[algorithm_id],
        &[version],
    ]
    .concat();

    let lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(args))
        .expect("script");

    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let merchant_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(merchant_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let cell_deps = vec![spillman_lock_dep, auth_dep].pack();

    // Create 2 inputs with the same lock script
    let input_out_point_1 = context.create_cell(
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );

    let input_out_point_2 = context.create_cell(
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );

    let input_1 = CellInput::new_builder()
        .previous_output(input_out_point_1)
        .build();

    let input_2 = CellInput::new_builder()
        .previous_output(input_out_point_2)
        .build();

    let outputs = vec![
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(user_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(50_000_000_000u64.pack())
            .lock(merchant_lock_script)
            .build(),
    ];

    let outputs_data = vec![Bytes::new(); 2];

    // Build transaction with 2 inputs
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .inputs(vec![input_1, input_2]) // 2 inputs!
        .outputs(outputs)
        .outputs_data(outputs_data.pack())
        .build();

    let message = compute_signing_message(&tx);
    let user_signature = user_key
        .0
        .sign_recoverable(&message.into())
        .unwrap()
        .serialize();
    let merchant_signature = merchant_key
        .0
        .sign_recoverable(&message.into())
        .unwrap()
        .serialize();

    let witness = [
        &EMPTY_WITNESS_ARGS[..],
        &[UNLOCK_TYPE_COMMITMENT][..],
        &merchant_signature[..],
        &user_signature[..],
    ]
    .concat();

    let fail_tx = tx
        .as_advanced_builder()
        .witness(witness.pack())
        .witness(Bytes::new().pack()) // witness for 2nd input
        .build();

    let err = context
        .verify_tx(&fail_tx, 10_000_000)
        .expect_err("multiple inputs should fail");
    println!("error (multiple inputs): {:?}", err);
}

#[test]
fn test_spillman_lock_timeout_path_too_many_outputs() {
    // Test timeout path with 3+ outputs (should fail)
    let mut context = Context::default();
    let loader = Loader::default();
    let spillman_lock_bin: Bytes = loader.load_binary("spillman-lock");
    let auth_bin: Bytes = loader.load_binary("../../deps/auth");
    let spillman_lock_out_point = context.deploy_cell(spillman_lock_bin);
    let auth_out_point = context.deploy_cell(auth_bin);

    let mut generator = Generator::new();
    let user_key = generator.gen_keypair();
    let merchant_key = generator.gen_keypair();

    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_timestamp = 1735689600u64;
    let timeout_since =
        Since::from_timestamp(timeout_timestamp, true).expect("valid timestamp since");
    let algorithm_id: u8 = 0;
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_since.as_u64().to_le_bytes(),
        &[algorithm_id],
        &[version],
    ]
    .concat();

    let lock_script = context
        .build_script(&spillman_lock_out_point, Bytes::from(args))
        .expect("script");

    let user_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(user_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let merchant_lock_script = Script::new_builder()
        .code_hash(SECP256K1_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(merchant_pubkey_hash.as_ref().to_vec()).pack())
        .build();

    let spillman_lock_dep = CellDep::new_builder()
        .out_point(spillman_lock_out_point)
        .build();
    let auth_dep = CellDep::new_builder().out_point(auth_out_point).build();
    let cell_deps = vec![spillman_lock_dep, auth_dep].pack();

    let input_out_point = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_100_000_000u64.pack())
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );

    let since_timestamp = timeout_timestamp + 86400;
    let since_value = Since::from_timestamp(since_timestamp, true).expect("valid since");

    let input = CellInput::new_builder()
        .previous_output(input_out_point)
        .since(since_value.as_u64().pack())
        .build();

    // 3 outputs (should fail, max is 2)
    let outputs = vec![
        CellOutput::new_builder()
            .capacity(33_333_333_333u64.pack())
            .lock(user_lock_script.clone())
            .build(),
        CellOutput::new_builder()
            .capacity(33_333_333_333u64.pack())
            .lock(merchant_lock_script)
            .build(),
        CellOutput::new_builder()
            .capacity(33_333_333_333u64.pack())
            .lock(user_lock_script.clone())
            .build(),
    ];

    let fail_tx = build_and_sign_tx(
        cell_deps,
        input,
        outputs,
        vec![Bytes::new(); 3],
        UNLOCK_TYPE_TIMEOUT,
        &user_key,
        &merchant_key,
    );

    let err = context
        .verify_tx(&fail_tx, 10_000_000)
        .expect_err("timeout with 3 outputs should fail");
    println!("error (3 outputs in timeout): {:?}", err);
}
