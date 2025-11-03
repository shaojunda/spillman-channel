use anyhow::{anyhow, Result};
use ckb_sdk::{rpc::CkbRpcClient, Address};
use serde::{Deserialize, Serialize};
use std::fs;
use std::str::FromStr;

use crate::tx_builder::funding::{build_cofund_funding_transaction, build_funding_transaction};
use crate::tx_builder::funding_v2;
use crate::tx_builder::spillman_lock::build_spillman_lock_script;
use crate::utils::config::load_config;
use crate::utils::crypto::parse_privkey;
use crate::utils::rpc::get_current_timestamp;

#[derive(Debug, Serialize, Deserialize)]
struct ChannelInfo {
    user_address: String,
    merchant_address: String,
    capacity_ckb: u64,
    timeout_epochs: u64,
    current_epoch: u64,
    timeout_epoch: u64,
    spillman_lock_script_hash: String,
    funding_tx_hash: String,
    funding_output_index: u32,
}

pub async fn execute(
    config_path: &str,
    output_dir: &str,
    merchant_address: Option<&str>,
    capacity: Option<u64>,
    timeout_epochs: Option<u64>,
    co_fund: bool,
) -> Result<()> {
    println!("🚀 执行 set-up 命令 - 准备 Spillman Channel");
    println!("==========================================\n");

    // 1. Load configuration
    println!("📋 加载配置文件: {}", config_path);
    let config = load_config(config_path)?;
    println!("✓ 配置加载成功");

    // Use values from config file, allow CLI to override
    let user_address = &config.user.address;
    let capacity = capacity.unwrap_or(config.channel.capacity_ckb);
    let timeout_epochs = timeout_epochs.unwrap_or(config.channel.timeout_epochs);

    // 2. Parse user and merchant info
    println!("\n👤 解析用户和商户信息...");
    let user_privkey = parse_privkey(&config.user.private_key)?;
    let merchant_privkey = parse_privkey(&config.merchant.private_key)?;

    let user_pubkey = user_privkey.pubkey()?;
    let merchant_pubkey = merchant_privkey.pubkey()?;

    println!("✓ 用户地址: {}", user_address);
    println!("✓ 用户公钥: {}", hex::encode(user_pubkey.serialize()));
    println!("✓ 商户公钥: {}", hex::encode(merchant_pubkey.serialize()));

    if co_fund {
        println!("✓ 模式: Co-fund (User + Merchant 共同出资)");
    } else {
        println!("✓ 模式: User 单独出资");
    }

    // 3. Connect to CKB network
    println!("\n🔗 连接到 CKB 网络...");
    let rpc_client = CkbRpcClient::new(&config.network.rpc_url);

    // Get current timestamp and calculate timeout
    // Note: timeout_epochs is converted to seconds (1 epoch ≈ 4 hours = 14400 seconds)
    let current_timestamp = get_current_timestamp(&rpc_client).await?;
    let timeout_seconds = timeout_epochs * 14400; // 4 hours per epoch
    let timeout_timestamp = current_timestamp + timeout_seconds;

    println!("✓ RPC URL: {}", config.network.rpc_url);
    println!("✓ 当前时间戳: {} ({} UTC)", current_timestamp,
        chrono::DateTime::from_timestamp(current_timestamp as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "Invalid".to_string())
    );
    println!("✓ 超时时间戳: {} (+{} epochs ≈ {} hours)",
        timeout_timestamp, timeout_epochs, timeout_epochs * 4);
    println!("  超时时间: {}",
        chrono::DateTime::from_timestamp(timeout_timestamp as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Invalid".to_string())
    );

    // 4. Build Spillman Lock script
    println!("\n🔒 构建 Spillman Lock script...");
    let spillman_lock_script = build_spillman_lock_script(
        &config,
        &user_pubkey,
        &merchant_pubkey,
        timeout_timestamp,
    )?;

    let script_hash = spillman_lock_script.calc_script_hash();
    println!("✓ Spillman Lock script hash: {:#x}", script_hash);
    println!("✓ Lock script args 长度: {} bytes", spillman_lock_script.args().raw_data().len());

    // 5. Build and sign funding transaction
    println!("\n📝 构建并签名 Funding Transaction...");

    // Create output directory structure
    let output_path = std::path::Path::new(output_dir);
    let secrets_dir = output_path.join("secrets");
    fs::create_dir_all(&secrets_dir)?;

    let funding_tx_path = secrets_dir.join("funding_tx_signed.json");
    let funding_info_path = funding_tx_path.to_str()
        .ok_or_else(|| anyhow!("invalid output path"))?;

    let user_addr_parsed = Address::from_str(user_address)
        .map_err(|e| anyhow!("invalid user address: {}", e))?;

    let (funding_tx_hash, funding_output_index) = if co_fund {
        // Co-fund mode: User + Merchant共同出资
        let merchant_addr = merchant_address.unwrap_or(&config.merchant.address);
        let merchant_addr_parsed = Address::from_str(merchant_addr)
            .map_err(|e| anyhow!("invalid merchant address: {}", e))?;

        build_cofund_funding_transaction(
            &config,
            &user_addr_parsed,
            &merchant_addr_parsed,
            capacity,
            &spillman_lock_script,
            funding_info_path,
        )
        .await?
    } else {
        // User-only funding mode
        build_funding_transaction(
            &config,
            &user_addr_parsed,
            &spillman_lock_script,
            capacity,
            funding_info_path,
        )
        .await?
    };

    // 6. Save channel info with actual funding tx info
    println!("\n💾 保存通道信息...");
    let channel_info = ChannelInfo {
        user_address: user_address.to_string(),
        merchant_address: merchant_address.unwrap_or(&config.merchant.address).to_string(),
        capacity_ckb: capacity,
        timeout_epochs,
        current_epoch: current_timestamp,  // Now storing timestamp instead of epoch
        timeout_epoch: timeout_timestamp,  // Now storing timestamp instead of epoch
        spillman_lock_script_hash: format!("{:#x}", script_hash),
        funding_tx_hash: format!("{:#x}", funding_tx_hash),
        funding_output_index,
    };

    let channel_info_json = serde_json::to_string_pretty(&channel_info)?;
    let channel_info_path = secrets_dir.join("channel_info.json");

    fs::write(&channel_info_path, channel_info_json)?;
    println!("✓ 通道信息已保存到: {}", channel_info_path.display());

    // 7. Build refund transaction template
    println!("\n📝 构建 Refund Transaction 模板...");
    println!("⚠️  Refund transaction 模板待实现");
    // TODO: build_refund_template(&config, &spillman_lock_script, capacity, timeout_timestamp)?;

    println!("\n✅ set-up 命令执行完成");
    println!("\n📌 下一步操作:");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("\n1️⃣  查看生成的文件:");
    println!("   - 已签名交易: {}", funding_tx_path.display());
    println!("   - 通道信息: {}", channel_info_path.display());
    println!("\n2️⃣  广播 funding transaction:");
    println!("   ckb-cli tx send --tx-file {}", funding_tx_path.display());
    println!("\n3️⃣  交易上链后即可开始使用:");
    println!("   spillman-cli pay --amount <CKB数量>");

    Ok(())
}

/// Execute setup command using funding_v2 (new implementation)
///
/// This is the v2 implementation using the refactored funding_v2 module.
/// The original execute() function above is kept as execute_v1 backup.
pub async fn execute_v2(
    config_path: &str,
    output_dir: &str,
    merchant_address: Option<&str>,
    capacity: Option<u64>,
    timeout_epochs: Option<u64>,
    co_fund: bool,
) -> Result<()> {
    println!("🚀 执行 set-up 命令 - 准备 Spillman Channel (v2)");
    println!("==========================================\n");

    // 1. Load configuration
    println!("📋 加载配置文件: {}", config_path);
    let config = load_config(config_path)?;
    println!("✓ 配置加载成功");

    // Use values from config file, allow CLI to override
    let user_address = &config.user.address;
    let capacity = capacity.unwrap_or(config.channel.capacity_ckb);
    let timeout_epochs = timeout_epochs.unwrap_or(config.channel.timeout_epochs);

    // 2. Parse user and merchant info
    println!("\n👤 解析用户和商户信息...");
    let user_privkey = parse_privkey(&config.user.private_key)?;
    let merchant_privkey = parse_privkey(&config.merchant.private_key)?;

    let user_pubkey = user_privkey.pubkey()?;
    let merchant_pubkey = merchant_privkey.pubkey()?;

    println!("✓ 用户地址: {}", user_address);
    println!("✓ 用户公钥: {}", hex::encode(user_pubkey.serialize()));
    println!("✓ 商户公钥: {}", hex::encode(merchant_pubkey.serialize()));

    if co_fund {
        println!("✓ 模式: Co-fund (User + Merchant 共同出资)");
    } else {
        println!("✓ 模式: User 单独出资");
    }

    // 3. Connect to CKB network
    println!("\n🔗 连接到 CKB 网络...");
    let rpc_client = CkbRpcClient::new(&config.network.rpc_url);

    // Get current timestamp and calculate timeout
    // Note: timeout_epochs is converted to seconds (1 epoch ≈ 4 hours = 14400 seconds)
    let current_timestamp = get_current_timestamp(&rpc_client).await?;
    let timeout_seconds = timeout_epochs * 14400; // 4 hours per epoch
    let timeout_timestamp = current_timestamp + timeout_seconds;

    println!("✓ RPC URL: {}", config.network.rpc_url);
    println!("✓ 当前时间戳: {} ({} UTC)", current_timestamp,
        chrono::DateTime::from_timestamp(current_timestamp as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "Invalid".to_string())
    );
    println!("✓ 超时时间戳: {} (+{} epochs ≈ {} hours)",
        timeout_timestamp, timeout_epochs, timeout_epochs * 4);
    println!("  超时时间: {}",
        chrono::DateTime::from_timestamp(timeout_timestamp as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Invalid".to_string())
    );

    // 4. Build Spillman Lock script
    println!("\n🔒 构建 Spillman Lock script...");
    let spillman_lock_script = build_spillman_lock_script(
        &config,
        &user_pubkey,
        &merchant_pubkey,
        timeout_timestamp,
    )?;

    let script_hash = spillman_lock_script.calc_script_hash();
    println!("✓ Spillman Lock script hash: {:#x}", script_hash);
    println!("✓ Lock script args 长度: {} bytes", spillman_lock_script.args().raw_data().len());

    // 5. Build and sign funding transaction
    println!("\n📝 构建并签名 Funding Transaction (v2)...");

    // Create output directory structure
    let output_path = std::path::Path::new(output_dir);
    let secrets_dir = output_path.join("secrets");
    fs::create_dir_all(&secrets_dir)?;

    let funding_tx_path = secrets_dir.join("funding_tx_signed.json");
    let funding_info_path = funding_tx_path.to_str()
        .ok_or_else(|| anyhow!("invalid output path"))?;

    let user_addr_parsed = Address::from_str(user_address)
        .map_err(|e| anyhow!("invalid user address: {}", e))?;

    // Convert capacity from CKB to HumanCapacity
    use ckb_sdk::HumanCapacity;
    let capacity_human = HumanCapacity::from_str(&capacity.to_string())
        .map_err(|e| anyhow!("Failed to parse capacity: {}", e))?;

    let (funding_tx_hash, funding_output_index) = if co_fund {
        // Co-fund mode: User + Merchant共同出资
        let merchant_addr = merchant_address.unwrap_or(&config.merchant.address);
        let merchant_addr_parsed = Address::from_str(merchant_addr)
            .map_err(|e| anyhow!("invalid merchant address: {}", e))?;

        funding_v2::build_cofund_funding_transaction(
            &config,
            &user_addr_parsed,
            &merchant_addr_parsed,
            capacity_human,
            &spillman_lock_script,
            funding_info_path,
        )
        .await?
    } else {
        // User-only funding mode
        funding_v2::build_funding_transaction(
            &config,
            &user_addr_parsed,
            &spillman_lock_script,
            capacity_human,
            funding_info_path,
        )
        .await?
    };

    // 6. Save channel info with actual funding tx info
    println!("\n💾 保存通道信息...");
    let channel_info = ChannelInfo {
        user_address: user_address.to_string(),
        merchant_address: merchant_address.unwrap_or(&config.merchant.address).to_string(),
        capacity_ckb: capacity,
        timeout_epochs,
        current_epoch: current_timestamp,  // Now storing timestamp instead of epoch
        timeout_epoch: timeout_timestamp,  // Now storing timestamp instead of epoch
        spillman_lock_script_hash: format!("{:#x}", script_hash),
        funding_tx_hash: format!("{:#x}", funding_tx_hash),
        funding_output_index,
    };

    let channel_info_json = serde_json::to_string_pretty(&channel_info)?;
    let channel_info_path = secrets_dir.join("channel_info.json");

    fs::write(&channel_info_path, channel_info_json)?;
    println!("✓ 通道信息已保存到: {}", channel_info_path.display());

    // 7. Build refund transaction template
    println!("\n📝 构建 Refund Transaction 模板...");
    println!("⚠️  Refund transaction 模板待实现");
    // TODO: build_refund_template(&config, &spillman_lock_script, capacity, timeout_timestamp)?;

    println!("\n✅ set-up 命令执行完成 (v2)");
    println!("\n📌 下一步操作:");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("\n1️⃣  查看生成的文件:");
    println!("   - 已签名交易: {}", funding_tx_path.display());
    println!("   - 通道信息: {}", channel_info_path.display());
    println!("\n2️⃣  广播 funding transaction:");
    println!("   ckb-cli tx send --tx-file {}", funding_tx_path.display());
    println!("\n3️⃣  交易上链后即可开始使用:");
    println!("   spillman-cli pay --amount <CKB数量>");

    Ok(())
}
