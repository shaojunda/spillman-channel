use anyhow::{anyhow, Result};
use ckb_crypto::secp::Privkey;
use ckb_hash::blake2b_256;
use ckb_sdk::rpc::CkbRpcClient;
use ckb_types::{
    bytes::Bytes,
    core::TransactionView,
    packed::CellDepVec,
    prelude::*,
};
use std::{fs, str::FromStr};

use crate::utils::config::load_config;

// Constants for witness structure
const EMPTY_WITNESS_ARGS_SIZE: usize = 16;
const UNLOCK_TYPE_SIZE: usize = 1;
const SIGNATURE_SIZE: usize = 65;

/// Execute settle command - merchant signs and broadcasts commitment transaction
pub async fn execute(
    tx_file: &str,
    config_path: &str,
    broadcast: bool,
) -> Result<()> {
    println!("\n═══════════════════════════════════════════════════════");
    println!("  🏦 商户结算 Commitment Transaction");
    println!("═══════════════════════════════════════════════════════\n");

    // 1. Load configuration
    println!("📋 加载配置...");
    let config = load_config(config_path)?;
    println!("✓ 配置加载完成");

    // 2. Parse merchant private key from config
    println!("\n🔑 加载商户私钥...");
    let merchant_privkey = Privkey::from_str(&config.merchant.private_key)
        .map_err(|e| anyhow!("Failed to parse merchant private key: {:?}", e))?;
    println!("✓ 商户私钥加载完成");

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

    // 4. Verify witness structure
    let witness = tx.witnesses().get(0)
        .ok_or_else(|| anyhow!("Missing witness"))?;
    let witness_data = witness.raw_data();

    let expected_size = EMPTY_WITNESS_ARGS_SIZE + UNLOCK_TYPE_SIZE + SIGNATURE_SIZE + SIGNATURE_SIZE;
    if witness_data.len() != expected_size {
        return Err(anyhow!(
            "Invalid witness size: expected {}, got {}",
            expected_size,
            witness_data.len()
        ));
    }

    // Check if merchant signature is placeholder (all zeros)
    let merchant_sig_start = EMPTY_WITNESS_ARGS_SIZE + UNLOCK_TYPE_SIZE;
    let merchant_sig_end = merchant_sig_start + SIGNATURE_SIZE;
    let merchant_sig_placeholder = &witness_data[merchant_sig_start..merchant_sig_end];

    if !merchant_sig_placeholder.iter().all(|&b| b == 0) {
        return Err(anyhow!("Merchant signature already present in transaction"));
    }

    println!("✓ Witness 结构验证通过");

    // 5. Sign transaction
    println!("\n🔐 商户签名交易...");
    let signing_message = compute_signing_message(&tx);

    let merchant_sig = merchant_privkey
        .sign_recoverable(&signing_message.into())
        .map_err(|e| anyhow!("Failed to sign transaction: {:?}", e))?
        .serialize();

    println!("✓ 签名完成");

    // 6. Update witness with merchant signature
    let mut new_witness = Vec::with_capacity(expected_size);
    new_witness.extend_from_slice(&witness_data[..merchant_sig_start]); // EMPTY_WITNESS_ARGS + UNLOCK_TYPE
    new_witness.extend_from_slice(&merchant_sig); // Merchant signature
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
