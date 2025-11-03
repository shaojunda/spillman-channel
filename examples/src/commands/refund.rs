use anyhow::{anyhow, Result};
use ckb_sdk::Address;
use ckb_types::{core::TransactionView, prelude::*, H256};
use std::str::FromStr;

use crate::{
    tx_builder::refund::build_refund_transaction,
    tx_builder::refund_v2,
    utils::config::load_config,
};

pub async fn execute(
    tx_file: &str,
    config_path: &str,
) -> Result<()> {
    println!("🔄 执行 Refund 命令");
    println!("═══════════════════════════════════════════");

    // Load config
    let config = load_config(config_path)?;
    println!("✓ 配置文件已加载: {}", config_path);

    // Read funding transaction
    println!("\n📖 读取 Funding 交易...");
    let funding_tx_json = std::fs::read_to_string(tx_file)
        .map_err(|e| anyhow!("Failed to read funding tx file: {}", e))?;

    let funding_tx_view: ckb_jsonrpc_types::TransactionView = serde_json::from_str(&funding_tx_json)
        .map_err(|e| anyhow!("Failed to parse funding tx JSON: {}", e))?;

    // Convert jsonrpc TransactionView to core TransactionView
    let funding_tx_packed: ckb_types::packed::Transaction = funding_tx_view.inner.into();
    let funding_tx: TransactionView = funding_tx_packed.into_view();
    let funding_tx_hash: H256 = funding_tx.hash().unpack();

    println!("  - Funding tx hash: {:#x}", funding_tx_hash);
    println!("  - Inputs: {}", funding_tx.inputs().len());
    println!("  - Outputs: {}", funding_tx.outputs().len());

    // Analyze funding transaction to determine mode
    println!("\n📊 分析 Funding 交易模式...");

    // Collect unique lock scripts from inputs by querying previous cells
    // For now, we'll use a simplified approach: check if inputs > 1
    let is_cofund = funding_tx.inputs().len() > 1;

    println!("  - 模式: {}", if is_cofund { "Co-fund (共同出资)" } else { "Single fund (用户单独出资)" });

    // Parse addresses
    let user_address = Address::from_str(&config.user.address)
        .map_err(|e| anyhow!("Failed to parse user address: {}", e))?;
    let user_lock = ckb_types::packed::Script::from(&user_address);

    let merchant_lock = if is_cofund {
        let merchant_address = Address::from_str(&config.merchant.address)
            .map_err(|e| anyhow!("Failed to parse merchant address: {}", e))?;
        Some(ckb_types::packed::Script::from(&merchant_address))
    } else {
        None
    };

    // Get timeout epoch from config
    let timeout_epoch = config.channel.timeout_epochs;

    // Build refund transaction
    println!("\n🔨 构建 Refund 交易...");
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let output_path = format!("secrets/refund_tx_{}.json", timestamp);

    let _refund_tx = build_refund_transaction(
        &config,
        funding_tx_hash,
        &funding_tx,
        user_lock,
        merchant_lock,
        timeout_epoch,
        &output_path,
    )?;

    println!("\n✅ Refund 交易构建成功！");
    println!("═══════════════════════════════════════════");
    println!("📄 交易已保存: {}", output_path);
    println!("\n💡 提示：");
    println!("  - 交易已包含双方签名（Merchant 预签名 + User 签名）");
    println!("  - 按照 Spillman Channel 设计：");
    println!("    1. Merchant 在通道创建时预签名（保证用户退款权利）");
    println!("    2. User 在超时后补充签名");
    println!("  - 等待超时 epoch ({}) 后可以广播此交易", timeout_epoch);
    println!("  - 使用 ckb-cli 广播: ckb-cli tx send --tx-file {}", output_path);
    if is_cofund {
        println!("\n📊 Co-fund 模式退款：");
        println!("  - User 取回自己的出资");
        println!("  - Merchant 取回自己的出资");
    } else {
        println!("\n📊 Single fund 模式退款：");
        println!("  - User 取回全部资金");
    }

    Ok(())
}

/// Execute refund command using refund_v2 (new implementation)
///
/// This is the v2 implementation using the refactored refund_v2 module.
/// The original execute() function above is kept as v1 backup.
pub async fn execute_v2(
    tx_file: &str,
    config_path: &str,
) -> Result<()> {
    println!("🔄 执行 Refund 命令 (v2)");
    println!("═══════════════════════════════════════════");

    // Load config
    let config = load_config(config_path)?;
    println!("✓ 配置文件已加载: {}", config_path);

    // Read funding transaction
    println!("\n📖 读取 Funding 交易...");
    let funding_tx_json = std::fs::read_to_string(tx_file)
        .map_err(|e| anyhow!("Failed to read funding tx file: {}", e))?;

    let funding_tx_view: ckb_jsonrpc_types::TransactionView = serde_json::from_str(&funding_tx_json)
        .map_err(|e| anyhow!("Failed to parse funding tx JSON: {}", e))?;

    // Convert jsonrpc TransactionView to core TransactionView
    let funding_tx_packed: ckb_types::packed::Transaction = funding_tx_view.inner.into();
    let funding_tx: TransactionView = funding_tx_packed.into_view();
    let funding_tx_hash: H256 = funding_tx.hash().unpack();

    println!("  - Funding tx hash: {:#x}", funding_tx_hash);
    println!("  - Inputs: {}", funding_tx.inputs().len());
    println!("  - Outputs: {}", funding_tx.outputs().len());

    // Analyze funding transaction to determine mode
    println!("\n📊 分析 Funding 交易模式...");

    // Check if co-fund mode by checking if inputs > 1
    let is_cofund = funding_tx.inputs().len() > 1;

    println!("  - 模式: {}", if is_cofund { "Co-fund (共同出资)" } else { "Single fund (用户单独出资)" });

    // Parse addresses
    let user_address = Address::from_str(&config.user.address)
        .map_err(|e| anyhow!("Failed to parse user address: {}", e))?;

    let merchant_address = if is_cofund {
        let merchant_addr = Address::from_str(&config.merchant.address)
            .map_err(|e| anyhow!("Failed to parse merchant address: {}", e))?;
        Some(merchant_addr)
    } else {
        None
    };

    // Build refund transaction using v2
    println!("\n🔨 构建 Refund 交易 (v2)...");
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let output_path = format!("secrets/refund_tx_{}.json", timestamp);

    let (_tx_hash, _tx) = refund_v2::build_refund_transaction(
        &config,
        funding_tx_hash,
        &funding_tx,
        &user_address,
        merchant_address.as_ref(),
        &output_path,
    ).await?;

    println!("\n✅ Refund 交易构建成功！(v2)");
    println!("═══════════════════════════════════════════");
    println!("📄 交易已保存: {}", output_path);
    println!("\n💡 提示：");
    println!("  - 交易已构建完成（未签名）");
    println!("  - 按照 Spillman Channel 设计：");
    println!("    1. Merchant 在通道创建时预签名（保证用户退款权利）");
    println!("    2. User 在超时后补充签名");
    println!("  - 等待超时后可以签名并广播此交易");
    println!("  - 使用 ckb-cli 广播: ckb-cli tx send --tx-file {}", output_path);
    if is_cofund {
        println!("\n📊 Co-fund 模式退款：");
        println!("  - User 取回自己的出资");
        println!("  - Merchant 取回自己的出资");
    } else {
        println!("\n📊 Single fund 模式退款：");
        println!("  - User 取回全部资金");
    }

    Ok(())
}
