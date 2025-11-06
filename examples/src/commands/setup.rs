use anyhow::{anyhow, Result};
use ckb_sdk::{rpc::CkbRpcClient, Address};
use serde::{Deserialize, Serialize};
use std::fs;
use std::str::FromStr;

use crate::tx_builder::funding::{build_cofund_funding_transaction, build_funding_transaction};
use crate::tx_builder::funding_v2;
use crate::tx_builder::spillman_lock::{build_spillman_lock_script, build_spillman_lock_script_with_hash};
use crate::utils::config::load_config;
use crate::utils::crypto::parse_privkey;

#[derive(Debug, Serialize, Deserialize)]
struct ChannelInfo {
    user_address: String,
    merchant_address: String,
    capacity_ckb: u64,
    timeout_epochs: u64, // Deprecated, keeping for backwards compatibility
    current_timestamp: u64,
    timeout_timestamp: u64,
    spillman_lock_script_hash: String,
    funding_tx_hash: String,
    funding_output_index: u32,
    // xUDT fields (optional, only present in xUDT channels)
    #[serde(skip_serializing_if = "Option::is_none")]
    xudt_type_script: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    xudt_amount: Option<String>,  // Store as string to avoid u128 parsing issues
}

pub async fn execute(
    config_path: &str,
    output_dir: &str,
    merchant_address: Option<&str>,
    capacity: Option<u64>,
    timeout_timestamp: Option<u64>,
    fee_rate: u64,
    co_fund: bool,
    broadcast: bool,
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
    let timeout_timestamp = timeout_timestamp.unwrap_or(config.channel.timeout_timestamp);

    // 2. Parse user and merchant info
    println!("\n👤 解析用户和商户信息...");

    // Parse user (must be single-sig for now)
    let user_privkey = parse_privkey(config.user.private_key.as_ref().expect("User private_key is required"))?;
    let user_pubkey = user_privkey.pubkey()?;

    println!("✓ 用户地址: {}", user_address);
    println!("✓ 用户公钥: {}", hex::encode(user_pubkey.serialize()));

    // Parse merchant (can be single-sig or multisig)
    let merchant_pubkey_hash = if config.merchant.is_multisig() {
        // Multisig: use blake160(multisig_config) as lock arg
        println!("✓ 商户模式: 多签 ({}-of-{})",
            config.merchant.multisig_threshold.unwrap(),
            config.merchant.multisig_total.unwrap()
        );

        let merchant_secret_keys = config.merchant.get_secret_keys()?;
        let (threshold, total) = config.merchant.get_multisig_config()
            .ok_or_else(|| anyhow!("Merchant multisig config is invalid"))?;

        // Build multisig config
        use crate::tx_builder::funding_v2::build_multisig_config;
        let multisig_config = build_multisig_config(&merchant_secret_keys, threshold, total)?;

        // Calculate blake160(multisig_config) for lock arg
        use ckb_hash::blake2b_256;
        let config_bytes = multisig_config.to_witness_data();
        let multisig_lock_arg = &blake2b_256(&config_bytes)[0..20];

        println!("✓ 商户多签 lock arg: {}", hex::encode(multisig_lock_arg));
        multisig_lock_arg.to_vec()
    } else {
        // Single-sig: use pubkey_hash(merchant_pubkey)
        println!("✓ 商户模式: 单签");
        let merchant_privkey = parse_privkey(config.merchant.private_key.as_ref().expect("Merchant private_key is required"))?;
        let merchant_pubkey = merchant_privkey.pubkey()?;
        println!("✓ 商户公钥: {}", hex::encode(merchant_pubkey.serialize()));

        use crate::utils::crypto::pubkey_hash;
        pubkey_hash(&merchant_pubkey).to_vec()
    };

    if co_fund {
        println!("✓ 模式: Co-fund (User + Merchant 共同出资)");
    } else {
        println!("✓ 模式: User 单独出资");
    }

    // 3. Connect to CKB network
    println!("\n🔗 连接到 CKB 网络...");
    let rpc_client = CkbRpcClient::new(&config.network.rpc_url);

    // Get current timestamp from system time
    let current_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| anyhow!("Failed to get system time: {}", e))?
        .as_secs();

    println!("✓ RPC URL: {}", config.network.rpc_url);
    println!("✓ 当前时间戳: {} ({} UTC)", current_timestamp,
        chrono::DateTime::from_timestamp(current_timestamp as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "Invalid".to_string())
    );
    println!("✓ 超时时间戳: {}", timeout_timestamp);
    println!("  超时时间: {}",
        chrono::DateTime::from_timestamp(timeout_timestamp as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Invalid".to_string())
    );

    // Validate timeout timestamp must be at least 20 minutes (1200 seconds) in the future
    let min_timeout = current_timestamp + 1200; // 20 minutes = 1200 seconds
    if timeout_timestamp < min_timeout {
        return Err(anyhow!(
            "超时时间戳必须大于当前时间至少 20 分钟！\n\
             当前时间戳: {}\n\
             最小超时时间戳: {} (当前时间 + 20 分钟)\n\
             您设置的超时时间戳: {}",
            current_timestamp,
            min_timeout,
            timeout_timestamp
        ));
    }
    println!("✓ 超时时间验证通过 (距离当前时间 {} 秒 ≈ {} 分钟)",
        timeout_timestamp - current_timestamp,
        (timeout_timestamp - current_timestamp) / 60
    );

    // 4. Build Spillman Lock script
    println!("\n🔒 构建 Spillman Lock script...");
    let spillman_lock_script = build_spillman_lock_script_with_hash(
        &config,
        &user_pubkey,
        &merchant_pubkey_hash,
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
            fee_rate,
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
            fee_rate,
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
        timeout_epochs: 0, // Deprecated, keeping for backwards compatibility
        current_timestamp,
        timeout_timestamp,
        spillman_lock_script_hash: format!("{:#x}", script_hash),
        funding_tx_hash: format!("{:#x}", funding_tx_hash),
        funding_output_index,
        xudt_type_script: None,  // TODO: Will be filled in xUDT mode
        xudt_amount: None,       // TODO: Will be filled in xUDT mode
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
    timeout_timestamp: Option<u64>,
    fee_rate: u64,
    co_fund: bool,
    broadcast: bool,
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
    let timeout_timestamp = timeout_timestamp.unwrap_or(config.channel.timeout_timestamp);

    // 2. Parse user and merchant info
    println!("\n👤 解析用户和商户信息...");

    // Parse user (must be single-sig for now)
    let user_privkey = parse_privkey(config.user.private_key.as_ref().expect("User private_key is required"))?;
    let user_pubkey = user_privkey.pubkey()?;

    println!("✓ 用户地址: {}", user_address);
    println!("✓ 用户公钥: {}", hex::encode(user_pubkey.serialize()));

    // Parse merchant (can be single-sig or multisig)
    let merchant_pubkey_hash = if config.merchant.is_multisig() {
        // Multisig: use blake160(multisig_config) as lock arg
        println!("✓ 商户模式: 多签 ({}-of-{})",
            config.merchant.multisig_threshold.unwrap(),
            config.merchant.multisig_total.unwrap()
        );

        let merchant_secret_keys = config.merchant.get_secret_keys()?;
        let (threshold, total) = config.merchant.get_multisig_config()
            .ok_or_else(|| anyhow!("Merchant multisig config is invalid"))?;

        // Build multisig config
        use crate::tx_builder::funding_v2::build_multisig_config;
        let multisig_config = build_multisig_config(&merchant_secret_keys, threshold, total)?;

        // Calculate blake160(multisig_config) for lock arg
        use ckb_hash::blake2b_256;
        let config_bytes = multisig_config.to_witness_data();
        let multisig_lock_arg = &blake2b_256(&config_bytes)[0..20];

        println!("✓ 商户多签 lock arg: {}", hex::encode(multisig_lock_arg));
        multisig_lock_arg.to_vec()
    } else {
        // Single-sig: use pubkey_hash(merchant_pubkey)
        println!("✓ 商户模式: 单签");
        let merchant_privkey = parse_privkey(config.merchant.private_key.as_ref().expect("Merchant private_key is required"))?;
        let merchant_pubkey = merchant_privkey.pubkey()?;
        println!("✓ 商户公钥: {}", hex::encode(merchant_pubkey.serialize()));

        use crate::utils::crypto::pubkey_hash;
        pubkey_hash(&merchant_pubkey).to_vec()
    };

    if co_fund {
        println!("✓ 模式: Co-fund (User + Merchant 共同出资)");
    } else {
        println!("✓ 模式: User 单独出资");
    }

    // 3. Connect to CKB network
    println!("\n🔗 连接到 CKB 网络...");
    let rpc_client = CkbRpcClient::new(&config.network.rpc_url);

    // Get current timestamp from system time
    let current_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| anyhow!("Failed to get system time: {}", e))?
        .as_secs();

    println!("✓ RPC URL: {}", config.network.rpc_url);
    println!("✓ 当前时间戳: {} ({} UTC)", current_timestamp,
        chrono::DateTime::from_timestamp(current_timestamp as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "Invalid".to_string())
    );
    println!("✓ 超时时间戳: {}", timeout_timestamp);
    println!("  超时时间: {}",
        chrono::DateTime::from_timestamp(timeout_timestamp as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Invalid".to_string())
    );

    // Validate timeout timestamp must be at least 20 minutes (1200 seconds) in the future
    // let min_timeout = current_timestamp + 1200; // 20 minutes = 1200 seconds
    // if timeout_timestamp < min_timeout {
    //     return Err(anyhow!(
    //         "超时时间戳必须大于当前时间至少 20 分钟！\n\
    //          当前时间戳: {}\n\
    //          最小超时时间戳: {} (当前时间 + 20 分钟)\n\
    //          您设置的超时时间戳: {}",
    //         current_timestamp,
    //         min_timeout,
    //         timeout_timestamp
    //     ));
    // }
    // println!("✓ 超时时间验证通过 (距离当前时间 {} 秒 ≈ {} 分钟)",
    //     timeout_timestamp - current_timestamp,
    //     (timeout_timestamp - current_timestamp) / 60
    // );

    // 4. Build Spillman Lock script
    println!("\n🔒 构建 Spillman Lock script...");
    let spillman_lock_script = build_spillman_lock_script_with_hash(
        &config,
        &user_pubkey,
        &merchant_pubkey_hash,
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
            fee_rate,
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
            fee_rate,
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
        timeout_epochs: 0, // Deprecated, keeping for backwards compatibility
        current_timestamp,
        timeout_timestamp,
        spillman_lock_script_hash: format!("{:#x}", script_hash),
        funding_tx_hash: format!("{:#x}", funding_tx_hash),
        funding_output_index,
        xudt_type_script: None,  // TODO: Will be filled in xUDT mode
        xudt_amount: None,       // TODO: Will be filled in xUDT mode
    };

    let channel_info_json = serde_json::to_string_pretty(&channel_info)?;
    let channel_info_path = secrets_dir.join("channel_info.json");

    fs::write(&channel_info_path, channel_info_json)?;
    println!("✓ 通道信息已保存到: {}", channel_info_path.display());

    // 7. Broadcast funding transaction (optional)
    if broadcast {
        println!("\n📡 广播 Funding Transaction 到链上...");

        // Load the saved transaction (TransactionView with hash)
        let funding_tx_json_str = fs::read_to_string(&funding_tx_path)?;
        let funding_tx_json: ckb_jsonrpc_types::TransactionView = serde_json::from_str(&funding_tx_json_str)?;

        // Send transaction via RPC (use inner Transaction without hash)
        let rpc_client = ckb_sdk::rpc::CkbRpcClient::new(&config.network.rpc_url);
        let broadcast_tx_hash = rpc_client
            .send_transaction(funding_tx_json.inner, None)
            .map_err(|e| anyhow!("Failed to broadcast transaction: {:?}", e))?;

        println!("✓ Funding Transaction 已广播");
        println!("  - TX Hash: {:#x}", broadcast_tx_hash);

        // 8. Build refund transaction template
        println!("\n📝 构建 Refund Transaction 模板...");
        println!("⚠️  Refund transaction 模板待实现");
        // TODO: build_refund_template(&config, &spillman_lock_script, capacity, timeout_timestamp)?;

        println!("\n✅ 通道创建成功 (v2)");
        println!("\n📌 下一步操作:");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("\n🔍 查询交易状态:");
        println!("   ckb-cli rpc get_transaction --hash {:#x}", broadcast_tx_hash);
        println!("\n⏳ 等待交易上链确认...");
        println!("   交易确认后即可开始支付");
        println!("\n💸 创建支付:");
        println!("   spillman-cli pay --amount <CKB数量> --channel-file {}", channel_info_path.display());
    } else {
        // 8. Build refund transaction template
        println!("\n📝 构建 Refund Transaction 模板...");
        println!("⚠️  Refund transaction 模板待实现");
        // TODO: build_refund_template(&config, &spillman_lock_script, capacity, timeout_timestamp)?;

        println!("\n✅ 通道创建成功 (v2) - 交易未广播");
        println!("\n📌 下一步操作:");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("\n📄 生成的文件:");
        println!("   - 已签名交易: {}", funding_tx_path.display());
        println!("   - 通道信息: {}", channel_info_path.display());
        println!("\n📡 手动广播交易:");
        println!("   spillman-cli set-up --use-v2 --broadcast ... (重新运行带 --broadcast)");
        println!("   或者使用其他工具手动发送交易");
    }

    Ok(())
}
