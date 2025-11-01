use anyhow::{anyhow, Result};
use ckb_sdk::{
    constants::SIGHASH_TYPE_HASH,
    rpc::CkbRpcClient,
    traits::{
        CellCollector, CellDepResolver, CellQueryOptions, DefaultCellCollector, DefaultCellDepResolver,
        DefaultHeaderDepResolver, DefaultTransactionDependencyProvider, SecpCkbRawKeySigner,
        ValueRangeOption,
    },
    transaction::builder::FeeCalculator,
    tx_builder::{transfer::CapacityTransferBuilder, unlock_tx, CapacityBalancer, TxBuilder},
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
use std::collections::HashMap;
use std::fs;

use crate::utils::config::Config;

/// Build complete funding transaction with inputs and signatures
///
/// This function:
/// - Collects user's live cells as inputs
/// - Creates Spillman Lock cell as output
/// - Calculates and adds change output
/// - Signs transaction with user's private key
/// - Saves signed transaction to file
///
/// Returns: (tx_hash, output_index) where output_index is the Spillman Lock cell index
pub async fn build_funding_transaction(
    config: &Config,
    user_address: &Address,
    spillman_lock_script: &Script,
    capacity_ckb: u64,
    output_path: &str,
) -> Result<(H256, u32)> {
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
        .capacity(capacity_shannon)
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

/// Build co-fund funding transaction where both user and merchant contribute
///
/// This function:
/// - User contributes the specified capacity
/// - Merchant contributes minimum cell occupied capacity
/// - Manually collects cells from both user and merchant addresses
/// - Creates Spillman Lock cell as output with combined capacity
/// - Signs transaction with both private keys
/// - Saves signed transaction to file
///
/// Returns: (tx_hash, output_index) where output_index is the Spillman Lock cell index
pub async fn build_cofund_funding_transaction(
    config: &Config,
    user_address: &Address,
    merchant_address: &Address,
    user_capacity_ckb: u64,
    spillman_lock_script: &Script,
    output_path: &str,
) -> Result<(H256, u32)> {
    use ckb_types::{
        core::TransactionBuilder,
        packed::CellInput,
    };

    println!("  - Co-fund 模式：User + Merchant 共同出资");

    let user_capacity_shannon = user_capacity_ckb * 100_000_000;

    // Calculate merchant's minimum occupied capacity
    let merchant_lock = Script::from(merchant_address);
    let temp_merchant_cell = CellOutput::new_builder()
        .capacity(0u64)
        .lock(merchant_lock.clone())
        .build();

    let merchant_capacity_shannon = temp_merchant_cell
        .occupied_capacity(ckb_types::core::Capacity::bytes(0).unwrap())
        .unwrap()
        .as_u64();

    // User adds extra 1 CKB as buffer (for fees, etc.)
    let user_buffer_shannon = 1 * 100_000_000;

    println!("  - User 需出资: {} CKB + 1 CKB buffer", user_capacity_ckb);
    println!("  - Merchant 需出资: {} CKB (最小占用)", merchant_capacity_shannon / 100_000_000);

    // Setup providers from RPC
    let ckb_client = CkbRpcClient::new(&config.network.rpc_url);

    let cell_dep_resolver = {
        let genesis_block = ckb_client.get_block_by_number(0.into())?.unwrap();
        DefaultCellDepResolver::from_genesis(&BlockView::from(genesis_block))?
    };
    let _header_dep_resolver = DefaultHeaderDepResolver::new(&config.network.rpc_url);
    let mut cell_collector = DefaultCellCollector::new(&config.network.rpc_url);
    let tx_dep_provider = DefaultTransactionDependencyProvider::new(&config.network.rpc_url, 10);

    // Parse private keys
    let user_privkey_hex = &config.user.private_key;
    let user_privkey_hex_trimmed = user_privkey_hex.trim_start_matches("0x");
    let user_privkey_bytes = hex::decode(user_privkey_hex_trimmed)
        .map_err(|e| anyhow!("failed to decode user private key hex: {}", e))?;
    let user_key = secp256k1::SecretKey::from_slice(&user_privkey_bytes)
        .map_err(|e| anyhow!("invalid user private key: {}", e))?;

    let merchant_privkey_hex = &config.merchant.private_key;
    let merchant_privkey_hex_trimmed = merchant_privkey_hex.trim_start_matches("0x");
    let merchant_privkey_bytes = hex::decode(merchant_privkey_hex_trimmed)
        .map_err(|e| anyhow!("failed to decode merchant private key hex: {}", e))?;
    let merchant_key = secp256k1::SecretKey::from_slice(&merchant_privkey_bytes)
        .map_err(|e| anyhow!("invalid merchant private key: {}", e))?;

    // Step 1: Manually collect cells from user's address
    println!("  - 收集 User 的 cells...");
    let user_lock = Script::from(user_address);
    let mut user_query = CellQueryOptions::new_lock(user_lock.clone());
    // Filter: only collect cells WITHOUT type script (plain CKB cells)
    // Set secondary_script_len_range to 0 to filter out cells with type script
    user_query.secondary_script_len_range = Some(ValueRangeOption::new_exact(0));
    user_query.data_len_range = Some(ValueRangeOption::new_exact(0));

    let mut user_query_with_capacity = user_query.clone();
    user_query_with_capacity.min_total_capacity = user_capacity_shannon + user_buffer_shannon;

    println!("  - 正在查询 cells（过滤掉 UDT/NFT cells）...");
    let (user_cells, _total_user_capacity) = cell_collector
        .collect_live_cells(&user_query_with_capacity, false)?;

    if user_cells.is_empty() {
        return Err(anyhow!("User 没有任何可用的 live cells"));
    }

    let user_input_capacity: u64 = user_cells.iter().map(|c| Unpack::<u64>::unpack(&c.output.capacity())).sum();
    let user_required_capacity = user_capacity_shannon + user_buffer_shannon;

    println!(
        "  - 收集到 {} 个 User cells，总容量: {} CKB",
        user_cells.len(),
        user_input_capacity / 100_000_000
    );

    // Verify user has enough capacity
    if user_input_capacity < user_required_capacity {
        return Err(anyhow!(
            "User 容量不足：需要 {} CKB，实际只有 {} CKB",
            user_required_capacity as f64 / 100_000_000.0,
            user_input_capacity as f64 / 100_000_000.0
        ));
    }

    // Step 2: Manually collect cells from merchant's address
    println!("  - 收集 Merchant 的 cells...");
    let mut merchant_query = CellQueryOptions::new_lock(merchant_lock.clone());
    // Filter: only collect cells WITHOUT type script (plain CKB cells)
    // Set secondary_script_len_range to 0 to filter out cells with type script
    merchant_query.secondary_script_len_range = Some(ValueRangeOption::new_exact(0));
    merchant_query.data_len_range = Some(ValueRangeOption::new_exact(0));

    let mut merchant_query_with_capacity = merchant_query.clone();
    merchant_query_with_capacity.min_total_capacity = merchant_capacity_shannon;

    println!("  - 正在查询 Merchant cells（过滤掉 UDT/NFT cells）...");
    let (merchant_cells, _total_merchant_capacity) = cell_collector
        .collect_live_cells(&merchant_query_with_capacity, false)?;

    if merchant_cells.is_empty() {
        return Err(anyhow!("Merchant 没有任何可用的 live cells"));
    }

    let merchant_input_capacity: u64 = merchant_cells
        .iter()
        .map(|c| Unpack::<u64>::unpack(&c.output.capacity()))
        .sum();

    println!(
        "  - 收集到 {} 个 Merchant cells，总容量: {} CKB",
        merchant_cells.len(),
        merchant_input_capacity / 100_000_000
    );

    // Verify merchant has enough capacity
    if merchant_input_capacity < merchant_capacity_shannon {
        return Err(anyhow!(
            "Merchant 容量不足：需要 {} CKB，实际只有 {} CKB",
            merchant_capacity_shannon as f64 / 100_000_000.0,
            merchant_input_capacity as f64 / 100_000_000.0
        ));
    }

    // Step 3: Build transaction inputs from collected cells
    let mut inputs = Vec::new();
    for cell in &user_cells {
        inputs.push(CellInput::new(cell.out_point.clone(), 0));
    }
    for cell in &merchant_cells {
        inputs.push(CellInput::new(cell.out_point.clone(), 0));
    }

    let total_input_capacity = user_input_capacity + merchant_input_capacity;
    let spillman_capacity = user_capacity_shannon + merchant_capacity_shannon + user_buffer_shannon;

    println!(
        "  - 总 Input 容量: {} CKB",
        total_input_capacity / 100_000_000
    );
    println!(
        "  - Spillman Lock cell 容量: {} CKB",
        spillman_capacity / 100_000_000
    );

    // Step 4: Iteratively build transaction with accurate fee calculation
    // We need to iterate because the transaction size (and thus fee) depends on
    // whether we have a change output or not

    // Fee rate: 1000 shannon/KB
    let fee_rate = 1000u64;
    let fee_calculator = FeeCalculator::new(fee_rate);

    // Get cell deps first (needed for transaction building)
    let sighash_dep = cell_dep_resolver
        .resolve(&user_lock)
        .ok_or_else(|| anyhow!("Failed to resolve sighash cell dep"))?;

    // Minimum change cell capacity for both user and merchant
    let min_user_change = CellOutput::new_builder()
        .capacity(0u64)
        .lock(user_lock.clone())
        .build()
        .occupied_capacity(ckb_types::core::Capacity::bytes(0).unwrap())
        .unwrap()
        .as_u64();

    let min_merchant_change = CellOutput::new_builder()
        .capacity(0u64)
        .lock(merchant_lock.clone())
        .build()
        .occupied_capacity(ckb_types::core::Capacity::bytes(0).unwrap())
        .unwrap()
        .as_u64();

    // Build Spillman Lock output
    let spillman_cell = CellOutput::new_builder()
        .capacity(spillman_capacity)
        .lock(spillman_lock_script.clone())
        .build();

    // Helper function to build transaction with given change capacities
    let build_tx = |user_change_opt: Option<u64>, merchant_change_opt: Option<u64>| {
        let mut builder = TransactionBuilder::default();

        // Add inputs
        for cell in &user_cells {
            builder = builder.input(CellInput::new(cell.out_point.clone(), 0));
        }
        for cell in &merchant_cells {
            builder = builder.input(CellInput::new(cell.out_point.clone(), 0));
        }

        // Add Spillman Lock output
        builder = builder
            .output(spillman_cell.clone())
            .output_data(Bytes::new().pack());

        // Add user change output if capacity is sufficient
        if let Some(change_cap) = user_change_opt {
            if change_cap >= min_user_change {
                let change_cell = CellOutput::new_builder()
                    .capacity(change_cap)
                    .lock(user_lock.clone())
                    .build();
                builder = builder.output(change_cell).output_data(Bytes::new().pack());
            }
        }

        // Add merchant change output if capacity is sufficient
        if let Some(change_cap) = merchant_change_opt {
            if change_cap >= min_merchant_change {
                let change_cell = CellOutput::new_builder()
                    .capacity(change_cap)
                    .lock(merchant_lock.clone())
                    .build();
                builder = builder.output(change_cell).output_data(Bytes::new().pack());
            }
        }

        // Add cell deps
        builder = builder.cell_dep(sighash_dep.clone());

        // Add witnesses placeholders with correct size
        // WitnessArgs with a 65-byte dummy signature in lock field
        // This ensures the transaction size calculation includes the signature overhead
        let dummy_signature = vec![0u8; 65];
        let witness_args = ckb_types::packed::WitnessArgs::new_builder()
            .lock(Some(Bytes::from(dummy_signature)).pack())
            .build();

        let witness_count = user_cells.len() + merchant_cells.len();
        for _ in 0..witness_count {
            builder = builder.witness(witness_args.as_bytes().pack());
        }

        builder.build()
    };

    // Helper function to calculate fee from a transaction
    let calculate_tx_fee = |tx: &ckb_types::core::TransactionView| -> u64 {
        let tx_size = tx.data().as_reader().serialized_size_in_block() as u64;
        fee_calculator.fee(tx_size)
    };

    // Step 4: Iteratively calculate fee until stable
    // Fee strategy: User pays all transaction fees (as initiator)
    let max_iterations = 10;
    let mut current_fee = 0u64;

    println!("  - User 需要贡献: {} CKB (含 buffer)", (user_capacity_shannon + user_buffer_shannon) as f64 / 100_000_000.0);
    println!("  - Merchant 需要贡献: {} CKB", merchant_capacity_shannon as f64 / 100_000_000.0);

    // Merchant change is fixed (doesn't depend on fee)
    let merchant_change_opt = {
        let merchant_available = merchant_input_capacity
            .checked_sub(merchant_capacity_shannon);

        match merchant_available {
            Some(change) if change >= min_merchant_change => Some(change),
            Some(change) if change > 0 => {
                println!("  - ℹ️  Merchant 找零太小 ({} shannon)，将并入交易", change);
                None
            }
            _ => None,
        }
    };

    let mut final_user_change_opt: Option<u64> = None;
    let mut final_tx: Option<ckb_types::core::TransactionView> = None;

    for iteration in 0..max_iterations {
        // Calculate user change based on current fee estimate
        let user_change_opt = {
            let user_available = user_input_capacity
                .checked_sub(user_capacity_shannon)
                .and_then(|c| c.checked_sub(user_buffer_shannon))
                .and_then(|c| c.checked_sub(current_fee));

            match user_available {
                Some(change) if change >= min_user_change => Some(change),
                Some(change) if change > 0 => {
                    if iteration == 0 {
                        println!("  - ℹ️  User 找零太小 ({} shannon)，将作为手续费", change);
                    }
                    None
                }
                _ => {
                    return Err(anyhow!(
                        "User 容量不足支付手续费: input={} CKB, required={} CKB, fee={} shannon",
                        user_input_capacity as f64 / 100_000_000.0,
                        (user_capacity_shannon + user_buffer_shannon) as f64 / 100_000_000.0,
                        current_fee
                    ));
                }
            }
        };

        // Build transaction with calculated changes
        let temp_tx = build_tx(user_change_opt, merchant_change_opt);

        // Calculate ACTUAL fee for this transaction (including all outputs and witnesses)
        let actual_fee = calculate_tx_fee(&temp_tx);

        if iteration == 0 {
            println!("  - 初始手续费估算: {} shannon ({} CKB)", actual_fee, actual_fee as f64 / 100_000_000.0);
        }

        // Check if fee has stabilized
        if actual_fee == current_fee {
            println!("  - 手续费已稳定: {} shannon ({} CKB) (迭代 {} 次)", actual_fee, actual_fee as f64 / 100_000_000.0, iteration + 1);
            final_user_change_opt = user_change_opt;
            final_tx = Some(temp_tx);
            break;
        }

        // Update fee for next iteration
        current_fee = actual_fee;

        if iteration == max_iterations - 1 {
            println!("  - ⚠️  达到最大迭代次数，使用最后计算的手续费: {} shannon", current_fee);
            final_user_change_opt = user_change_opt;
            final_tx = Some(temp_tx);
        }
    }

    let tx = final_tx.ok_or_else(|| anyhow!("Failed to build transaction"))?;

    // Calculate and print final fee
    let final_fee = calculate_tx_fee(&tx);
    println!("\n  📊 最终交易费用统计:");
    println!("    - 交易大小: {} bytes", tx.data().as_reader().serialized_size_in_block());
    println!("    - 手续费率: {} shannon/KB", fee_rate);
    println!("    - 最终手续费: {} shannon ({} CKB)", final_fee, final_fee as f64 / 100_000_000.0);

    // Verify against node's minimum fee requirement
    let min_required_fee = 630u64; // CKB node's min fee requirement
    if final_fee < min_required_fee {
        println!("    - ⚠️  警告: 手续费 ({} shannon) 低于节点最低要求 ({} shannon)", final_fee, min_required_fee);
        println!("    - 💡 建议: 提高 fee_rate 或增加交易复杂度");
    }

    println!("\n  💰 找零统计:");
    if let Some(user_change) = final_user_change_opt {
        println!("    - User 找零: {} CKB", user_change as f64 / 100_000_000.0);
    } else {
        println!("    - User 无找零（全部用于 Spillman cell 和手续费）");
    }
    if let Some(merchant_change) = merchant_change_opt {
        println!("    - Merchant 找零: {} CKB", merchant_change as f64 / 100_000_000.0);
    } else {
        println!("    - Merchant 无找零（全部用于 Spillman cell）");
    }

    // Step 6: Sign transaction
    let signer = SecpCkbRawKeySigner::new_with_secret_keys(vec![user_key, merchant_key]);
    let sighash_unlocker = SecpSighashUnlocker::from(Box::new(signer) as Box<_>);
    let sighash_script_id = ScriptId::new_type(SIGHASH_TYPE_HASH.clone());
    let mut unlockers = HashMap::default();
    unlockers.insert(
        sighash_script_id,
        Box::new(sighash_unlocker) as Box<dyn ScriptUnlocker>,
    );

    let (signed_tx, still_locked_groups) = unlock_tx(tx, &tx_dep_provider, &unlockers)?;

    if !still_locked_groups.is_empty() {
        return Err(anyhow!(
            "Some script groups are still locked: {:?}",
            still_locked_groups
        ));
    }

    let tx_hash = signed_tx.hash();
    println!("✓ Co-fund 交易已构建并签名");
    println!("  - Transaction hash: {:#x}", tx_hash);
    println!("  - Inputs 数量: {}", signed_tx.inputs().len());
    println!(
        "    - User inputs: {} 个",
        user_cells.len()
    );
    println!(
        "    - Merchant inputs: {} 个",
        merchant_cells.len()
    );
    println!("  - Outputs 数量: {}", signed_tx.outputs().len());

    // Verify capacity balance
    let total_output: u64 = signed_tx
        .outputs()
        .into_iter()
        .map(|o| Unpack::<u64>::unpack(&o.capacity()))
        .sum();
    let fee = total_input_capacity - total_output;
    println!("  - 手续费: {} shannon ({} CKB)", fee, fee as f64 / 100_000_000.0);

    // Save signed transaction
    let tx_json = ckb_jsonrpc_types::TransactionView::from(signed_tx);
    let json_str = serde_json::to_string_pretty(&tx_json)?;

    if let Some(parent) = std::path::Path::new(output_path).parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, json_str)?;

    println!("✓ 已签名的 Co-fund Funding transaction 已保存: {}", output_path);

    // Return tx_hash and output_index (Spillman Lock cell is always at index 0)
    Ok((tx_hash.unpack(), 0))
}

