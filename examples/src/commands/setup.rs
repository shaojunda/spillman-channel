use anyhow::{anyhow, Result};
use ckb_sdk::{
    constants::SIGHASH_TYPE_HASH,
    rpc::CkbRpcClient,
    traits::{
        DefaultCellCollector, DefaultCellDepResolver, DefaultHeaderDepResolver,
        DefaultTransactionDependencyProvider, SecpCkbRawKeySigner,
    },
    tx_builder::{transfer::CapacityTransferBuilder, CapacityBalancer, TxBuilder},
    unlock::{ScriptUnlocker, SecpSighashUnlocker},
    Address, ScriptId,
};
use ckb_types::{
    bytes::Bytes,
    core::BlockView,
    packed::{CellOutput, Script, WitnessArgs},
    prelude::*,
    H256,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::str::FromStr;

use crate::tx_builder::spillman_lock::build_spillman_lock_script;
use crate::utils::config::{load_config, Config};
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
    funding_tx_hash: String,
    funding_output_index: u32,
}

pub async fn execute(
    config_path: &str,
    output_dir: &str,
    merchant_address: Option<&str>,
    capacity: Option<u64>,
    timeout_epochs: Option<u64>,
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
    let (funding_tx_hash, funding_output_index) = build_funding_transaction(
        &config,
        &user_addr_parsed,
        &spillman_lock_script,
        capacity,
        funding_info_path,
    )
    .await?;

    // 6. Save channel info with actual funding tx info
    println!("\n💾 保存通道信息...");
    let channel_info = ChannelInfo {
        user_address: user_address.to_string(),
        merchant_address: merchant_address.unwrap_or(&config.merchant.address).to_string(),
        capacity_ckb: capacity,
        timeout_epochs,
        current_epoch,
        timeout_epoch,
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
    // TODO: build_refund_template(&config, &spillman_lock_script, capacity, timeout_epoch)?;

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

async fn build_funding_transaction(
    config: &Config,
    user_address: &Address,
    spillman_lock_script: &Script,
    capacity_ckb: u64,
    output_path: &str,
) -> Result<(ckb_types::H256, u32)> {
    let capacity_shannon = capacity_ckb * 100_000_000;

    println!("  - Spillman Lock cell capacity: {} CKB ({} shannon)", capacity_ckb, capacity_shannon);

    // Setup providers from RPC
    let ckb_client = CkbRpcClient::new(&config.network.rpc_url);

    let cell_dep_resolver = {
        let genesis_block = ckb_client.get_block_by_number(0.into())?.unwrap();
        DefaultCellDepResolver::from_genesis(&BlockView::from(genesis_block))?
    };
    let header_dep_resolver = DefaultHeaderDepResolver::new(&config.network.rpc_url);
    let mut cell_collector = DefaultCellCollector::new(&config.network.rpc_url);
    let tx_dep_provider = DefaultTransactionDependencyProvider::new(&config.network.rpc_url, 10);

    // Build sender lock script from user address
    let sender = Script::from(user_address);

    // Build ScriptUnlocker for signing
    // Convert Privkey to secp256k1::SecretKey
    // We need to re-parse the private key from the config
    let privkey_hex = &config.user.private_key;
    let privkey_hex_trimmed = privkey_hex.trim_start_matches("0x");
    let privkey_bytes = hex::decode(privkey_hex_trimmed)
        .map_err(|e| anyhow!("failed to decode private key hex: {}", e))?;
    let sender_key = secp256k1::SecretKey::from_slice(&privkey_bytes)
        .map_err(|e| anyhow!("invalid user private key: {}", e))?;
    let signer = SecpCkbRawKeySigner::new_with_secret_keys(vec![sender_key]);
    let sighash_unlocker = SecpSighashUnlocker::from(Box::new(signer) as Box<_>);
    let sighash_script_id = ScriptId::new_type(SIGHASH_TYPE_HASH.clone());
    let mut unlockers = HashMap::default();
    unlockers.insert(
        sighash_script_id,
        Box::new(sighash_unlocker) as Box<dyn ScriptUnlocker>,
    );

    // Build CapacityBalancer
    let placeholder_witness = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(vec![0u8; 65])).pack())
        .build();
    let balancer = CapacityBalancer::new_simple(sender, placeholder_witness, 1000);

    // Build Spillman Lock cell output
    let spillman_cell = CellOutput::new_builder()
        .capacity((capacity_shannon as u64).pack())
        .lock(spillman_lock_script.clone())
        .build();

    // Build the transaction
    println!("  - 收集用户的 live cells 并构建交易...");
    let builder = CapacityTransferBuilder::new(vec![(spillman_cell, Bytes::default())]);
    let (tx, still_locked_groups) = builder.build_unlocked(
        &mut cell_collector,
        &cell_dep_resolver,
        &header_dep_resolver,
        &tx_dep_provider,
        &balancer,
        &unlockers,
    )?;

    if !still_locked_groups.is_empty() {
        return Err(anyhow!("Some script groups are still locked: {:?}", still_locked_groups));
    }

    let tx_hash = tx.hash();
    println!("✓ 交易已构建并签名");
    println!("  - Transaction hash: {:#x}", tx_hash);
    println!("  - Inputs 数量: {}", tx.inputs().len());
    println!("  - Outputs 数量: {}", tx.outputs().len());

    // Save signed transaction
    let tx_json = ckb_jsonrpc_types::TransactionView::from(tx);
    let json_str = serde_json::to_string_pretty(&tx_json)?;

    if let Some(parent) = std::path::Path::new(output_path).parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, json_str)?;

    println!("✓ 已签名的 Funding transaction 已保存: {}", output_path);

    // Return tx_hash and output_index (Spillman Lock cell is always at index 0)
    Ok((tx_hash.unpack(), 0))
}

// TODO: Implement refund template builder
// fn build_refund_template(...) -> Result<()> { ... }
