/// Refactored funding transaction builder following Fiber's TxBuilder pattern
///
/// Key improvements over v1:
/// - Uses TxBuilder trait for structured transaction construction
/// - Separates concerns: build_base -> balance -> sign
/// - Supports incremental construction (co-funding)
/// - Uses CapacityBalancer for automatic fee calculation
/// - Cleaner, more maintainable code structure
///
/// # Co-Funding Example
///
/// The builder supports incremental construction for co-funding scenarios:
///
/// ```ignore
/// // Step 1: User builds initial funding transaction with main capacity
/// let user_request = FundingRequest {
///     script: funding_lock.clone(),
///     local_amount: 1000_00000000 + 1_00000000, // User's 1000 CKB + 1 CKB buffer
///     fee_rate: 1000,
/// };
///
/// let user_tx = FundingTx::new()
///     .build(user_request, user_context)
///     .await?;
///
/// // User sends the transaction to Merchant
///
/// // Step 2: Merchant adds their minimum occupied capacity on top
/// let merchant_request = FundingRequest {
///     script: funding_lock.clone(),
///     local_amount: 61_00000000,   // Merchant's minimum occupied (61 CKB)
///     fee_rate: 1000,
/// };
///
/// let final_tx = user_tx  // Start from user's tx
///     .build(merchant_request, merchant_context)
///     .await?;
///
/// // The final funding cell will contain 1062 CKB (1001 + 61)
/// ```
///
/// The magic happens in `build_funding_cell()`: it checks if `outputs` is empty to
/// determine if this is the first party (user) or second party (merchant), and
/// adjusts the funding cell capacity accordingly.

use anyhow::{anyhow, Result};
use ckb_sdk::{
    constants::{ONE_CKB, SIGHASH_TYPE_HASH},
    rpc::CkbRpcClient,
    traits::{
        CellCollector, CellDepResolver, DefaultCellCollector,
        DefaultCellDepResolver, DefaultHeaderDepResolver, DefaultTransactionDependencyProvider,
        HeaderDepResolver, SecpCkbRawKeySigner, TransactionDependencyProvider,
    },
    tx_builder::{unlock_tx, CapacityBalancer, TxBuilder, TxBuilderError},
    unlock::{ScriptUnlocker, SecpSighashUnlocker},
    Address, HumanCapacity, ScriptId,
};
use ckb_types::{
    core::{BlockView, Capacity, TransactionView},
    packed::{CellOutput, Script, Transaction, WitnessArgs},
    prelude::*,
    H256,
};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use crate::utils::config::Config;

/// Funding request parameters
pub struct FundingRequest {
    /// The funding cell lock script
    pub script: Script,
    /// Local party's capacity to fund (in shannons)
    pub local_amount: u64,
    /// Fee rate in shannon/KB
    pub fee_rate: u64,
}

/// Funding context (keys and RPC)
#[derive(Clone)]
pub struct FundingContext {
    pub secret_key: secp256k1::SecretKey,
    pub rpc_url: String,
    pub funding_source_lock_script: Script,
}

/// Funding transaction wrapper
#[derive(Clone, Debug, Default)]
pub struct FundingTx {
    tx: Option<TransactionView>,
}

impl FundingTx {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take(&mut self) -> Option<TransactionView> {
        self.tx.take()
    }

    pub fn as_ref(&self) -> Option<&TransactionView> {
        self.tx.as_ref()
    }

    pub fn into_inner(self) -> Option<TransactionView> {
        self.tx
    }

    pub fn update(&mut self, tx: TransactionView) {
        self.tx = Some(tx);
    }

    /// Build the funding transaction
    pub async fn build(
        self,
        request: FundingRequest,
        context: FundingContext,
    ) -> Result<Self> {
        let builder = FundingTxBuilder {
            funding_tx: self,
            request,
            context,
        };
        builder.build_internal(true).await
    }

    /// Build the funding transaction without signing (for co-funding)
    pub async fn build_without_sign(
        self,
        request: FundingRequest,
        context: FundingContext,
    ) -> Result<Self> {
        let builder = FundingTxBuilder {
            funding_tx: self,
            request,
            context,
        };
        builder.build_internal(false).await
    }

    /// Sign the funding transaction
    pub async fn sign(mut self, secret_key: secp256k1::SecretKey, rpc_url: String) -> Result<Self> {
        let signer = SecpCkbRawKeySigner::new_with_secret_keys(vec![secret_key]);
        let sighash_unlocker = SecpSighashUnlocker::from(Box::new(signer) as Box<_>);
        let sighash_script_id = ScriptId::new_type(SIGHASH_TYPE_HASH.clone());
        let mut unlockers = HashMap::default();
        unlockers.insert(
            sighash_script_id,
            Box::new(sighash_unlocker) as Box<dyn ScriptUnlocker>,
        );

        let tx = self.take().ok_or_else(|| anyhow!("No transaction to sign"))?;
        let tx_dep_provider = DefaultTransactionDependencyProvider::new(&rpc_url, 10);

        let (tx, still_locked_groups) = unlock_tx(tx, &tx_dep_provider, &unlockers)?;
        if !still_locked_groups.is_empty() {
            return Err(anyhow!("Some script groups are still locked: {:?}", still_locked_groups));
        }

        self.update(tx);
        Ok(self)
    }

    /// Sign the funding transaction with multiple keys (for co-funding)
    pub async fn sign_with_multiple_keys(mut self, secret_keys: Vec<secp256k1::SecretKey>, rpc_url: String) -> Result<Self> {
        let signer = SecpCkbRawKeySigner::new_with_secret_keys(secret_keys);
        let sighash_unlocker = SecpSighashUnlocker::from(Box::new(signer) as Box<_>);
        let sighash_script_id = ScriptId::new_type(SIGHASH_TYPE_HASH.clone());
        let mut unlockers = HashMap::default();
        unlockers.insert(
            sighash_script_id,
            Box::new(sighash_unlocker) as Box<dyn ScriptUnlocker>,
        );

        let tx = self.take().ok_or_else(|| anyhow!("No transaction to sign"))?;
        let tx_dep_provider = DefaultTransactionDependencyProvider::new(&rpc_url, 10);

        let (tx, still_locked_groups) = unlock_tx(tx, &tx_dep_provider, &unlockers)?;
        if !still_locked_groups.is_empty() {
            return Err(anyhow!("Some script groups are still locked: {:?}", still_locked_groups));
        }

        self.update(tx);
        Ok(self)
    }
}

impl From<TransactionView> for FundingTx {
    fn from(tx: TransactionView) -> Self {
        Self { tx: Some(tx) }
    }
}

impl From<Transaction> for FundingTx {
    fn from(tx: Transaction) -> Self {
        Self {
            tx: Some(tx.into_view()),
        }
    }
}

/// Internal builder implementing TxBuilder trait
struct FundingTxBuilder {
    funding_tx: FundingTx,
    request: FundingRequest,
    context: FundingContext,
}

#[async_trait::async_trait]
impl TxBuilder for FundingTxBuilder {
    async fn build_base_async(
        &self,
        _cell_collector: &mut dyn CellCollector,
        _cell_dep_resolver: &dyn CellDepResolver,
        _header_dep_resolver: &dyn HeaderDepResolver,
        _tx_dep_provider: &dyn TransactionDependencyProvider,
    ) -> Result<TransactionView, TxBuilderError> {
        let funding_cell_output = self.build_funding_cell();

        let mut inputs = vec![];
        let mut cell_deps = HashSet::new();
        let mut witnesses = vec![];

        // Funding cell output
        let mut outputs: Vec<CellOutput> = vec![funding_cell_output];
        let mut outputs_data: Vec<ckb_types::packed::Bytes> = vec![Default::default()];

        // If there's an existing transaction, preserve its structure
        if let Some(ref tx) = self.funding_tx.tx {
            inputs = tx.inputs().into_iter().collect();
            cell_deps = tx.cell_deps().into_iter().collect();
            witnesses = tx.witnesses().into_iter().collect();

            // Preserve other outputs (e.g., change outputs)
            for (i, output) in tx.outputs().into_iter().enumerate().skip(1) {
                outputs.push(output.clone());
                outputs_data.push(tx.outputs_data().get(i).unwrap_or_default().clone());
            }
        }

        let builder = match self.funding_tx.tx {
            Some(ref tx) => tx.as_advanced_builder(),
            None => Transaction::default().as_advanced_builder(),
        };

        // Set a placeholder witness for fee calculation (only for new transactions)
        // Using 65 bytes for single-sig (170 for multisig would be more conservative)
        if witnesses.is_empty() {
            let placeholder_witness = WitnessArgs::new_builder()
                .lock(Some(molecule::bytes::Bytes::from(vec![0u8; 65])).pack())
                .build();
            witnesses.push(placeholder_witness.as_bytes().pack());
        }

        let tx_builder = builder
            .set_inputs(inputs)
            .set_outputs(outputs)
            .set_outputs_data(outputs_data)
            .set_cell_deps(cell_deps.into_iter().collect())
            .set_witnesses(witnesses);

        let tx = tx_builder.build();
        Ok(tx)
    }
}

impl FundingTxBuilder {
    /// Build the funding cell output
    ///
    /// This method implements incremental construction for co-funding:
    /// - If outputs is empty (first party): funding cell contains only local_amount
    /// - If outputs is not empty (second party): funding cell = existing capacity + local_amount
    fn build_funding_cell(&self) -> CellOutput {
        let total_capacity = if let Some(tx) = self.funding_tx.tx.as_ref() {
            if let Some(existing_output) = tx.outputs().get(0) {
                // Second party: add to existing funding cell capacity
                let existing_capacity: u64 = existing_output.capacity().unpack();
                existing_capacity
                    .checked_add(self.request.local_amount)
                    .expect("Capacity overflow")
            } else {
                // First party: use local_amount only
                self.request.local_amount
            }
        } else {
            // No transaction yet: use local_amount only
            self.request.local_amount
        };

        CellOutput::new_builder()
            .capacity(Capacity::shannons(total_capacity))
            .lock(self.request.script.clone())
            .build()
    }

    /// Internal build method that orchestrates the entire build process
    ///
    /// # Arguments
    /// * `should_sign` - Whether to sign the transaction immediately
    async fn build_internal(self, should_sign: bool) -> Result<FundingTx> {
        // Step 1: Create unlockers with the secret key from context
        let signer = SecpCkbRawKeySigner::new_with_secret_keys(vec![self.context.secret_key.clone()]);
        let sighash_unlocker = SecpSighashUnlocker::from(Box::new(signer) as Box<_>);
        let sighash_script_id = ScriptId::new_type(SIGHASH_TYPE_HASH.clone());
        let mut unlockers = HashMap::default();
        unlockers.insert(
            sighash_script_id,
            Box::new(sighash_unlocker) as Box<dyn ScriptUnlocker>,
        );

        let sender = self.context.funding_source_lock_script.clone();

        // Step 2: Create capacity balancer
        let placeholder_witness = WitnessArgs::new_builder()
            .lock(Some(molecule::bytes::Bytes::from(vec![0u8; 65])).pack())
            .build();

        let mut balancer = CapacityBalancer::new_simple(
            sender.clone(),
            placeholder_witness,
            self.request.fee_rate,
        );

        // Step 3: Setup providers
        let ckb_client = CkbRpcClient::new(&self.context.rpc_url);
        let cell_dep_resolver = {
            match ckb_client.get_block_by_number(0.into())? {
                Some(genesis_block) => {
                    DefaultCellDepResolver::from_genesis(&BlockView::from(genesis_block))?
                }
                None => {
                    return Err(anyhow!("Failed to get genesis block"));
                }
            }
        };

        let header_dep_resolver = DefaultHeaderDepResolver::new(&self.context.rpc_url);
        let mut cell_collector = DefaultCellCollector::new(&self.context.rpc_url);
        let tx_dep_provider = DefaultTransactionDependencyProvider::new(&self.context.rpc_url, 10);

        // Step 4: Build transaction
        let is_incremental = self.funding_tx.tx.is_some();

        let tx = if !should_sign {
            // Build without signing (for co-funding - sign later with all keys)
            let base_tx = self.build_base_async(
                &mut cell_collector,
                &cell_dep_resolver,
                &header_dep_resolver,
                &tx_dep_provider,
            ).await?;

            // Balance the transaction (add inputs for this party)
            balancer.balance_tx_capacity(&base_tx, &mut cell_collector, &tx_dep_provider, &cell_dep_resolver, &header_dep_resolver)?
        } else if is_incremental {
            // Incremental construction: build and balance, but preserve existing signatures
            let base_tx = self.build_base_async(
                &mut cell_collector,
                &cell_dep_resolver,
                &header_dep_resolver,
                &tx_dep_provider,
            ).await?;

            // Balance the transaction (add inputs for this party)
            let balanced_tx = balancer.balance_tx_capacity(&base_tx, &mut cell_collector, &tx_dep_provider, &cell_dep_resolver, &header_dep_resolver)?;

            // Unlock only the NEW inputs added by this party
            // Get the number of existing inputs from the original transaction
            let existing_input_count = self.funding_tx.tx.as_ref().map(|tx| tx.inputs().len()).unwrap_or(0);
            let existing_witnesses = self.funding_tx.tx.as_ref().map(|tx| tx.witnesses()).unwrap_or_default();
            let existing_witnesses_len = existing_witnesses.len();

            // Sign the new transaction
            let (signed_tx, still_locked) = unlock_tx(balanced_tx, &tx_dep_provider, &unlockers)?;

            // Check if only new inputs are locked (existing inputs should remain signed)
            let new_locked_groups: Vec<_> = still_locked.iter()
                .filter(|g| g.input_indices.iter().any(|&idx| idx >= existing_input_count))
                .collect();

            if !new_locked_groups.is_empty() {
                return Err(anyhow!("Some NEW script groups are still locked: {:?}", new_locked_groups));
            }

            // Preserve existing witnesses for existing inputs
            let builder = signed_tx.as_advanced_builder();
            let mut all_witnesses: Vec<_> = existing_witnesses.into_iter().collect();

            // Append new witnesses for new inputs
            let new_witnesses: Vec<_> = signed_tx.witnesses().into_iter().skip(existing_witnesses_len).collect();
            all_witnesses.extend(new_witnesses);

            builder.set_witnesses(all_witnesses).build().into()
        } else {
            // First party: normal build_unlocked
            let (tx, still_locked_groups) = self.build_unlocked(
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

            tx
        };

        let mut funding_tx = self.funding_tx;
        funding_tx.update(tx);

        Ok(funding_tx)
    }
}

/// Build complete funding transaction (high-level API) - Single party funding
///
/// This function:
/// - Creates FundingRequest and FundingContext
/// - Builds the transaction using TxBuilder pattern
/// - Signs the transaction
/// - Saves to file
///
/// Returns: (tx_hash, output_index)
///
/// # Arguments
/// * `user_address` - The user's address
/// * `spillman_lock_script` - The funding cell lock script
/// * `capacity` - Capacity (supports both u64 shannon and HumanCapacity)
///   - Can be created from u64: `HumanCapacity::from(10000000000)`
///   - Can be parsed from string: `HumanCapacity::from_str("100.5")?`
/// * `output_path` - Path to save the signed transaction JSON
///
/// # Examples
/// ```ignore
/// // From u64 (shannon)
/// build_funding_transaction(config, addr, script, 100_00000000.into(), path).await?;
///
/// // From string (CKB)
/// let capacity = HumanCapacity::from_str("100.5")?;
/// build_funding_transaction(config, addr, script, capacity, path).await?;
/// ```
pub async fn build_funding_transaction(
    config: &Config,
    user_address: &Address,
    spillman_lock_script: &Script,
    capacity: HumanCapacity,
    output_path: &str,
) -> Result<(H256, u32)> {
    let capacity_shannon: u64 = capacity.into();

    println!(
        "  - Spillman Lock cell capacity: {} ({} shannon)",
        capacity,
        capacity_shannon
    );

    // Parse user private key
    let privkey_hex = config.user.private_key.trim_start_matches("0x");
    let privkey_bytes = hex::decode(privkey_hex)?;
    let secret_key = secp256k1::SecretKey::from_slice(&privkey_bytes)?;

    // Create funding request (single-party funding, remote_amount = 0)
    let request = FundingRequest {
        script: spillman_lock_script.clone(),
        local_amount: capacity_shannon,
        fee_rate: 1000, // 1000 shannon/KB
    };

    // Create funding context
    let user_lock = Script::from(user_address);
    let context = FundingContext {
        secret_key: secret_key.clone(),
        rpc_url: config.network.rpc_url.clone(),
        funding_source_lock_script: user_lock,
    };

    // Build and sign transaction
    println!("  - Building and signing funding transaction...");
    let signed_tx = FundingTx::new()
        .build(request, context.clone())
        .await?;

    let tx = signed_tx.into_inner().ok_or_else(|| anyhow!("No transaction"))?;
    let tx_hash = tx.hash();

    println!("✓ Transaction built and signed");
    println!("  - Transaction hash: {:#x}", tx_hash);
    println!("  - Inputs count: {}", tx.inputs().len());
    println!("  - Outputs count: {}", tx.outputs().len());

    // Calculate fee
    let total_input: u64 = {
        let ckb_client = CkbRpcClient::new(&context.rpc_url);
        let mut total = 0u64;
        for input in tx.input_pts_iter() {
            if let Ok(cell_with_status) = ckb_client.get_live_cell(input.into(), false) {
                if let Some(cell) = cell_with_status.cell {
                    let capacity: u64 = cell.output.capacity.into();
                    total += capacity;
                }
            }
        }
        total
    };

    let total_output: u64 = tx
        .outputs()
        .into_iter()
        .map(|o| Unpack::<u64>::unpack(&o.capacity()))
        .sum();

    let fee = total_input.saturating_sub(total_output);
    println!("  - Fee: {} ({} shannon)", HumanCapacity::from(fee), fee);

    // Save transaction
    let tx_json = ckb_jsonrpc_types::TransactionView::from(tx);
    let json_str = serde_json::to_string_pretty(&tx_json)?;

    if let Some(parent) = std::path::Path::new(output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, json_str)?;

    println!("✓ Signed funding transaction saved: {}", output_path);

    // Return tx_hash and output_index (funding cell is always at index 0)
    Ok((tx_hash.unpack(), 0))
}

/// Build co-funding transaction (high-level API) - Two party funding
///
/// This implements the incremental construction pattern:
/// 1. User builds initial funding transaction with their funds (main capacity)
/// 2. Merchant adds their funds on top of user's transaction (minimum occupied capacity only)
///
/// Note: User contributes the main capacity, Merchant only contributes the minimum occupied capacity
///
/// Returns: (tx_hash, output_index)
///
/// # Arguments
/// * `user_address` - The user's address
/// * `merchant_address` - The merchant's address
/// * `user_capacity` - User's capacity to fund (main capacity)
/// * `spillman_lock_script` - The funding cell lock script
/// * `output_path` - Path to save the signed transaction JSON
///
/// # Examples
/// ```ignore
/// // From string
/// let capacity = HumanCapacity::from_str("1000")?; // User contributes 1000 CKB (main capacity)
/// // Final funding cell will be: 1000 (user) + 1 (buffer) + 61 (merchant min) = 1062 CKB
/// build_cofund_funding_transaction(config, user_addr, merchant_addr, capacity, script, path).await?;
/// ```
pub async fn build_cofund_funding_transaction(
    config: &Config,
    user_address: &Address,
    merchant_address: &Address,
    user_capacity: HumanCapacity,
    spillman_lock_script: &Script,
    output_path: &str,
) -> Result<(H256, u32)> {
    println!("  - Co-fund 模式：User + Merchant 共同出资");

    let user_capacity_shannon: u64 = user_capacity.into();

    // Calculate merchant's minimum occupied capacity
    let merchant_lock = Script::from(merchant_address);
    let temp_merchant_cell = CellOutput::new_builder()
        .capacity(0u64)
        .lock(merchant_lock.clone())
        .build();

    let merchant_capacity_shannon = temp_merchant_cell
        .occupied_capacity(Capacity::bytes(0).unwrap())
        .unwrap()
        .as_u64();

    // User adds extra 1 CKB as buffer (for fees, etc.)
    let user_buffer_shannon = 1 * ONE_CKB;

    let user_amount = user_capacity_shannon + user_buffer_shannon;
    let merchant_amount = merchant_capacity_shannon;

    println!("  - User 需出资: {} + {} buffer", user_capacity, HumanCapacity::from(user_buffer_shannon));
    println!("  - Merchant 需出资: {} (最小占用)", HumanCapacity::from(merchant_capacity_shannon));

    // Parse keys
    let user_privkey_hex = config.user.private_key.trim_start_matches("0x");
    let user_privkey_bytes = hex::decode(user_privkey_hex)?;
    let user_secret_key = secp256k1::SecretKey::from_slice(&user_privkey_bytes)?;

    let merchant_privkey_hex = config.merchant.private_key.trim_start_matches("0x");
    let merchant_privkey_bytes = hex::decode(merchant_privkey_hex)?;
    let merchant_secret_key = secp256k1::SecretKey::from_slice(&merchant_privkey_bytes)?;

    // Step 1: User builds initial transaction (without signing)
    println!("\n📝 Step 1: User 构建初始交易（不签名）...");
    let user_request = FundingRequest {
        script: spillman_lock_script.clone(),
        local_amount: user_amount,  // user_capacity + buffer
        fee_rate: 1000,
    };

    let user_lock = Script::from(user_address);
    let user_context = FundingContext {
        secret_key: user_secret_key.clone(),
        rpc_url: config.network.rpc_url.clone(),
        funding_source_lock_script: user_lock,
    };

    let user_tx = FundingTx::new()
        .build_without_sign(user_request, user_context)
        .await?;

    println!("✓ User transaction built (含 {} user 资金 + buffer)", user_capacity);

    // Step 2: Merchant adds their minimum occupied capacity on top (without signing)
    println!("\n📝 Step 2: Merchant 添加最小占用容量（不签名）...");
    let merchant_request = FundingRequest {
        script: spillman_lock_script.clone(),
        local_amount: merchant_amount,  // min occupied capacity
        fee_rate: 1000,
    };

    let merchant_context = FundingContext {
        secret_key: merchant_secret_key.clone(),
        rpc_url: config.network.rpc_url.clone(),
        funding_source_lock_script: merchant_lock,
    };

    let combined_tx = user_tx  // Incremental construction!
        .build_without_sign(merchant_request, merchant_context.clone())
        .await?;

    println!("✓ Merchant 最小占用容量已添加");

    // Step 3: Sign with both keys
    println!("\n🔏 Step 3: 使用双方密钥签名交易...");
    let final_tx = combined_tx
        .sign_with_multiple_keys(vec![user_secret_key, merchant_secret_key], merchant_context.rpc_url.clone())
        .await?;

    let tx = final_tx.into_inner().ok_or_else(|| anyhow!("No transaction"))?;
    let tx_hash = tx.hash();

    println!("✓ Transaction built and signed");
    println!("  - Transaction hash: {:#x}", tx_hash);
    println!("  - Inputs count: {}", tx.inputs().len());
    println!("  - Outputs count: {}", tx.outputs().len());

    // Calculate fee
    let total_input: u64 = {
        let ckb_client = CkbRpcClient::new(&merchant_context.rpc_url);
        let mut total = 0u64;
        for input in tx.input_pts_iter() {
            if let Ok(cell_with_status) = ckb_client.get_live_cell(input.into(), false) {
                if let Some(cell) = cell_with_status.cell {
                    let capacity: u64 = cell.output.capacity.into();
                    total += capacity;
                }
            }
        }
        total
    };

    let total_output: u64 = tx
        .outputs()
        .into_iter()
        .map(|o| Unpack::<u64>::unpack(&o.capacity()))
        .sum();

    let fee = total_input.saturating_sub(total_output);
    println!("  - Fee: {} ({} shannon)", HumanCapacity::from(fee), fee);

    // Verify funding cell capacity
    let funding_cell_capacity: u64 = Unpack::<u64>::unpack(&tx.outputs().get(0).unwrap().capacity());
    let expected_capacity = user_capacity_shannon + merchant_capacity_shannon + user_buffer_shannon;
    println!("  - Funding cell capacity: {} ({} shannon)", HumanCapacity::from(funding_cell_capacity), funding_cell_capacity);
    println!("  - Expected capacity: {} ({} shannon)", HumanCapacity::from(expected_capacity), expected_capacity);
    assert_eq!(funding_cell_capacity, expected_capacity, "Funding cell capacity mismatch!");

    // Save transaction
    let tx_json = ckb_jsonrpc_types::TransactionView::from(tx);
    let json_str = serde_json::to_string_pretty(&tx_json)?;

    if let Some(parent) = std::path::Path::new(output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, json_str)?;

    println!("✓ Signed co-funding transaction saved: {}", output_path);

    // Return tx_hash and output_index (funding cell is always at index 0)
    Ok((tx_hash.unpack(), 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_funding_request_creation() {
        let script = Script::default();
        let request = FundingRequest {
            script: script.clone(),
            local_amount: 1000_0000_0000,
            fee_rate: 1000,
        };

        assert_eq!(request.local_amount, 1000_0000_0000);
        assert_eq!(request.fee_rate, 1000);
    }

    #[test]
    fn test_funding_tx_creation() {
        let funding_tx = FundingTx::new();
        assert!(funding_tx.as_ref().is_none());
    }

    #[test]
    fn test_human_capacity_parsing() {
        use std::str::FromStr;

        // Test various formats
        assert_eq!(HumanCapacity::from_str("100").unwrap(), HumanCapacity::from(100 * ONE_CKB));
        assert_eq!(HumanCapacity::from_str("100.0").unwrap(), HumanCapacity::from(100 * ONE_CKB));
        assert_eq!(HumanCapacity::from_str("100.5").unwrap(), HumanCapacity::from(100_50000000));
        assert_eq!(HumanCapacity::from_str("0.123").unwrap(), HumanCapacity::from(12_300_000));
        assert_eq!(HumanCapacity::from_str("0.00000001").unwrap(), HumanCapacity::from(1)); // 1 shannon

        // Test conversion to u64
        let capacity: u64 = HumanCapacity::from_str("100.5").unwrap().into();
        assert_eq!(capacity, 100_50000000);

        // Test invalid formats
        assert!(HumanCapacity::from_str("abc").is_err());
        assert!(HumanCapacity::from_str("-100").is_err());
        assert!(HumanCapacity::from_str("100.123456789").is_err()); // Too many decimals
    }

    #[test]
    fn test_human_capacity_display() {
        assert_eq!(HumanCapacity::from(100 * ONE_CKB).to_string(), "100.0");
        assert_eq!(HumanCapacity::from(100_50000000).to_string(), "100.5");
        assert_eq!(HumanCapacity::from(12_300_000).to_string(), "0.123");
        assert_eq!(HumanCapacity::from(1).to_string(), "0.00000001");
    }
}
