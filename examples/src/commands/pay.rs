use anyhow::{anyhow, Result};
use ckb_sdk::{constants::ONE_CKB, rpc::CkbRpcClient, Address, HumanCapacity};
use ckb_types::{
    core::{Capacity, TransactionView},
    packed::{CellOutput, Script},
    prelude::*,
    H256,
};
use serde::{Deserialize, Serialize};
use std::{fs, str::FromStr};

use crate::{
    storage::tx_storage::generate_tx_filename,
    tx_builder::commitment::build_commitment_transaction,
    utils::config::load_config,
};

/// Channel information loaded from file
#[derive(Debug, Serialize, Deserialize)]
struct ChannelInfo {
    user_address: String,
    merchant_address: String,
    capacity_ckb: u64,
    #[allow(dead_code)]
    timeout_epochs: u64,
    #[allow(dead_code)]
    current_timestamp: u64,
    #[allow(dead_code)]
    timeout_timestamp: u64,
    #[allow(dead_code)]
    spillman_lock_script_hash: String,
    funding_tx_hash: String,
    funding_output_index: u32,
}

pub async fn execute(
    amount: &str,
    channel_file: &str,
    config_path: &str,
) -> Result<()> {
    println!("\n═══════════════════════════════════════════════════════");
    println!("  💸 创建 Commitment Transaction (链下支付)");
    println!("═══════════════════════════════════════════════════════\n");

    // Parse payment amount from string (supports decimals like "100.5")
    let payment_capacity = HumanCapacity::from_str(amount)
        .map_err(|e| anyhow!("Invalid payment amount '{}': {}", amount, e))?;
    let payment_amount_shannons: u64 = payment_capacity.into();

    // 1. Load configuration
    println!("📋 加载配置...");
    let config = load_config(config_path)?;
    println!("✓ 配置加载完成");

    // 2. Load channel info
    println!("\n📂 加载通道信息...");
    let channel_info = load_channel_info(channel_file)?;
    println!("✓ 通道信息:");
    println!("  - 用户地址: {}", channel_info.user_address);
    println!("  - 商户地址: {}", channel_info.merchant_address);
    println!("  - 通道容量: {} CKB", channel_info.capacity_ckb);
    println!("  - Funding TX: {}", channel_info.funding_tx_hash);
    println!("  - Output Index: {}", channel_info.funding_output_index);

    // 3. Get Spillman Lock cell info from chain
    println!("\n🔍 从链上查询 Spillman Lock cell...");
    let rpc_client = CkbRpcClient::new(&config.network.rpc_url);

    let funding_tx_hash = H256::from_str(channel_info.funding_tx_hash.trim_start_matches("0x"))
        .map_err(|e| anyhow!("Invalid funding tx hash: {}", e))?;

    let funding_tx_with_status = rpc_client
        .get_transaction(funding_tx_hash.clone())
        .map_err(|e| anyhow!("RPC error: {:?}", e))?
        .ok_or_else(|| anyhow!("Funding transaction not found on chain"))?;

    let funding_tx_json = funding_tx_with_status.transaction
        .ok_or_else(|| anyhow!("Transaction view not found"))?;

    // Convert jsonrpc TransactionView to core TransactionView
    use ckb_jsonrpc_types::Either;
    let funding_tx: TransactionView = match funding_tx_json.inner {
        Either::Left(tx_view) => {
            let tx_packed: ckb_types::packed::Transaction = tx_view.inner.into();
            tx_packed.into_view()
        }
        Either::Right(_) => {
            return Err(anyhow!("Unexpected transaction format"));
        }
    };

    // Get the Spillman Lock cell (output at funding_output_index)
    let spillman_lock_cell = funding_tx
        .outputs()
        .get(channel_info.funding_output_index as usize)
        .ok_or_else(|| anyhow!("Spillman Lock cell not found at output index {}",
            channel_info.funding_output_index))?;

    let spillman_lock_capacity: u64 = spillman_lock_cell.capacity().unpack();
    let spillman_lock_script = spillman_lock_cell.lock();

    println!("✓ Spillman Lock cell 信息:");
    println!("  - Capacity: {}", HumanCapacity::from(spillman_lock_capacity));
    println!("  - Script hash: {:#x}", spillman_lock_script.calc_script_hash());

    // 4. Parse addresses
    let user_address = Address::from_str(&channel_info.user_address)
        .map_err(|e| anyhow!("Invalid user address: {}", e))?;
    let merchant_address = Address::from_str(&channel_info.merchant_address)
        .map_err(|e| anyhow!("Invalid merchant address: {}", e))?;

    let user_lock_script = Script::from(&user_address);
    let merchant_lock_script = Script::from(&merchant_address);

    // 5. Calculate merchant's minimum occupied capacity
    let merchant_cell = CellOutput::new_builder()
        .capacity(Capacity::shannons(0))
        .lock(merchant_lock_script.clone())
        .build();

    let merchant_min_capacity = merchant_cell
        .occupied_capacity(Capacity::bytes(0).unwrap())
        .map_err(|e| anyhow!("Failed to calculate merchant minimum capacity: {:?}", e))?
        .as_u64();

    println!("\n💰 支付详情:");
    println!("  - 商户最小占用容量: {}", HumanCapacity::from(merchant_min_capacity));

    // Merchant receives: payment amount + minimum occupied capacity
    let merchant_total_capacity = payment_amount_shannons + merchant_min_capacity;

    // Validate payment amount
    if merchant_total_capacity >= spillman_lock_capacity {
        return Err(anyhow!(
            "支付金额过大：商户将收到 {}（{} 支付 + {} 最小占用），超过通道容量 {}",
            HumanCapacity::from(merchant_total_capacity),
            payment_capacity,
            HumanCapacity::from(merchant_min_capacity),
            HumanCapacity::from(spillman_lock_capacity)
        ));
    }

    println!("  - 用户支付金额: {}", payment_capacity);
    println!("  - 商户实际收到: {} ({} 支付 + {} 最小占用)",
        HumanCapacity::from(merchant_total_capacity),
        payment_capacity,
        HumanCapacity::from(merchant_min_capacity));

    // 7. Build and save commitment transaction
    // Use cleaned amount string for filename (replace '.' with '_')
    let amount_str = amount.replace('.', "_");
    let output_file = generate_tx_filename("commitment", Some(&format!("{}_ckb", amount_str)));

    let (_tx_hash, _tx) = build_commitment_transaction(
        &config,
        funding_tx_hash,
        channel_info.funding_output_index,
        spillman_lock_capacity,
        spillman_lock_script,
        user_lock_script,
        merchant_lock_script,
        payment_amount_shannons,
        merchant_min_capacity,
        &output_file,
    )?;

    // Success message and next steps
    println!("\n✅ Commitment Transaction 创建成功!");
    println!("\n📌 下一步操作:");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("\n💡 这是一笔链下支付交易：");
    println!("  - 用户已签名，商户需要在结算时补充签名");
    println!("  - 商户可以随时广播此交易到链上结算");
    println!("\n🎯 商户结算命令：");
    println!("  spillman-cli settle --tx-file {} --config {}", output_file, config_path);
    println!("\n💸 继续支付（创建新的 commitment）：");
    println!("  spillman-cli pay --amount <更大的金额> --channel-file {} --config {}",
        channel_file, config_path);
    println!("\n⚠️  注意：每次支付的金额必须大于上一次！");

    Ok(())
}

/// Load channel information from JSON file
fn load_channel_info(file_path: &str) -> Result<ChannelInfo> {
    let json = fs::read_to_string(file_path)
        .map_err(|e| anyhow!("Failed to read channel info file {}: {}", file_path, e))?;

    let info: ChannelInfo = serde_json::from_str(&json)
        .map_err(|e| anyhow!("Failed to parse channel info: {}", e))?;

    Ok(info)
}
