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
    constants::{MultisigScript, ONE_CKB, SIGHASH_TYPE_HASH},
    rpc::CkbRpcClient,
    traits::{
        CellCollector, CellDepResolver, DefaultCellCollector, DefaultCellDepResolver,
        DefaultHeaderDepResolver, DefaultTransactionDependencyProvider, HeaderDepResolver,
        SecpCkbRawKeySigner, TransactionDependencyProvider,
    },
    tx_builder::{unlock_tx, CapacityBalancer, TxBuilder, TxBuilderError},
    unlock::{
        MultisigConfig as SdkMultisigConfig, ScriptUnlocker, SecpMultisigUnlocker,
        SecpSighashUnlocker,
    },
    Address, HumanCapacity, ScriptId,
};
use ckb_types::{
    bytes::Bytes,
    core::{BlockView, Capacity, ScriptHashType, TransactionView},
    packed::{CellDep, CellOutput, Script, Transaction, WitnessArgs},
    prelude::*,
    H160, H256,
};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use crate::utils::config::Config;
use ckb_hash::blake2b_256;
use ckb_sdk::traits::ValueRangeOption;

/// Funding request parameters
pub struct FundingRequest {
    /// The funding cell lock script
    pub script: Script,
    /// Local party's capacity to fund (in shannons)
    pub local_amount: u64,
    /// Fee rate in shannon/KB
    pub fee_rate: u64,
    /// Optional xUDT type script
    pub xudt_type_script: Option<Script>,
    /// Optional xUDT amount to fund
    pub xudt_amount: Option<u128>,
}

/// Funding context (keys and RPC)
#[derive(Clone)]
pub struct FundingContext {
    /// 所有私钥（单签时只有1个，多签时有多个）
    pub secret_keys: Vec<secp256k1::SecretKey>,
    /// 多签配置（可选，仅在多签时使用）- 使用 SDK 的 MultisigConfig
    pub multisig_config: Option<SdkMultisigConfig>,
    pub rpc_url: String,
    pub funding_source_lock_script: Script,
    /// Optional xUDT cell dep (for xUDT transactions)
    pub xudt_cell_dep: Option<CellDep>,
    /// Optional pre-created cell dep resolver (to avoid repeated genesis queries)
    pub cell_dep_resolver: Option<DefaultCellDepResolver>,
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

    pub fn into_inner(self) -> Option<TransactionView> {
        self.tx
    }

    pub fn update(&mut self, tx: TransactionView) {
        self.tx = Some(tx);
    }

    /// Build the funding transaction
    pub async fn build(self, request: FundingRequest, context: FundingContext) -> Result<Self> {
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

    /// Sign the funding transaction with multiple keys (for co-funding)
    pub async fn sign_with_multiple_keys(
        mut self,
        secret_keys: Vec<secp256k1::SecretKey>,
        multisig_config: Option<SdkMultisigConfig>,
        rpc_url: String,
    ) -> Result<Self> {
        let signer = SecpCkbRawKeySigner::new_with_secret_keys(secret_keys);

        let mut unlockers: HashMap<ScriptId, Box<dyn ScriptUnlocker>> = HashMap::default();

        // Always register SIGHASH unlocker (for user's single-sig inputs)
        let sighash_unlocker = SecpSighashUnlocker::from(Box::new(signer.clone()) as Box<_>);
        let sighash_script_id = ScriptId::new_type(SIGHASH_TYPE_HASH.clone());
        unlockers.insert(
            sighash_script_id,
            Box::new(sighash_unlocker) as Box<dyn ScriptUnlocker>,
        );

        // Register MULTISIG unlocker if merchant is using multisig
        if let Some(config) = multisig_config {
            // Register Legacy multisig unlocker
            let legacy_multisig_unlocker =
                SecpMultisigUnlocker::from((Box::new(signer.clone()) as Box<_>, config.clone()));
            let legacy_script_id = MultisigScript::Legacy.script_id();
            unlockers.insert(
                legacy_script_id,
                Box::new(legacy_multisig_unlocker) as Box<dyn ScriptUnlocker>,
            );

            // Register V2 multisig unlocker
            let v2_multisig_unlocker =
                SecpMultisigUnlocker::from((Box::new(signer) as Box<_>, config));
            let v2_script_id = MultisigScript::V2.script_id();
            unlockers.insert(
                v2_script_id,
                Box::new(v2_multisig_unlocker) as Box<dyn ScriptUnlocker>,
            );
        }

        let tx = self
            .take()
            .ok_or_else(|| anyhow!("No transaction to sign"))?;
        let tx_dep_provider = DefaultTransactionDependencyProvider::new(&rpc_url, 10);

        let (tx, still_locked_groups) = unlock_tx(tx, &tx_dep_provider, &unlockers)?;
        if !still_locked_groups.is_empty() {
            return Err(anyhow!(
                "Some script groups are still locked: {:?}",
                still_locked_groups
            ));
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
        let (funding_cell_output, funding_cell_data) = self.build_funding_cell();

        let mut inputs = vec![];
        let mut cell_deps = HashSet::new();
        let mut witnesses = vec![];

        // Funding cell output
        let mut outputs: Vec<CellOutput> = vec![funding_cell_output];
        let mut outputs_data: Vec<ckb_types::packed::Bytes> = vec![funding_cell_data.pack()];

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

        // Add xUDT cell dep if present
        if let Some(ref xudt_cell_dep) = self.context.xudt_cell_dep {
            cell_deps.insert(xudt_cell_dep.clone());
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
    /// Build the funding cell output and data
    ///
    /// This method implements incremental construction for co-funding:
    /// - If outputs is empty (first party): funding cell contains only local_amount
    /// - If outputs is not empty (second party): funding cell = existing capacity + local_amount
    ///
    /// For xUDT channels:
    /// - Adds type script to the funding cell
    /// - Returns cell data containing xUDT amount (16 bytes, u128 little-endian)
    fn build_funding_cell(&self) -> (CellOutput, Bytes) {
        // Calculate total capacity (CKB)
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

        // Calculate total xUDT amount (for xUDT channels)
        let total_xudt_amount = if let Some(xudt_amount) = self.request.xudt_amount {
            if let Some(tx) = self.funding_tx.tx.as_ref() {
                if let Some(existing_data) = tx.outputs_data().get(0) {
                    // Second party: add to existing xUDT amount
                    let existing_data_bytes: Vec<u8> = existing_data.unpack();
                    if existing_data_bytes.len() >= 16 {
                        let existing_amount =
                            u128::from_le_bytes(existing_data_bytes[0..16].try_into().unwrap());
                        existing_amount
                            .checked_add(xudt_amount)
                            .expect("xUDT amount overflow")
                    } else {
                        xudt_amount
                    }
                } else {
                    // First party: use xudt_amount only
                    xudt_amount
                }
            } else {
                // No transaction yet: use xudt_amount only
                xudt_amount
            }
        } else {
            0u128
        };

        // Build cell output
        let mut builder = CellOutput::new_builder()
            .capacity(Capacity::shannons(total_capacity))
            .lock(self.request.script.clone());

        // Add type script for xUDT channels
        if let Some(ref type_script) = self.request.xudt_type_script {
            builder = builder.type_(Some(type_script.clone()).pack());
        }

        let output = builder.build();

        // Build cell data
        let data = if self.request.xudt_type_script.is_some() {
            // xUDT channel: 16 bytes for amount
            Bytes::from(total_xudt_amount.to_le_bytes().to_vec())
        } else {
            // CKB-only channel: empty data
            Bytes::new()
        };

        (output, data)
    }

    /// Collect xUDT cells and add change output if needed
    ///
    /// This method modifies the base transaction to:
    /// 1. Add xUDT inputs to cover the required amount
    /// 2. Add xUDT change output if there's余额
    async fn balance_xudt_cells(
        &self,
        base_tx: TransactionView,
        cell_collector: &mut dyn CellCollector,
        cell_dep_resolver: &dyn CellDepResolver,
    ) -> Result<TransactionView> {
        // Only process if this is an xUDT transaction
        let xudt_amount = match self.request.xudt_amount {
            Some(amount) if amount > 0 => amount,
            _ => return Ok(base_tx),
        };

        let type_script = self
            .request
            .xudt_type_script
            .as_ref()
            .ok_or_else(|| anyhow!("xUDT amount specified but no type script"))?;
        // Collect all cells with matching lock script
        use ckb_sdk::traits::CellQueryOptions;
        let mut query = CellQueryOptions::new_lock(self.context.funding_source_lock_script.clone());
        query.secondary_script = Some(type_script.clone());
        query.data_len_range = Some(ValueRangeOption::new_min(16));
        // Set min_total_capacity to a large value to collect all matching cells
        // Default is 1 shannon which stops after collecting just one cell
        query.min_total_capacity = u64::MAX;
        let (cells, _) = cell_collector
            .collect_live_cells_async(&query, false)
            .await?;

        println!("  - Found {} cells with matching lock script", cells.len());

        // Filter cells with matching type script and collect xUDT amounts
        let mut xudt_inputs = vec![];
        let mut collected_xudt_amount = 0u128;
        let mut cells_with_type = 0;
        let mut cells_without_type = 0;

        println!("  - Cells: {:?}", cells.len());

        for cell in cells {
            // Check if cell has the matching type script
            if let Some(cell_type) = cell.output.type_().to_opt() {
                cells_with_type += 1;

                if cell_type.as_slice() == type_script.as_slice() {
                    // Parse xUDT amount from cell data
                    let data_bytes = cell.output_data.to_vec();
                    if data_bytes.len() >= 16 {
                        let amount = u128::from_le_bytes(data_bytes[0..16].try_into().unwrap());
                        println!("  - ✓ Found matching xUDT cell with amount: {}", amount);
                        collected_xudt_amount += amount;
                        xudt_inputs.push(cell);

                        if collected_xudt_amount >= xudt_amount {
                            break;
                        }
                    }
                } else {
                    println!("  - ✗ Type script doesn't match");
                }
            } else {
                cells_without_type += 1;
            }
        }

        println!(
            "  - Summary: {} cells with type script, {} cells without type script",
            cells_with_type, cells_without_type
        );

        if collected_xudt_amount < xudt_amount {
            return Err(anyhow!(
                "Insufficient xUDT balance: collected {}, required {}",
                collected_xudt_amount,
                xudt_amount
            ));
        }

        println!(
            "  - Collected {} xUDT from {} cells",
            collected_xudt_amount,
            xudt_inputs.len()
        );

        // Calculate change amount
        let change_amount = collected_xudt_amount - xudt_amount;

        // Build the updated transaction
        let mut inputs: Vec<_> = base_tx.inputs().into_iter().collect();
        let mut outputs: Vec<_> = base_tx.outputs().into_iter().collect();
        let mut outputs_data: Vec<_> = base_tx.outputs_data().into_iter().collect();
        let mut witnesses: Vec<_> = base_tx.witnesses().into_iter().collect();

        // Determine witness placeholder size
        let witness_placeholder = if let Some(ref config) = self.context.multisig_config {
            // For multisig: use SDK's placeholder_witness() method
            config.placeholder_witness()
        } else {
            // For single-sig: 65 bytes signature
            WitnessArgs::new_builder()
                .lock(Some(molecule::bytes::Bytes::from(vec![0u8; 65])).pack())
                .build()
        };

        // Add xUDT inputs and their witness placeholders
        for cell in &xudt_inputs {
            inputs.push(
                ckb_types::packed::CellInput::new_builder()
                    .previous_output(cell.out_point.clone())
                    .build(),
            );
            // Add witness placeholder for this input
            witnesses.push(witness_placeholder.as_bytes().pack());
        }

        // Add xUDT change output if needed
        if change_amount > 0 {
            println!("  - Adding xUDT change output: {} xUDT", change_amount);

            // Calculate minimum capacity for xUDT change cell
            let change_output = CellOutput::new_builder()
                .lock(self.context.funding_source_lock_script.clone())
                .type_(Some(type_script.clone()).pack())
                .build();

            let min_capacity = change_output
                .occupied_capacity(Capacity::bytes(16).unwrap())
                .unwrap()
                .as_u64();

            let change_output = change_output
                .as_builder()
                .capacity(Capacity::shannons(min_capacity).pack())
                .build();

            let change_data = Bytes::from(change_amount.to_le_bytes().to_vec());

            outputs.push(change_output);
            outputs_data.push(change_data.pack());
        }

        // Collect cell deps from existing transaction
        let mut cell_deps: Vec<_> = base_tx.cell_deps().into_iter().collect();

        // Resolve and add cell deps for newly added xUDT inputs
        if !xudt_inputs.is_empty() {
            // Get lock script from the first xUDT input (they should all have the same lock script)
            let lock_script = &xudt_inputs[0].output.lock();

            // Resolve cell dep for the lock script (e.g., secp256k1)
            if let Some(cell_dep) = cell_dep_resolver.resolve(lock_script) {
                // Check if this cell dep is already in the list (compare by out_point)
                let new_out_point = cell_dep.out_point();
                let already_exists = cell_deps.iter().any(|d| d.out_point() == new_out_point);
                if !already_exists {
                    cell_deps.push(cell_dep);
                }
            }
        }

        // Rebuild transaction with witnesses and cell deps
        let tx = base_tx
            .as_advanced_builder()
            .set_inputs(inputs)
            .set_outputs(outputs)
            .set_outputs_data(outputs_data)
            .set_cell_deps(cell_deps)
            .set_witnesses(witnesses)
            .build();

        Ok(tx)
    }

    /// Internal build method that orchestrates the entire build process
    ///
    /// # Arguments
    /// * `should_sign` - Whether to sign the transaction immediately
    async fn build_internal(self, should_sign: bool) -> Result<FundingTx> {
        // Step 1: Create unlockers with the secret keys from context (user is always single-sig)
        let signer = SecpCkbRawKeySigner::new_with_secret_keys(self.context.secret_keys.clone());
        let sighash_unlocker = SecpSighashUnlocker::from(Box::new(signer) as Box<_>);
        let sighash_script_id = ScriptId::new_type(SIGHASH_TYPE_HASH.clone());
        let mut unlockers = HashMap::default();
        unlockers.insert(
            sighash_script_id,
            Box::new(sighash_unlocker) as Box<dyn ScriptUnlocker>,
        );

        let sender = self.context.funding_source_lock_script.clone();

        // Step 2: Create capacity balancer with appropriate placeholder witness
        let placeholder_witness = if let Some(ref config) = self.context.multisig_config {
            // For multisig: use SDK's placeholder_witness() method
            config.placeholder_witness()
        } else {
            // For single-sig: 65 bytes signature
            WitnessArgs::new_builder()
                .lock(Some(molecule::bytes::Bytes::from(vec![0u8; 65])).pack())
                .build()
        };

        let mut balancer = CapacityBalancer::new_simple(
            sender.clone(),
            placeholder_witness,
            self.request.fee_rate,
        );

        // Step 3: Setup providers
        let ckb_client = CkbRpcClient::new(&self.context.rpc_url);

        // Use pre-created resolver from context if available, otherwise create one
        let cell_dep_resolver = if let Some(resolver) = &self.context.cell_dep_resolver {
            resolver.clone()
        } else {
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
            let base_tx = self
                .build_base_async(
                    &mut cell_collector,
                    &cell_dep_resolver,
                    &header_dep_resolver,
                    &tx_dep_provider,
                )
                .await?;

            // Balance xUDT cells first (if this is an xUDT transaction)
            let xudt_balanced_tx = self
                .balance_xudt_cells(base_tx, &mut cell_collector, &cell_dep_resolver)
                .await?;

            // Balance the transaction (add inputs for this party)
            balancer.balance_tx_capacity(
                &xudt_balanced_tx,
                &mut cell_collector,
                &tx_dep_provider,
                &cell_dep_resolver,
                &header_dep_resolver,
            )?
        } else if is_incremental {
            // Incremental construction: build and balance, but preserve existing signatures
            let base_tx = self
                .build_base_async(
                    &mut cell_collector,
                    &cell_dep_resolver,
                    &header_dep_resolver,
                    &tx_dep_provider,
                )
                .await?;

            // Balance xUDT cells first (if this is an xUDT transaction)
            let xudt_balanced_tx = self
                .balance_xudt_cells(base_tx, &mut cell_collector, &cell_dep_resolver)
                .await?;

            // Balance the transaction (add inputs for this party)
            let balanced_tx = balancer.balance_tx_capacity(
                &xudt_balanced_tx,
                &mut cell_collector,
                &tx_dep_provider,
                &cell_dep_resolver,
                &header_dep_resolver,
            )?;

            // Unlock only the NEW inputs added by this party
            // Get the number of existing inputs from the original transaction
            let existing_input_count = self
                .funding_tx
                .tx
                .as_ref()
                .map(|tx| tx.inputs().len())
                .unwrap_or(0);
            let existing_witnesses = self
                .funding_tx
                .tx
                .as_ref()
                .map(|tx| tx.witnesses())
                .unwrap_or_default();
            let existing_witnesses_len = existing_witnesses.len();

            // Sign the new transaction
            let (signed_tx, still_locked) = unlock_tx(balanced_tx, &tx_dep_provider, &unlockers)?;

            // Check if only new inputs are locked (existing inputs should remain signed)
            let new_locked_groups: Vec<_> = still_locked
                .iter()
                .filter(|g| {
                    g.input_indices
                        .iter()
                        .any(|&idx| idx >= existing_input_count)
                })
                .collect();

            if !new_locked_groups.is_empty() {
                return Err(anyhow!(
                    "Some NEW script groups are still locked: {:?}",
                    new_locked_groups
                ));
            }

            // Preserve existing witnesses for existing inputs
            let builder = signed_tx.as_advanced_builder();
            let mut all_witnesses: Vec<_> = existing_witnesses.into_iter().collect();

            // Append new witnesses for new inputs
            let new_witnesses: Vec<_> = signed_tx
                .witnesses()
                .into_iter()
                .skip(existing_witnesses_len)
                .collect();
            all_witnesses.extend(new_witnesses);

            builder.set_witnesses(all_witnesses).build()
        } else {
            // First party: build, balance xUDT, balance capacity, then unlock
            let base_tx = self
                .build_base_async(
                    &mut cell_collector,
                    &cell_dep_resolver,
                    &header_dep_resolver,
                    &tx_dep_provider,
                )
                .await?;

            // Balance xUDT cells first (if this is an xUDT transaction)
            let xudt_balanced_tx = self
                .balance_xudt_cells(base_tx, &mut cell_collector, &cell_dep_resolver)
                .await?;

            // Balance capacity
            let balanced_tx = balancer.balance_tx_capacity(
                &xudt_balanced_tx,
                &mut cell_collector,
                &tx_dep_provider,
                &cell_dep_resolver,
                &header_dep_resolver,
            )?;

            // Unlock
            let (tx, still_locked_groups) = unlock_tx(balanced_tx, &tx_dep_provider, &unlockers)?;

            if !still_locked_groups.is_empty() {
                return Err(anyhow!(
                    "Some script groups are still locked: {:?}",
                    still_locked_groups
                ));
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
    fee_rate: u64,
    output_path: &str,
    xudt_amount: Option<u128>,
) -> Result<(H256, u32)> {
    let capacity_shannon: u64 = capacity.into();

    println!(
        "  - Spillman Lock cell capacity: {} ({} shannon)",
        capacity, capacity_shannon
    );

    // Build xUDT type script and cell dep if xudt_amount is provided
    let (xudt_type_script, xudt_cell_dep) = if xudt_amount.is_some() {
        if let Some(ref usdi_config) = config.usdi {
            // Build xUDT type script
            let code_hash = H256::from_str(usdi_config.code_hash.trim_start_matches("0x"))
                .map_err(|e| anyhow!("Invalid code_hash: {}", e))?;
            let args = ckb_types::bytes::Bytes::from(
                hex::decode(usdi_config.args.trim_start_matches("0x"))
                    .map_err(|e| anyhow!("Invalid args hex: {}", e))?,
            );

            let type_script = Script::new_builder()
                .code_hash(code_hash.pack())
                .hash_type(ckb_types::packed::Byte::new(ScriptHashType::Type as u8))
                .args(args.pack())
                .build();

            // Build xUDT cell dep
            let tx_hash = H256::from_str(usdi_config.tx_hash.trim_start_matches("0x"))
                .map_err(|e| anyhow!("Invalid tx_hash: {}", e))?;
            let out_point = ckb_types::packed::OutPoint::new_builder()
                .tx_hash(tx_hash.pack())
                .index(ckb_types::packed::Uint32::new_unchecked(
                    usdi_config.index.to_le_bytes().to_vec().into(),
                ))
                .build();
            let cell_dep = CellDep::new_builder()
                .out_point(out_point)
                .dep_type(ckb_types::packed::Byte::new(
                    ckb_types::core::DepType::Code as u8,
                ))
                .build();

            println!("  - xUDT amount: {}", xudt_amount.unwrap());

            (Some(type_script), Some(cell_dep))
        } else {
            return Err(anyhow!("xUDT amount provided but usdi config not found"));
        }
    } else {
        (None, None)
    };

    // Parse user private keys
    let secret_keys = config.user.get_secret_keys()?;

    // Check if user is multisig and build multisig config if needed
    let multisig_config = if let Some((threshold, total)) = config.user.get_multisig_config() {
        Some(build_multisig_config(&secret_keys, threshold, total)?)
    } else {
        None
    };

    // Create funding request (single-party funding, remote_amount = 0)
    let request = FundingRequest {
        script: spillman_lock_script.clone(),
        local_amount: capacity_shannon,
        fee_rate, // Use parameter, default 1000 shannon/KB
        xudt_type_script: xudt_type_script.clone(),
        xudt_amount,
    };

    // Create funding context
    let user_lock = Script::from(user_address);
    let context = FundingContext {
        secret_keys,
        multisig_config,
        rpc_url: config.network.rpc_url.clone(),
        funding_source_lock_script: user_lock,
        xudt_cell_dep,
        cell_dep_resolver: None, // Will be created inside build()
    };

    // Build and sign transaction
    println!("  - Building and signing funding transaction...");
    let signed_tx = FundingTx::new().build(request, context.clone()).await?;

    let tx = signed_tx
        .into_inner()
        .ok_or_else(|| anyhow!("No transaction"))?;
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

    // Save transaction (with hash field for refund command to use)
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
    fee_rate: u64,
    output_path: &str,
    user_xudt_amount: Option<u128>,
    merchant_xudt_amount: Option<u128>,
) -> Result<(H256, u32)> {
    println!("  - Co-fund 模式：User + Merchant 共同出资");

    let user_capacity_shannon: u64 = user_capacity.into();

    // Build xUDT type script and cell dep if xudt amounts are provided
    let (xudt_type_script, xudt_cell_dep) =
        if user_xudt_amount.is_some() || merchant_xudt_amount.is_some() {
            if let Some(ref usdi_config) = config.usdi {
                // Build xUDT type script
                let code_hash = H256::from_str(usdi_config.code_hash.trim_start_matches("0x"))
                    .map_err(|e| anyhow!("Invalid code_hash: {}", e))?;
                let args = ckb_types::bytes::Bytes::from(
                    hex::decode(usdi_config.args.trim_start_matches("0x"))
                        .map_err(|e| anyhow!("Invalid args hex: {}", e))?,
                );

                let type_script = Script::new_builder()
                    .code_hash(code_hash.pack())
                    .hash_type(ckb_types::packed::Byte::new(ScriptHashType::Type as u8))
                    .args(args.pack())
                    .build();

                // Build xUDT cell dep
                let tx_hash = H256::from_str(usdi_config.tx_hash.trim_start_matches("0x"))
                    .map_err(|e| anyhow!("Invalid tx_hash: {}", e))?;
                let out_point = ckb_types::packed::OutPoint::new_builder()
                    .tx_hash(tx_hash.pack())
                    .index(ckb_types::packed::Uint32::new_unchecked(
                        usdi_config.index.to_le_bytes().to_vec().into(),
                    ))
                    .build();
                let cell_dep = CellDep::new_builder()
                    .out_point(out_point)
                    .dep_type(ckb_types::packed::Byte::new(
                        ckb_types::core::DepType::Code as u8,
                    ))
                    .build();

                if let Some(user_amt) = user_xudt_amount {
                    println!("  - User xUDT amount: {}", user_amt);
                }
                if let Some(merchant_amt) = merchant_xudt_amount {
                    println!("  - Merchant xUDT amount: {}", merchant_amt);
                }

                (Some(type_script), Some(cell_dep))
            } else {
                return Err(anyhow!("xUDT amount provided but usdi config not found"));
            }
        } else {
            (None, None)
        };

    // Calculate merchant's minimum occupied capacity
    // NOTE: For xUDT channels, merchant needs extra capacity for type script
    let merchant_lock = Script::from(merchant_address);
    let mut temp_merchant_cell_builder = CellOutput::new_builder()
        .capacity(0u64)
        .lock(merchant_lock.clone());

    // Add type script if this is an xUDT channel
    if let Some(ref type_script) = xudt_type_script {
        temp_merchant_cell_builder =
            temp_merchant_cell_builder.type_(Some(type_script.clone()).pack());
    }

    let temp_merchant_cell = temp_merchant_cell_builder.build();

    // For xUDT: data is 16 bytes; for CKB-only: data is 0 bytes
    let data_size = if xudt_type_script.is_some() { 16 } else { 0 };
    let merchant_capacity_shannon = temp_merchant_cell
        .occupied_capacity(Capacity::bytes(data_size).unwrap())
        .unwrap()
        .as_u64();

    // User adds extra 1 CKB as buffer (for fees, etc.)
    let user_buffer_shannon = ONE_CKB;

    let user_amount = user_capacity_shannon + user_buffer_shannon;
    let merchant_amount = merchant_capacity_shannon;

    println!(
        "  - User 需出资: {} + {} buffer",
        user_capacity,
        HumanCapacity::from(user_buffer_shannon)
    );
    println!(
        "  - Merchant 需出资: {} (最小占用)",
        HumanCapacity::from(merchant_capacity_shannon)
    );

    // Optimization: Query genesis block once and reuse for both parties
    // This avoids slow genesis queries (5-10s each) during Step 1 and Step 2
    println!("\n🔍 预先查询 genesis block (优化性能)...");
    let ckb_client = CkbRpcClient::new(&config.network.rpc_url);
    let cell_dep_resolver = {
        match ckb_client.get_block_by_number(0.into())? {
            Some(genesis_block) => {
                println!("✓ Genesis block 查询完成，将复用于 User 和 Merchant");
                Some(DefaultCellDepResolver::from_genesis(&BlockView::from(
                    genesis_block,
                ))?)
            }
            None => {
                return Err(anyhow!("Failed to get genesis block"));
            }
        }
    };

    // Parse keys for user and merchant
    let user_secret_keys = config.user.get_secret_keys()?;
    let merchant_secret_keys = config.merchant.get_secret_keys()?;

    // Build multisig configs if needed (detect type from address)
    let user_multisig_config = if let Some((threshold, total)) = config.user.get_multisig_config() {
        // Detect user's multisig type from address
        let user_lock_script = Script::from(user_address);
        let code_hash: H256 = user_lock_script.code_hash().unpack();

        let legacy_script_id = MultisigScript::Legacy.script_id();
        let v2_script_id = MultisigScript::V2.script_id();

        let multisig_type = if code_hash == legacy_script_id.code_hash
            && user_lock_script.hash_type() == legacy_script_id.hash_type.into()
        {
            MultisigScript::Legacy
        } else if code_hash == v2_script_id.code_hash
            && user_lock_script.hash_type() == v2_script_id.hash_type.into()
        {
            MultisigScript::V2
        } else {
            return Err(anyhow!("Unknown multisig type for user address"));
        };

        Some(build_multisig_config_with_type(
            &user_secret_keys,
            threshold,
            total,
            multisig_type,
        )?)
    } else {
        None
    };

    let merchant_multisig_config =
        if let Some((threshold, total)) = config.merchant.get_multisig_config() {
            // Detect merchant's multisig type from address
            let merchant_lock_script = Script::from(merchant_address);
            let code_hash: H256 = merchant_lock_script.code_hash().unpack();

            let legacy_script_id = MultisigScript::Legacy.script_id();
            let v2_script_id = MultisigScript::V2.script_id();

            let multisig_type = if code_hash == legacy_script_id.code_hash
                && merchant_lock_script.hash_type() == legacy_script_id.hash_type.into()
            {
                MultisigScript::Legacy
            } else if code_hash == v2_script_id.code_hash
                && merchant_lock_script.hash_type() == v2_script_id.hash_type.into()
            {
                MultisigScript::V2
            } else {
                return Err(anyhow!("Unknown multisig type for merchant address"));
            };

            Some(build_multisig_config_with_type(
                &merchant_secret_keys,
                threshold,
                total,
                multisig_type,
            )?)
        } else {
            None
        };

    // Step 1: User builds initial transaction (without signing)
    println!("\n📝 Step 1: User 构建初始交易（不签名）...");
    let user_request = FundingRequest {
        script: spillman_lock_script.clone(),
        local_amount: user_amount, // user_capacity + buffer
        fee_rate,                  // Use parameter, default 1000 shannon/KB
        xudt_type_script: xudt_type_script.clone(),
        xudt_amount: user_xudt_amount,
    };

    let user_lock = Script::from(user_address);
    let user_context = FundingContext {
        secret_keys: user_secret_keys.clone(),
        multisig_config: user_multisig_config.clone(),
        rpc_url: config.network.rpc_url.clone(),
        funding_source_lock_script: user_lock,
        xudt_cell_dep: xudt_cell_dep.clone(),
        cell_dep_resolver: cell_dep_resolver.clone(),
    };

    let user_tx = FundingTx::new()
        .build_without_sign(user_request, user_context)
        .await?;

    println!(
        "✓ User transaction built (含 {} user 资金 + buffer)",
        user_capacity
    );

    // Step 2: Merchant adds their minimum occupied capacity on top (without signing)
    println!("\n📝 Step 2: Merchant 添加最小占用容量（不签名）...");
    let merchant_request = FundingRequest {
        script: spillman_lock_script.clone(),
        local_amount: merchant_amount, // min occupied capacity
        fee_rate,                      // Use parameter, default 1000 shannon/KB
        xudt_type_script: xudt_type_script.clone(),
        xudt_amount: merchant_xudt_amount,
    };

    let merchant_context = FundingContext {
        secret_keys: merchant_secret_keys.clone(),
        multisig_config: merchant_multisig_config,
        rpc_url: config.network.rpc_url.clone(),
        funding_source_lock_script: merchant_lock,
        xudt_cell_dep,
        cell_dep_resolver,
    };

    let combined_tx = user_tx // Incremental construction!
        .build_without_sign(merchant_request, merchant_context.clone())
        .await?;

    println!("✓ Merchant 最小占用容量已添加");

    // Note: Multisig cell dep is automatically added by SecpMultisigUnlocker during signing

    // Step 3: Sign with both parties' keys
    println!("\n🔏 Step 3: 使用双方密钥签名交易...");

    // For multisig, only include threshold number of merchant keys (not all)
    let merchant_signing_keys: Vec<_> =
        if let Some(ref multisig_cfg) = merchant_context.multisig_config {
            // Only take threshold number of keys for signing
            merchant_secret_keys
                .iter()
                .take(multisig_cfg.threshold() as usize)
                .cloned()
                .collect()
        } else {
            merchant_secret_keys.to_vec()
        };

    let all_secret_keys: Vec<_> = user_secret_keys
        .iter()
        .chain(merchant_signing_keys.iter())
        .cloned()
        .collect();

    let final_tx = combined_tx
        .sign_with_multiple_keys(
            all_secret_keys,
            merchant_context.multisig_config.clone(),
            merchant_context.rpc_url.clone(),
        )
        .await?;

    let tx = final_tx
        .into_inner()
        .ok_or_else(|| anyhow!("No transaction"))?;
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
    let funding_cell_capacity: u64 =
        Unpack::<u64>::unpack(&tx.outputs().get(0).unwrap().capacity());
    let expected_capacity = user_capacity_shannon + merchant_capacity_shannon + user_buffer_shannon;
    println!(
        "  - Funding cell capacity: {} ({} shannon)",
        HumanCapacity::from(funding_cell_capacity),
        funding_cell_capacity
    );
    println!(
        "  - Expected capacity: {} ({} shannon)",
        HumanCapacity::from(expected_capacity),
        expected_capacity
    );
    assert_eq!(
        funding_cell_capacity, expected_capacity,
        "Funding cell capacity mismatch!"
    );

    // Save transaction (with hash field for refund command to use)
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
            xudt_type_script: None,
            xudt_amount: None,
        };

        assert_eq!(request.local_amount, 1000_0000_0000);
        assert_eq!(request.fee_rate, 1000);
    }

    #[test]
    fn test_funding_tx_creation() {
        let funding_tx = FundingTx::new();
        assert!(funding_tx.into_inner().is_none());
    }

    #[test]
    fn test_human_capacity_parsing() {
        use std::str::FromStr;

        // Test various formats
        assert_eq!(
            HumanCapacity::from_str("100").unwrap(),
            HumanCapacity::from(100 * ONE_CKB)
        );
        assert_eq!(
            HumanCapacity::from_str("100.0").unwrap(),
            HumanCapacity::from(100 * ONE_CKB)
        );
        assert_eq!(
            HumanCapacity::from_str("100.5").unwrap(),
            HumanCapacity::from(100_50000000)
        );
        assert_eq!(
            HumanCapacity::from_str("0.123").unwrap(),
            HumanCapacity::from(12_300_000)
        );
        assert_eq!(
            HumanCapacity::from_str("0.00000001").unwrap(),
            HumanCapacity::from(1)
        ); // 1 shannon

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

/// 构建多签配置的辅助函数
///
/// 根据私钥列表构建 SDK 的 MultisigConfig（默认使用 V2）
///
/// # Arguments
/// * `secret_keys` - 私钥列表（长度必须等于 total）
/// * `threshold` - M: 需要多少个签名
/// * `total` - N: 总共多少个公钥
///
/// # Returns
/// * `SdkMultisigConfig` - SDK 的 MultisigConfig，可以直接调用 placeholder_witness() 等方法
pub fn build_multisig_config(
    secret_keys: &[secp256k1::SecretKey],
    threshold: u8,
    total: u8,
) -> Result<SdkMultisigConfig> {
    build_multisig_config_with_type(secret_keys, threshold, total, MultisigScript::V2)
}

/// 根据私钥列表和指定类型构建 SDK 的 MultisigConfig
///
/// # Arguments
/// * `secret_keys` - 私钥列表（长度必须等于 total）
/// * `threshold` - M: 需要多少个签名
/// * `total` - N: 总共多少个公钥
/// * `multisig_type` - MultisigScript::Legacy 或 MultisigScript::V2
///
/// # Returns
/// * `SdkMultisigConfig` - SDK 的 MultisigConfig，可以直接调用 placeholder_witness() 等方法
pub fn build_multisig_config_with_type(
    secret_keys: &[secp256k1::SecretKey],
    threshold: u8,
    total: u8,
    multisig_type: MultisigScript,
) -> Result<SdkMultisigConfig> {
    if secret_keys.len() != total as usize {
        return Err(anyhow!(
            "secret_keys length ({}) must equal total ({})",
            secret_keys.len(),
            total
        ));
    }

    if threshold == 0 || threshold > total {
        return Err(anyhow!(
            "Invalid multisig config: threshold={}, total={}",
            threshold,
            total
        ));
    }

    // 计算所有公钥的 hash160
    let secp = secp256k1::Secp256k1::new();
    let mut sighash_addresses = Vec::new();

    for secret_key in secret_keys {
        let pubkey = secp256k1::PublicKey::from_secret_key(&secp, secret_key);
        let pubkey_bytes = pubkey.serialize();
        let pubkey_hash = &blake2b_256(pubkey_bytes)[0..20];
        sighash_addresses.push(H160::from_slice(pubkey_hash)?);
    }

    // 使用 SDK 的 MultisigConfig::new_with 构建
    Ok(SdkMultisigConfig::new_with(
        multisig_type,
        sighash_addresses,
        0, // require_first_n: 0 means any M of N
        threshold,
    )?)
}
