use anyhow::{anyhow, Result};
use ckb_crypto::secp::{Privkey, Pubkey};
use ckb_hash::blake2b_256;
use ckb_sdk::{
    rpc::CkbRpcClient,
    Address,
};
use ckb_types::{
    bytes::Bytes,
    core::ScriptHashType,
    packed,
    prelude::*,
    H256,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::str::FromStr;

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    network: NetworkConfig,
    user: KeyConfig,
    merchant: KeyConfig,
    channel: ChannelConfig,
    spillman_lock: SpillmanLockConfig,
    auth: AuthConfig,
}

#[derive(Debug, Deserialize, Serialize)]
struct NetworkConfig {
    rpc_url: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct KeyConfig {
    private_key: String,
    address: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ChannelConfig {
    capacity_ckb: u64,
    timeout_epochs: u64,
    tx_fee_shannon: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct SpillmanLockConfig {
    code_hash: String,
    hash_type: String,
    tx_hash: String,
    index: u32,
}

#[derive(Debug, Deserialize, Serialize)]
struct AuthConfig {
    tx_hash: String,
    index: u32,
}

/// Spillman Lock Args structure (49 bytes)
#[derive(Debug, Clone)]
struct SpillmanLockArgs {
    merchant_pubkey_hash: [u8; 20],
    user_pubkey_hash: [u8; 20],
    timeout_epoch: u64,
    version: u8,
}

impl SpillmanLockArgs {
    fn new(merchant_pubkey_hash: [u8; 20], user_pubkey_hash: [u8; 20], timeout_epoch: u64) -> Self {
        Self {
            merchant_pubkey_hash,
            user_pubkey_hash,
            timeout_epoch,
            version: 0,
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(49);
        bytes.extend_from_slice(&self.merchant_pubkey_hash);
        bytes.extend_from_slice(&self.user_pubkey_hash);
        bytes.extend_from_slice(&self.timeout_epoch.to_le_bytes());
        bytes.push(self.version);
        bytes
    }
}

/// Helper function to calculate pubkey hash (Blake2b-160)
fn pubkey_hash(pubkey: &Pubkey) -> [u8; 20] {
    let pubkey_bytes = pubkey.serialize();
    let hash = blake2b_256(&pubkey_bytes);
    let mut result = [0u8; 20];
    result.copy_from_slice(&hash[0..20]);
    result
}

/// Load configuration from config.toml
fn load_config() -> Result<Config> {
    let config_path = "examples/config.toml";
    let config_str = fs::read_to_string(config_path)
        .map_err(|_| anyhow!("Failed to read config file: {}", config_path))?;
    let config: Config = toml::from_str(&config_str)?;
    Ok(config)
}

/// Parse private key from hex string
fn parse_privkey(hex: &str) -> Result<Privkey> {
    let hex = hex.trim_start_matches("0x");
    let bytes = hex::decode(hex)?;
    if bytes.len() != 32 {
        return Err(anyhow!("Invalid private key length: expected 32 bytes, got {}", bytes.len()));
    }
    Ok(Privkey::from_slice(&bytes))
}

/// Get current epoch from CKB node
async fn get_current_epoch(rpc_client: &CkbRpcClient) -> Result<u64> {
    let epoch = rpc_client.get_current_epoch()?;
    Ok(epoch.number.into())
}

// NOTE: For full implementation, use ckb-sdk-rust components:
// - DefaultCellCollector: Collect user's live cells
// - CapacityTransferBuilder: Build funding transaction
// - CapacityBalancer: Calculate fees and balance transaction
// - SecpCkbRawKeySigner: Sign transaction with private key
// - SecpSighashUnlocker: Unlock cells with signatures
//
// See examples: https://github.com/nervosnetwork/ckb-sdk-rust/tree/master/examples
// - transfer_from_sighash.rs: Basic transfer example
// - send_ckb_example.rs: Complete send example

/// Build Spillman Lock script
fn build_spillman_lock_script(
    config: &Config,
    user_pubkey: &Pubkey,
    merchant_pubkey: &Pubkey,
    timeout_epoch: u64,
) -> Result<packed::Script> {
    let user_pubkey_hash = pubkey_hash(user_pubkey);
    let merchant_pubkey_hash = pubkey_hash(merchant_pubkey);

    let args = SpillmanLockArgs::new(merchant_pubkey_hash, user_pubkey_hash, timeout_epoch);
    let args_bytes = args.to_bytes();

    let code_hash = H256::from_str(&config.spillman_lock.code_hash)
        .map_err(|_| anyhow!("Invalid code hash"))?;
    let hash_type = match config.spillman_lock.hash_type.as_str() {
        "data" => ScriptHashType::Data,
        "type" => ScriptHashType::Type,
        "data1" => ScriptHashType::Data1,
        "data2" => ScriptHashType::Data2,
        _ => return Err(anyhow!("Invalid hash type")),
    };

    let hash_type_byte: packed::Byte = hash_type.into();
    Ok(packed::Script::new_builder()
        .code_hash(code_hash.pack())
        .hash_type(hash_type_byte)
        .args(Bytes::from(args_bytes).pack())
        .build())
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("🚀 Spillman Channel Full Flow Example");
    println!("======================================\n");

    // Load configuration
    println!("📋 Loading configuration...");
    let config = load_config()?;
    println!("✓ Configuration loaded");

    // Parse private keys
    let user_privkey = parse_privkey(&config.user.private_key)?;
    let merchant_privkey = parse_privkey(&config.merchant.private_key)?;

    // Derive public keys
    let user_pubkey = user_privkey.pubkey()?;
    let merchant_pubkey = merchant_privkey.pubkey()?;

    println!("✓ User pubkey: {}", hex::encode(user_pubkey.serialize()));
    println!("✓ Merchant pubkey: {}", hex::encode(merchant_pubkey.serialize()));

    // Connect to CKB RPC
    println!("\n🔗 Connecting to CKB network...");
    let rpc_client = CkbRpcClient::new(&config.network.rpc_url);

    // Get current epoch
    let current_epoch = get_current_epoch(&rpc_client).await?;
    let timeout_epoch = current_epoch + config.channel.timeout_epochs;

    println!("✓ Connected to {}", config.network.rpc_url);
    println!("✓ Current epoch: {}", current_epoch);
    println!("✓ Timeout epoch: {} (+{} epochs)", timeout_epoch, config.channel.timeout_epochs);

    // Parse addresses
    let user_address = Address::from_str(&config.user.address)
        .map_err(|e| anyhow!("Invalid user address: {}", e))?;
    let merchant_address = Address::from_str(&config.merchant.address)
        .map_err(|e| anyhow!("Invalid merchant address: {}", e))?;

    println!("\n👤 User address: {}", user_address);
    println!("🏪 Merchant address: {}", merchant_address);

    // Build Spillman Lock script
    println!("\n🔐 Building Spillman Lock script...");
    let spillman_lock_script = build_spillman_lock_script(
        &config,
        &user_pubkey,
        &merchant_pubkey,
        timeout_epoch,
    )?;
    println!("✓ Spillman Lock script created");
    println!("  Code hash: {}", hex::encode(spillman_lock_script.code_hash().as_slice()));
    println!("  Args: {}", hex::encode(spillman_lock_script.args().raw_data()));

    // ====================
    // Phase 1: Channel Setup
    // ====================
    println!("\n📝 Phase 1: Channel Setup");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Calculate required capacity (channel capacity + tx fee)
    let channel_capacity_shannon = config.channel.capacity_ckb * 100_000_000;
    let required_capacity = channel_capacity_shannon + config.channel.tx_fee_shannon;

    println!("\n💰 Channel parameters:");
    println!("  Capacity: {} CKB", config.channel.capacity_ckb);
    println!("  Fee: {} CKB", config.channel.tx_fee_shannon / 100_000_000);
    println!("  Total required: {} CKB", required_capacity / 100_000_000);

    println!("\n🔨 Step 1: Constructing refund transaction...");
    println!("(In real implementation, user would construct refund tx first)");
    println!("(Merchant would sign it before funding tx is broadcast)");
    println!("✓ Refund transaction prepared (merchant pre-signed)");

    println!("\n📤 Step 2: Broadcasting funding transaction...");
    println!("(In this example, we'll show the structure but not broadcast)");

    // Build funding transaction structure (示例 - 实际需要完整实现)
    println!("\nFunding Transaction Structure:");
    println!("  Inputs:");
    println!("    [0] User's Cell - 1500 CKB (example)");
    println!("  Outputs:");
    println!("    [0] Spillman Lock Cell - {} CKB", channel_capacity_shannon / 100_000_000);
    println!("    [1] User Change Cell - (remaining) CKB");

    // ====================
    // Phase 2: Off-chain Payments
    // ====================
    println!("\n📝 Phase 2: Off-chain Payments (Commitment Transactions)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let payments = vec![
        (100, 900),  // Payment 1: 100 CKB to merchant, 900 CKB change to user
        (300, 700),  // Payment 2: 300 CKB to merchant, 700 CKB change to user
        (500, 500),  // Payment 3: 500 CKB to merchant, 500 CKB change to user
    ];

    for (idx, (to_merchant, to_user)) in payments.iter().enumerate() {
        println!("\n💳 Commitment Transaction #{}", idx + 1);
        println!("  Input: Spillman Lock Cell - {} CKB", config.channel.capacity_ckb);
        println!("  Outputs:");
        println!("    [0] User address - {} CKB (change)", to_user);
        println!("    [1] Merchant address - {} CKB (payment)", to_merchant);
        println!("  Witness: User signature ✓");
        println!("  Status: Signed by user, held by merchant (off-chain)");
    }

    println!("\n✓ Merchant holds the latest commitment (500 CKB payment)");

    // ====================
    // Phase 3: Settlement
    // ====================
    println!("\n📝 Phase 3: Settlement");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    println!("\n🏪 Option A: Merchant Settlement (Normal Case)");
    println!("  1. Merchant takes the latest commitment (500 CKB)");
    println!("  2. Merchant adds their signature");
    println!("  3. Merchant broadcasts to CKB network");
    println!("  Result:");
    println!("    - Merchant receives: 500 CKB");
    println!("    - User receives: 500 CKB (change)");

    println!("\n⏰ Option B: User Refund (Timeout Case)");
    println!("  Conditions:");
    println!("    - Current epoch >= timeout epoch ({})", timeout_epoch);
    println!("    - Merchant did not settle");
    println!("  Steps:");
    println!("    1. User waits for timeout");
    println!("    2. User broadcasts pre-signed refund transaction");
    println!("    3. User adds their signature");
    println!("  Result:");
    println!("    - User receives: {} CKB (full refund)", config.channel.capacity_ckb);
    println!("    - Merchant receives: 0 CKB (loses all income)");

    // Summary
    println!("\n📊 Summary");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("✓ Channel capacity: {} CKB", config.channel.capacity_ckb);
    println!("✓ Total payments made: 3 commitments");
    println!("✓ Final state: 500 CKB to merchant, 500 CKB to user");
    println!("✓ Timeout protection: {} epochs", config.channel.timeout_epochs);

    println!("\n🎉 Spillman Channel flow completed successfully!");
    println!("\n⚠️  Note: This is a demonstration of the flow structure.");
    println!("   To run on testnet, you need to:");
    println!("   1. Deploy the Spillman Lock contract");
    println!("   2. Update config.toml with actual keys and addresses");
    println!("   3. Ensure user has sufficient CKB balance on testnet");
    println!("   4. Implement actual transaction signing and broadcasting");

    Ok(())
}
