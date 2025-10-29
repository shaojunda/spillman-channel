use crate::Loader;
use ckb_sdk::util::blake160;
use ckb_std::since::{EpochNumberWithFraction, Since};
use ckb_testtool::context::Context;
use ckb_testtool::{
    builtin::ALWAYS_SUCCESS,
    ckb_crypto::secp::Generator,
    ckb_hash::blake2b_256,
    ckb_types::{
        bytes::Bytes,
        core::{
            ScriptHashType,
            TransactionBuilder,
            TransactionView
        }, packed::*, prelude::*},
};


const EMPTY_WITNESS_ARGS: [u8; 16] = [16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0];
const UNLOCK_TYPE_COMMITMENT: u8 = 0x00;
const UNLOCK_TYPE_TIMEOUT: u8 = 0x01;

// Mainnet/Testnet secp256k1_blake160_sighash_all code_hash
const SECP256K1_CODE_HASH: [u8; 32] = [
    0x9b, 0xd7, 0xe0, 0x6f, 0x3e, 0xcf, 0x4b, 0xe0,
    0xf2, 0xfc, 0xd2, 0x18, 0x8b, 0x23, 0xf1, 0xb9,
    0xfc, 0xc8, 0x8e, 0x5d, 0x4b, 0x65, 0xa8, 0x63,
    0x7b, 0x17, 0x72, 0x3b, 0xbd, 0xa3, 0xcc, 0xe8,
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

    // build transaction
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .input(input)
        .outputs(outputs)
        .outputs_data(outputs_data.pack())
        .build();
    let tx = context.complete_tx(tx);

    let message = compute_signing_message(&tx);
    let user_signature = user_key.0.sign_recoverable(&message.into()).unwrap().serialize();
    let merchant_signature = merchant_key.0.sign_recoverable(&message.into()).unwrap().serialize();
    let witness = [
        &EMPTY_WITNESS_ARGS[..],
        &[UNLOCK_TYPE_COMMITMENT][..],
        &merchant_signature[..],
        &user_signature[..],
    ].concat();

    let success_tx = tx.as_advanced_builder().witness(witness.pack()).build();


    // run
    let cycles = context
        .verify_tx(&success_tx, 10_000_000)
        .expect("pass verification");
    println!("consume cycles: {}", cycles);

    // wrong user signature should fail verification
    let wrong_user_signature = [0u8; 65];
    let wrong_witness = [
        &EMPTY_WITNESS_ARGS[..],
        &[UNLOCK_TYPE_COMMITMENT][..],
        &merchant_signature[..],
        &wrong_user_signature[..],
    ].concat();
    let fail_tx = tx.as_advanced_builder().witness(wrong_witness.pack()).build();

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

    let outputs = vec![
        CellOutput::new_builder()
            .capacity(100_000_000_000u64.pack()) // 1000 CKB refund to user, 1 CKB fee
            .lock(user_lock_script.clone())
            .build(),
    ];

    let outputs_data = vec![Bytes::new(); 1];

    // build transaction
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .input(input.clone())
        .outputs(outputs)
        .outputs_data(outputs_data.pack())
        .build();
    let tx = context.complete_tx(tx);

    let message = compute_signing_message(&tx);
    let user_signature = user_key.0.sign_recoverable(&message.into()).unwrap().serialize();
    let merchant_signature = merchant_key.0.sign_recoverable(&message.into()).unwrap().serialize();

    // For timeout path: witness format is the same, but unlock_type is UNLOCK_TYPE_TIMEOUT
    let witness = [
        &EMPTY_WITNESS_ARGS[..],
        &[UNLOCK_TYPE_TIMEOUT][..],
        &merchant_signature[..],
        &user_signature[..],
    ].concat();

    let success_tx = tx.as_advanced_builder().witness(witness.pack()).build();

    // run
    let cycles = context
        .verify_tx(&success_tx, 10_000_000)
        .expect("pass verification");
    println!("consume cycles: {}", cycles);

    // Test: timeout not reached should fail
    let early_since = Since::from_epoch(EpochNumberWithFraction::new(10, 0, 1), false);
    let early_input = success_tx.inputs().get(0).unwrap()
        .as_builder()
        .since(early_since.as_u64().pack())
        .build();

    let early_tx = success_tx.as_advanced_builder()
        .set_inputs(vec![early_input])
        .build();

    let err = context
        .verify_tx(&early_tx, 10_000_000)
        .expect_err("timeout not reached should fail verification");
    println!("error (timeout not reached): {:?}", err);

    // Test: invalid unlock type should fail
    let invalid_unlock_type = 0x02; // not COMMITMENT(0x00) or TIMEOUT(0x01)
    let invalid_witness = [
        &EMPTY_WITNESS_ARGS[..],
        &[invalid_unlock_type][..],
        &merchant_signature[..],
        &user_signature[..],
    ].concat();

    let invalid_tx = success_tx.as_advanced_builder()
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

    let excessive_fee_base_tx = TransactionBuilder::default()
        .cell_deps(tx.cell_deps())
        .input(input.clone())
        .output(small_output)
        .output_data(Bytes::new().pack())
        .build();
    let excessive_fee_base_tx = context.complete_tx(excessive_fee_base_tx);

    // Re-sign with the new transaction message
    let excessive_fee_message = compute_signing_message(&excessive_fee_base_tx);
    let excessive_fee_user_sig = user_key.0.sign_recoverable(&excessive_fee_message.into()).unwrap().serialize();
    let excessive_fee_merchant_sig = merchant_key.0.sign_recoverable(&excessive_fee_message.into()).unwrap().serialize();
    let excessive_fee_witness = [
        &EMPTY_WITNESS_ARGS[..],
        &[UNLOCK_TYPE_TIMEOUT][..],
        &excessive_fee_merchant_sig[..],
        &excessive_fee_user_sig[..],
    ].concat();

    let excessive_fee_tx = excessive_fee_base_tx.as_advanced_builder()
        .witness(excessive_fee_witness.pack())
        .build();

    let err = context
        .verify_tx(&excessive_fee_tx, 10_000_000)
        .expect_err("excessive fee should fail verification");
    println!("error (excessive fee): {:?}", err);
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
