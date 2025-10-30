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
    //     timeout_epoch: [u8; 8],          // 40..48 (u64 little-endian)
    //     version: u8,                     // 48
    // }
    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_epoch = Since::from_epoch(EpochNumberWithFraction::new(42, 0, 1), false); // 7 days
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),         // 0..20: merchant pubkey hash
        user_pubkey_hash.as_ref(),             // 20..40: user pubkey hash
        &timeout_epoch.as_u64().to_le_bytes(), // 40..48: timeout epoch (little-endian)
        &[version],                            // 48: version
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

    // Build SpillmanLockArgs with timeout epoch
    let merchant_pubkey_hash = blake160(&merchant_key.1.serialize());
    let user_pubkey_hash = blake160(&user_key.1.serialize());
    let timeout_epoch = Since::from_epoch(EpochNumberWithFraction::new(42, 0, 1), false);
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_epoch.as_u64().to_le_bytes(),
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
    // Set since to a value greater than timeout_epoch to simulate timeout
    let since_value = Since::from_epoch(EpochNumberWithFraction::new(50, 0, 1), false);

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
    let early_since = Since::from_epoch(EpochNumberWithFraction::new(10, 0, 1), false);
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
    let timeout_epoch = Since::from_epoch(EpochNumberWithFraction::new(42, 0, 1), false);
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_epoch.as_u64().to_le_bytes(),
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

    let since_value = Since::from_epoch(EpochNumberWithFraction::new(50, 0, 1), false);

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
    let timeout_epoch = Since::from_epoch(EpochNumberWithFraction::new(42, 0, 1), false);
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_epoch.as_u64().to_le_bytes(),
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

    let since_value = Since::from_epoch(EpochNumberWithFraction::new(50, 0, 1), false);

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
    let timeout_epoch = Since::from_epoch(EpochNumberWithFraction::new(42, 0, 1), false);
    let version: u8 = 0;

    let args = [
        merchant_pubkey_hash.as_ref(),
        user_pubkey_hash.as_ref(),
        &timeout_epoch.as_u64().to_le_bytes(),
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
        .build_script(&simple_udt_out_point.clone(), udt_owner_lock_hash.to_vec().into())
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

    let since_value = Since::from_epoch(EpochNumberWithFraction::new(50, 0, 1), false);

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

// Helper function to build and sign transaction
fn build_and_sign_tx(
    cell_deps: CellDepVec,
    input: CellInput,
    outputs: Vec<CellOutput>,
    outputs_data: Vec<Bytes>,
    unlock_type: u8,
    user_key: &(ckb_testtool::ckb_crypto::secp::Privkey, ckb_testtool::ckb_crypto::secp::Pubkey),
    merchant_key: &(ckb_testtool::ckb_crypto::secp::Privkey, ckb_testtool::ckb_crypto::secp::Pubkey),
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
