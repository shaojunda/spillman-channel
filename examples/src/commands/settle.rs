use anyhow::{anyhow, Result};
use ckb_crypto::secp::Privkey;
use ckb_hash::blake2b_256;
use ckb_sdk::{constants::MultisigScript, rpc::CkbRpcClient, Address};
use ckb_types::{
    bytes::Bytes,
    core::TransactionView,
    packed::{CellDepVec, Script as PackedScript},
    prelude::*,
    H256,
};
use std::{fs, str::FromStr};

use crate::{
    tx_builder::funding_v2::build_multisig_config_with_type,
    tx_builder::witness_utils::{EMPTY_WITNESS_ARGS_SIZE, SIGNATURE_SIZE, UNLOCK_TYPE_SIZE},
    utils::config::load_config,
};

/// Execute settle command - merchant signs and broadcasts commitment transaction
pub async fn execute(tx_file: &str, config_path: &str, broadcast: bool) -> Result<()> {
    println!("\n═══════════════════════════════════════════════════════");
    println!("  🏦 商户结算 Commitment Transaction");
    println!("═══════════════════════════════════════════════════════\n");

    // 1. Load configuration
    println!("📋 加载配置...");
    let config = load_config(config_path)?;
    println!("✓ 配置加载完成");

    // 2. Check if merchant uses multisig
    println!("\n🔑 检测商户签名类型...");
    let is_multisig = config.merchant.is_multisig();

    let (merchant_multisig_config, merchant_privkeys) = if is_multisig {
        println!("✓ 商户使用多签地址");

        // Get multisig parameters from config
        let threshold = config
            .merchant
            .multisig_threshold
            .ok_or_else(|| anyhow!("Merchant multisig_threshold is required"))?;
        let total = config
            .merchant
            .multisig_total
            .ok_or_else(|| anyhow!("Merchant multisig_total is required"))?;

        // Parse private keys
        let privkeys = config
            .merchant
            .private_keys
            .as_ref()
            .ok_or_else(|| anyhow!("Merchant private_keys is required for multisig"))?;

        let parsed_keys: Result<Vec<secp256k1::SecretKey>> = privkeys
            .iter()
            .map(|key_str| {
                let key_bytes = hex::decode(key_str.trim_start_matches("0x"))
                    .map_err(|e| anyhow!("Failed to decode private key: {}", e))?;
                secp256k1::SecretKey::from_slice(&key_bytes)
                    .map_err(|e| anyhow!("Invalid private key: {}", e))
            })
            .collect();

        let keys = parsed_keys?;
        println!("  - 已加载 {} 个私钥", keys.len());

        // Detect merchant address type (Legacy or V2)
        let merchant_address = Address::from_str(&config.merchant.address)
            .map_err(|e| anyhow!("Failed to parse merchant address: {}", e))?;
        let merchant_lock_script = PackedScript::from(&merchant_address);
        let code_hash: H256 = merchant_lock_script.code_hash().unpack();

        let legacy_script_id = MultisigScript::Legacy.script_id();
        let v2_script_id = MultisigScript::V2.script_id();

        let multisig_type = if code_hash == legacy_script_id.code_hash
            && merchant_lock_script.hash_type() == legacy_script_id.hash_type.into()
        {
            println!("  - 检测到 Legacy multisig 地址");
            MultisigScript::Legacy
        } else if code_hash == v2_script_id.code_hash
            && merchant_lock_script.hash_type() == v2_script_id.hash_type.into()
        {
            println!("  - 检测到 V2 multisig 地址");
            MultisigScript::V2
        } else {
            return Err(anyhow!("Unknown multisig type for merchant address"));
        };

        // Build multisig config with detected type
        let multisig_config =
            build_multisig_config_with_type(&keys, threshold, total, multisig_type)?;
        println!(
            "  - 多签配置: {}-of-{}",
            multisig_config.threshold(),
            multisig_config.sighash_addresses().len()
        );

        (Some(multisig_config), keys)
    } else {
        println!("✓ 商户使用单签地址");

        // Parse single private key
        let key_str = config
            .merchant
            .private_key
            .as_ref()
            .ok_or_else(|| anyhow!("Merchant private_key is required"))?;

        let key_bytes = hex::decode(key_str.trim_start_matches("0x"))
            .map_err(|e| anyhow!("Failed to decode private key: {}", e))?;
        let key = secp256k1::SecretKey::from_slice(&key_bytes)
            .map_err(|e| anyhow!("Invalid private key: {}", e))?;

        (None, vec![key])
    };

    // 3. Load commitment transaction from file
    println!("\n📄 加载 Commitment 交易: {}", tx_file);
    let tx_json_str = fs::read_to_string(tx_file)
        .map_err(|e| anyhow!("Failed to read transaction file: {}", e))?;

    let tx_json: ckb_jsonrpc_types::TransactionView = serde_json::from_str(&tx_json_str)
        .map_err(|e| anyhow!("Failed to parse transaction JSON: {}", e))?;

    // Convert to core TransactionView
    let tx_packed: ckb_types::packed::Transaction = tx_json.inner.into();
    let tx: TransactionView = tx_packed.into_view();

    println!("✓ 交易加载完成");
    println!("  - TX Hash: {:#x}", tx.hash());
    println!("  - Inputs: {}", tx.inputs().len());
    println!("  - Outputs: {}", tx.outputs().len());

    // 4. Verify witness structure and determine sizes
    let witness = tx
        .witnesses()
        .get(0)
        .ok_or_else(|| anyhow!("Missing witness"))?;
    let witness_data = witness.raw_data();

    // Calculate expected witness size based on multisig config
    let (merchant_sig_start, merchant_sig_size, expected_size) =
        if let Some(ref multisig_config) = merchant_multisig_config {
            let config_data = multisig_config.to_witness_data();
            let threshold = multisig_config.threshold() as usize;
            let merchant_sigs_size = threshold * SIGNATURE_SIZE;

            let start = EMPTY_WITNESS_ARGS_SIZE + UNLOCK_TYPE_SIZE;
            let size = config_data.len() + merchant_sigs_size;
            let total = start + size + SIGNATURE_SIZE;

            (start, size, total)
        } else {
            let start = EMPTY_WITNESS_ARGS_SIZE + UNLOCK_TYPE_SIZE;
            let size = SIGNATURE_SIZE;
            let total = start + size + SIGNATURE_SIZE;

            (start, size, total)
        };

    if witness_data.len() != expected_size {
        return Err(anyhow!(
            "Invalid witness size: expected {}, got {}",
            expected_size,
            witness_data.len()
        ));
    }

    // Check if merchant signature is placeholder (all zeros)
    let merchant_sig_end = merchant_sig_start + merchant_sig_size;
    let merchant_sig_placeholder = &witness_data[merchant_sig_start..merchant_sig_end];

    if !merchant_sig_placeholder.iter().all(|&b| b == 0) {
        return Err(anyhow!("Merchant signature already present in transaction"));
    }

    println!("✓ Witness 结构验证通过");

    // 5. Sign transaction
    println!("\n🔐 商户签名交易...");
    let signing_message = compute_signing_message(&tx);

    // Build merchant signatures based on single-sig or multisig
    let merchant_witness_data = if let Some(ref multisig_config) = merchant_multisig_config {
        // Multisig: need to sign with threshold number of keys
        let threshold = multisig_config.threshold() as usize;
        let mut signatures = Vec::new();

        for (i, key) in merchant_privkeys.iter().take(threshold).enumerate() {
            let privkey_bytes = key.secret_bytes();
            let privkey = Privkey::from_slice(&privkey_bytes);

            let sig = privkey
                .sign_recoverable(&signing_message.into())
                .map_err(|e| anyhow!("Failed to sign with key {}: {:?}", i, e))?
                .serialize();

            signatures.extend_from_slice(&sig);
            println!("  ✓ 签名 {}/{} 完成", i + 1, threshold);
        }

        // Build multisig witness: multisig_config + signatures
        let mut multisig_witness = multisig_config.to_witness_data();
        multisig_witness.extend_from_slice(&signatures);
        multisig_witness
    } else {
        // Single-sig: just one signature
        let privkey_bytes = merchant_privkeys[0].secret_bytes();
        let privkey = Privkey::from_slice(&privkey_bytes);

        let sig = privkey
            .sign_recoverable(&signing_message.into())
            .map_err(|e| anyhow!("Failed to sign transaction: {:?}", e))?
            .serialize();

        println!("  ✓ 签名完成");
        sig.to_vec()
    };

    // 6. Update witness with merchant signature
    let mut new_witness = Vec::with_capacity(expected_size);
    new_witness.extend_from_slice(&witness_data[..merchant_sig_start]); // EMPTY_WITNESS_ARGS + UNLOCK_TYPE
    new_witness.extend_from_slice(&merchant_witness_data); // Merchant signature(s)
    new_witness.extend_from_slice(&witness_data[merchant_sig_end..]); // User signature

    let signed_tx = tx
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from(new_witness).pack()])
        .build();

    let signed_tx_hash = signed_tx.hash();
    println!("✓ 交易签名更新完成");
    println!("  - New TX Hash: {:#x}", signed_tx_hash);

    // 7. Broadcast transaction (optional)
    if broadcast {
        println!("\n📡 广播交易到链上...");
        let rpc_client = CkbRpcClient::new(&config.network.rpc_url);

        // Convert to JSON RPC format (standard SDK method)
        let signed_tx_json = ckb_jsonrpc_types::TransactionView::from(signed_tx.clone());

        let tx_hash = rpc_client
            .send_transaction(signed_tx_json.inner, None)
            .map_err(|e| anyhow!("Failed to broadcast transaction: {:?}", e))?;

        println!("✓ 交易已广播");
        println!("  - TX Hash: {:#x}", tx_hash);

        // 8. Success message
        println!("\n✅ 结算成功！");
        println!("\n📌 后续操作:");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("\n🔍 查询交易状态：");
        println!("  ckb-cli rpc get_transaction --hash {:#x}", tx_hash);
        println!("\n⏳ 等待交易上链确认...");
        println!("  交易确认后，支付金额将到达商户地址");
    } else {
        // Save signed transaction to file
        println!("\n💾 保存已签名交易...");

        let signed_tx_json = ckb_jsonrpc_types::TransactionView::from(signed_tx);
        let output_path = tx_file.replace(".json", "_signed.json");

        let json_str = serde_json::to_string_pretty(&signed_tx_json.inner)?;
        fs::write(&output_path, json_str)?;

        println!("✓ 已签名交易已保存到: {}", output_path);

        // 8. Success message
        println!("\n✅ 交易签名完成 - 未广播");
        println!("\n📌 后续操作:");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("\n📄 已签名交易文件: {}", output_path);
        println!("\n📡 手动广播交易:");
        println!("  spillman-cli settle --tx-file {} --broadcast", tx_file);
        println!("  或者使用其他工具手动发送交易");
    }

    Ok(())
}

/// Compute signing message for Spillman Lock
///
/// Spillman Lock signs the raw transaction without cell_deps
fn compute_signing_message(tx: &TransactionView) -> [u8; 32] {
    let raw_tx = tx
        .data()
        .raw()
        .as_builder()
        .cell_deps(CellDepVec::default())
        .build();

    blake2b_256(raw_tx.as_slice())
}
