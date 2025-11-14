use anyhow::{anyhow, Result};
use ckb_sdk::{rpc::CkbRpcClient, Address, HumanCapacity};
use ckb_types::{
    core::{Capacity, TransactionView},
    packed::{CellOutput, Script},
    prelude::*,
    H256,
};
use serde::{Deserialize, Serialize};
use std::{fs, str::FromStr};

use crate::{tx_builder::commitment::build_commitment_transaction, utils::config::load_config};

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
    // xUDT fields (optional, only present in xUDT channels)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    xudt_type_script: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    xudt_amount: Option<String>,
}

pub async fn execute(
    amount: &str,
    channel_file: &str,
    config_path: &str,
    fee_rate: u64,
) -> Result<()> {
    println!("\n═══════════════════════════════════════════════════════");
    println!("  💸 创建 Commitment Transaction (链下支付)");
    println!("═══════════════════════════════════════════════════════\n");

    // 1. Load configuration (need to check if xUDT before parsing amount)
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

    let funding_tx_json = funding_tx_with_status
        .transaction
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
        .ok_or_else(|| {
            anyhow!(
                "Spillman Lock cell not found at output index {}",
                channel_info.funding_output_index
            )
        })?;

    let spillman_lock_capacity: u64 = spillman_lock_cell.capacity().unpack();
    let spillman_lock_script = spillman_lock_cell.lock();

    // Check if this is an xUDT channel
    let (xudt_type_script, xudt_total_amount) =
        if let Some(type_script) = spillman_lock_cell.type_().to_opt() {
            // Extract xUDT amount from cell data
            let cell_data = funding_tx
                .outputs_data()
                .get(channel_info.funding_output_index as usize)
                .ok_or_else(|| anyhow!("Cell data not found"))?;
            let data_bytes: Vec<u8> = cell_data.unpack();

            if data_bytes.len() >= 16 {
                let xudt_amount = u128::from_le_bytes(
                    data_bytes[0..16]
                        .try_into()
                        .map_err(|_| anyhow!("Failed to parse xUDT amount"))?,
                );
                (Some(type_script), Some(xudt_amount))
            } else {
                return Err(anyhow!("Invalid xUDT data length: {}", data_bytes.len()));
            }
        } else {
            (None, None)
        };

    println!("✓ Spillman Lock cell 信息:");
    println!(
        "  - Capacity: {}",
        HumanCapacity::from(spillman_lock_capacity)
    );
    println!(
        "  - Script hash: {:#x}",
        spillman_lock_script.calc_script_hash()
    );
    if let Some(xudt_amount) = xudt_total_amount {
        println!("  - xUDT amount: {}", xudt_amount);
    }

    // 3.5 Parse payment amount based on channel type
    let (payment_amount_shannons, xudt_payment_amount) = if xudt_type_script.is_some() {
        // xUDT channel: amount is xUDT quantity, need to convert using decimal
        let usdi_config = config
            .usdi
            .as_ref()
            .ok_or_else(|| anyhow!("xUDT channel detected but usdi config not found"))?;

        let payment_amount_f64 = amount
            .parse::<f64>()
            .map_err(|e| anyhow!("Invalid xUDT amount '{}': {}", amount, e))?;

        let decimal = usdi_config.decimal;
        let multiplier = 10u128.pow(decimal as u32);
        let xudt_payment = (payment_amount_f64 * multiplier as f64) as u128;

        println!("\n💰 xUDT 支付详情:");
        println!(
            "  - 支付 xUDT 数量: {} (decimal: {}, smallest unit: {})",
            payment_amount_f64, decimal, xudt_payment
        );

        // Validate xUDT payment amount
        let xudt_total = xudt_total_amount.ok_or_else(|| anyhow!("xUDT total amount not found"))?;
        if xudt_payment > xudt_total {
            return Err(anyhow!(
                "xUDT 支付金额过大：支付 {}，通道总量 {}",
                xudt_payment,
                xudt_total
            ));
        }

        // For xUDT channel, CKB payment is 0 (merchant only gets minimum occupied capacity)
        (0u64, Some(xudt_payment))
    } else {
        // Regular CKB channel: amount is CKB quantity
        let payment_capacity = HumanCapacity::from_str(amount)
            .map_err(|e| anyhow!("Invalid CKB amount '{}': {}", amount, e))?;
        let payment_shannons: u64 = payment_capacity.into();

        println!("\n💰 CKB 支付详情:");
        println!("  - 支付 CKB 数量: {}", payment_capacity);

        (payment_shannons, None)
    };

    // 4. Parse addresses
    let user_address = Address::from_str(&channel_info.user_address)
        .map_err(|e| anyhow!("Invalid user address: {}", e))?;
    let merchant_address = Address::from_str(&channel_info.merchant_address)
        .map_err(|e| anyhow!("Invalid merchant address: {}", e))?;

    let user_lock_script = Script::from(&user_address);
    let merchant_lock_script = Script::from(&merchant_address);

    // 5. Calculate merchant's minimum occupied capacity (including type script for xUDT)
    let mut merchant_cell_builder = CellOutput::new_builder()
        .capacity(Capacity::shannons(0))
        .lock(merchant_lock_script.clone());

    // Add type script if xUDT channel
    let data_size = if let Some(ref type_script) = xudt_type_script {
        merchant_cell_builder = merchant_cell_builder.type_(Some(type_script.clone()).pack());
        16 // 16 bytes for xUDT data
    } else {
        0
    };

    let merchant_cell = merchant_cell_builder.build();

    let merchant_min_capacity = merchant_cell
        .occupied_capacity(Capacity::bytes(data_size).unwrap())
        .map_err(|e| anyhow!("Failed to calculate merchant minimum capacity: {:?}", e))?
        .as_u64();

    // Merchant receives: payment amount + minimum occupied capacity
    let merchant_total_capacity = payment_amount_shannons + merchant_min_capacity;

    // Validate payment amount (CKB channel only)
    if xudt_type_script.is_none() {
        if merchant_total_capacity >= spillman_lock_capacity {
            return Err(anyhow!(
                "CKB 支付金额过大：商户将收到 {}（{} 支付 + {} 最小占用），超过通道容量 {}",
                HumanCapacity::from(merchant_total_capacity),
                HumanCapacity::from(payment_amount_shannons),
                HumanCapacity::from(merchant_min_capacity),
                HumanCapacity::from(spillman_lock_capacity)
            ));
        }

        println!(
            "  - 商户最小占用容量: {}",
            HumanCapacity::from(merchant_min_capacity)
        );
        println!(
            "  - 商户实际收到 CKB: {} ({} 支付 + {} 最小占用)",
            HumanCapacity::from(merchant_total_capacity),
            HumanCapacity::from(payment_amount_shannons),
            HumanCapacity::from(merchant_min_capacity)
        );
    } else {
        // xUDT channel: only show xUDT payment details
        println!(
            "  - 商户收到 CKB: {} (仅最小占用)",
            HumanCapacity::from(merchant_min_capacity)
        );
        if let Some(xudt_payment) = xudt_payment_amount {
            let xudt_total = xudt_total_amount.unwrap();
            let xudt_change = xudt_total - xudt_payment;
            println!("  - 商户收到 xUDT: {}", xudt_payment);
            println!("  - 用户保留 xUDT: {}", xudt_change);
        }
    }

    // 7. Build and save commitment transaction
    // Use cleaned amount string for filename (replace '.' with '_')
    let amount_str = amount.replace('.', "_");
    let output_file = format!("commitment_{}_ckb.json", amount_str);

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
        fee_rate,
        &output_file,
        xudt_type_script,
        xudt_total_amount,
        xudt_payment_amount,
    )?;

    // Success message and next steps
    println!("\n✅ Commitment Transaction 创建成功!");
    println!("\n📌 下一步操作:");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("\n💡 这是一笔链下支付交易：");
    println!("  - 用户已签名，商户需要在结算时补充签名");
    println!("  - 商户可以随时广播此交易到链上结算");
    println!("\n🎯 商户结算命令：");
    println!(
        "  spillman-cli settle --tx-file {} --config {}",
        output_file, config_path
    );
    println!("\n💸 继续支付（创建新的 commitment）：");
    println!(
        "  spillman-cli pay --amount <更大的金额> --channel-file {} --config {}",
        channel_file, config_path
    );
    println!("\n⚠️  注意：每次支付的金额必须大于上一次！");

    Ok(())
}

/// Load channel information from JSON file
fn load_channel_info(file_path: &str) -> Result<ChannelInfo> {
    let json = fs::read_to_string(file_path)
        .map_err(|e| anyhow!("Failed to read channel info file {}: {}", file_path, e))?;

    let info: ChannelInfo =
        serde_json::from_str(&json).map_err(|e| anyhow!("Failed to parse channel info: {}", e))?;

    Ok(info)
}
