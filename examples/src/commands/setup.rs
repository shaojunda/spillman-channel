use anyhow::{anyhow, Result};
use ckb_sdk::rpc::CkbRpcClient;
use serde::{Deserialize, Serialize};
use std::fs;

use crate::tx_builder::spillman_lock::build_spillman_lock_script;
use crate::utils::config::load_config;
use crate::utils::crypto::parse_privkey;
use crate::utils::rpc::get_current_epoch;

#[derive(Debug, Serialize, Deserialize)]
struct ChannelInfo {
    user_address: String,
    merchant_address: String,
    capacity_ckb: u64,
    timeout_epochs: u64,
    current_epoch: u64,
    timeout_epoch: u64,
    spillman_lock_script_hash: String,
}

pub async fn execute(
    user_address: &str,
    merchant_address: Option<&str>,
    capacity: u64,
    timeout_epochs: u64,
    config_path: &str,
) -> Result<()> {
    println!("🚀 执行 set-up 命令 - 准备 Spillman Channel");
    println!("==========================================\n");

    // 1. Load configuration
    println!("📋 加载配置文件: {}", config_path);
    let config = load_config(config_path)?;
    println!("✓ 配置加载成功");

    // 2. Parse user and merchant info
    println!("\n👤 解析用户和商户信息...");
    let user_privkey = parse_privkey(&config.user.private_key)?;
    let merchant_privkey = parse_privkey(&config.merchant.private_key)?;

    let user_pubkey = user_privkey.pubkey()?;
    let merchant_pubkey = merchant_privkey.pubkey()?;

    println!("✓ 用户地址: {}", user_address);
    println!("✓ 用户公钥: {}", hex::encode(user_pubkey.serialize()));

    if let Some(merchant) = merchant_address {
        println!("✓ 商户地址: {} (共同出资模式)", merchant);
    } else {
        println!("✓ 模式: 用户单独出资");
    }
    println!("✓ 商户公钥: {}", hex::encode(merchant_pubkey.serialize()));

    // 3. Connect to CKB network
    println!("\n🔗 连接到 CKB 网络...");
    let rpc_client = CkbRpcClient::new(&config.network.rpc_url);

    // Get current epoch
    let current_epoch = get_current_epoch(&rpc_client).await?;
    let timeout_epoch = current_epoch + timeout_epochs;

    println!("✓ RPC URL: {}", config.network.rpc_url);
    println!("✓ 当前 epoch: {}", current_epoch);
    println!("✓ 超时 epoch: {} (+{} epochs)", timeout_epoch, timeout_epochs);

    // 4. Build Spillman Lock script
    println!("\n🔒 构建 Spillman Lock script...");
    let spillman_lock_script = build_spillman_lock_script(
        &config,
        &user_pubkey,
        &merchant_pubkey,
        timeout_epoch,
    )?;

    let script_hash = spillman_lock_script.calc_script_hash();
    println!("✓ Spillman Lock script hash: {:#x}", script_hash);
    println!("✓ Lock script args 长度: {} bytes", spillman_lock_script.args().raw_data().len());

    // 5. Save channel info
    println!("\n💾 保存通道信息...");
    let channel_info = ChannelInfo {
        user_address: user_address.to_string(),
        merchant_address: merchant_address.unwrap_or(&config.merchant.address).to_string(),
        capacity_ckb: capacity,
        timeout_epochs,
        current_epoch,
        timeout_epoch,
        spillman_lock_script_hash: format!("{:#x}", script_hash),
    };

    let channel_info_json = serde_json::to_string_pretty(&channel_info)?;
    let channel_info_path = "examples/secrets/channel_info.json";
    fs::write(channel_info_path, channel_info_json)?;
    println!("✓ 通道信息已保存到: {}", channel_info_path);

    // 6. TODO: Build funding transaction
    println!("\n📝 构建 funding transaction...");
    println!("⚠️  功能待实现: 需要收集 cells 并构造交易");
    println!("   - 使用 DefaultCellCollector 收集用户的 live cells");
    println!("   - 使用 CapacityTransferBuilder 构造交易");
    println!("   - 创建 Spillman Lock cell 作为输出");

    // 7. TODO: Build refund transaction
    println!("\n📝 构建 refund transaction...");
    println!("⚠️  功能待实现: 需要构造退款交易");
    println!("   - Input: Spillman Lock cell (尚不存在，需要引用 funding tx)");
    println!("   - Output: 用户地址（全额退款）");
    println!("   - 用户先签名，保存到文件供商户签名");

    println!("\n✅ set-up 命令执行完成");
    println!("\n📌 下一步:");
    println!("   1. 商户需要为 refund transaction 签名");
    println!("   2. 用户确认后广播 funding transaction");
    println!("   3. 通道创建完成，可以开始支付");

    Ok(())
}
