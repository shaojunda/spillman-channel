/// Refactored refund transaction builder following Fiber's TxBuilder pattern
///
/// Key improvements over v1:
/// - Uses TxBuilder trait for structured transaction construction
/// - Separates concerns: build_base -> balance (no balance needed for refund) -> sign
/// - Cleaner witness structure handling
/// - Better error messages and type safety
/// - Consistent with funding_v2 architecture
///
/// # Refund Transaction Structure
///
/// ## Inputs
/// - Spillman Lock cell (from funding transaction output 0)
/// - Since: timeout timestamp (read from Spillman Lock args)
///
/// ## Outputs
/// - Single mode: User's cell (full refund minus fee)
/// - Co-fund mode: User's cell + Merchant's cell (proportional split minus fee)
///
/// ## Witness
/// - EMPTY_WITNESS_ARGS (16 bytes)
/// - UNLOCK_TYPE_TIMEOUT (1 byte, 0x01)
/// - Merchant signature (65 bytes, pre-signed during setup)
/// - User signature (65 bytes, added after timeout)
///
/// Total: 147 bytes
///
/// # Signing Flow
///
/// 1. **Setup phase**: Merchant pre-signs the refund transaction
///    - This guarantees the user can always get their funds back after timeout
/// 2. **After timeout**: User adds their signature and broadcasts
///
/// # Example
///
/// ```ignore
/// // Single-party refund
/// let (tx_hash, tx) = build_refund_transaction(
///     &config,
///     funding_tx_hash,
///     &funding_tx,
///     &user_address,
///     None, // No merchant for single-party
///     "output/refund_tx.json",
/// ).await?;
///
/// // Co-funded refund
/// let (tx_hash, tx) = build_refund_transaction(
///     &config,
///     funding_tx_hash,
///     &funding_tx,
///     &user_address,
///     Some(&merchant_address),
///     "output/refund_tx.json",
/// ).await?;
/// ```

use anyhow::{anyhow, Result};
use ckb_crypto::secp::Privkey;
use ckb_hash::blake2b_256;
use ckb_sdk::{
    traits::{
        CellDepResolver, HeaderDepResolver,
        TransactionDependencyProvider,
    },
    tx_builder::{TxBuilder, TxBuilderError},
    Address, HumanCapacity,
};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, DepType, TransactionView},
    packed::{CellDep, CellDepVec, CellInput, CellOutput, OutPoint, Script, Transaction},
    prelude::*,
    H256,
};
use std::str::FromStr;

use crate::utils::config::Config;
use crate::utils::crypto::pubkey_hash;

// Constants for witness structure
const EMPTY_WITNESS_ARGS: [u8; 16] = [16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0];
const UNLOCK_TYPE_TIMEOUT: u8 = 0x01;
const REFUND_WITNESS_SIZE_SINGLE_SIG: usize = 147; // 16 + 1 + 65 + 65

/// Calculate refund witness size based on merchant's signature type
///
/// # Arguments
/// * `merchant_multisig_config` - Optional multisig config for merchant
///
/// # Returns
/// Total witness size in bytes
fn calculate_refund_witness_size(merchant_multisig_config: Option<&ckb_sdk::unlock::MultisigConfig>) -> usize {
    use crate::tx_builder::witness_utils;
    witness_utils::calculate_refund_witness_size(merchant_multisig_config)
}

/// Refund request parameters
#[derive(Clone)]
pub struct RefundRequest {
    /// The funding transaction hash
    pub funding_tx_hash: H256,
    /// The funding transaction
    pub funding_tx: TransactionView,
    /// User's lock script (refund destination)
    pub user_lock_script: Script,
    /// Merchant's lock script (optional, for co-fund mode)
    pub merchant_lock_script: Option<Script>,
    /// Fee rate in shannon/KB
    pub fee_rate: u64,
}

/// Refund context (keys and RPC)
#[derive(Clone)]
pub struct RefundContext {
    #[allow(dead_code)]
    pub user_secret_key: secp256k1::SecretKey,
    /// Merchant secret keys (single-sig: 1 key, multisig: multiple keys)
    #[allow(dead_code)]
    pub merchant_secret_keys: Option<Vec<secp256k1::SecretKey>>,
    /// Multisig configuration for merchant (if merchant uses multisig)
    #[allow(dead_code)]
    pub merchant_multisig_config: Option<ckb_sdk::unlock::MultisigConfig>,
    #[allow(dead_code)]
    pub rpc_url: String,
    pub spillman_lock_dep: CellDep,
    pub auth_dep: CellDep,
}

/// Refund transaction wrapper
#[derive(Clone, Debug, Default)]
pub struct RefundTx {
    tx: Option<TransactionView>,
}

impl RefundTx {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take(&mut self) -> Option<TransactionView> {
        self.tx.take()
    }

    #[allow(dead_code)]
    pub fn as_ref(&self) -> Option<&TransactionView> {
        self.tx.as_ref()
    }

    pub fn into_inner(self) -> Option<TransactionView> {
        self.tx
    }

    pub fn update(&mut self, tx: TransactionView) {
        self.tx = Some(tx);
    }

    /// Build the refund transaction
    pub async fn build(
        self,
        request: RefundRequest,
        context: RefundContext,
    ) -> Result<Self> {
        let builder = RefundTxBuilder {
            refund_tx: self,
            request,
            context,
        };
        builder.build_internal().await
    }

    /// Sign the refund transaction with Spillman Lock witness structure
    ///
    /// Spillman Lock timeout path requires:
    /// - EMPTY_WITNESS_ARGS (16 bytes)
    /// - UNLOCK_TYPE_TIMEOUT (1 byte, 0x01)
    /// - Merchant signature (65 bytes for single-sig, or multisig config + M signatures for multisig)
    /// - User signature (65 bytes)
    pub fn sign_for_spillman_lock(
        mut self,
        user_privkey: &Privkey,
        merchant_secret_keys: &[secp256k1::SecretKey],
        spillman_lock_args: &[u8],
        merchant_multisig_config: Option<&ckb_sdk::unlock::MultisigConfig>,
    ) -> Result<Self> {
        let tx = self.take().ok_or_else(|| anyhow!("No transaction to sign"))?;

        // Verify pubkey hashes match Spillman Lock args
        let user_pubkey = user_privkey.pubkey()
            .map_err(|e| anyhow!("Failed to get user pubkey: {:?}", e))?;

        let user_pubkey_hash_from_privkey = pubkey_hash(&user_pubkey);

        let expected_merchant_hash = &spillman_lock_args[0..20];
        let expected_user_hash = &spillman_lock_args[20..40];

        // Verify merchant hash (different logic for single-sig vs multisig)
        if let Some(multisig_config) = merchant_multisig_config {
            // For multisig: merchant_hash should be blake160(multisig_config_data)
            use ckb_hash::blake2b_256;
            let config_data = multisig_config.to_witness_data();
            let hash = blake2b_256(&config_data);
            let merchant_multisig_hash = &hash[0..20];
            if merchant_multisig_hash != expected_merchant_hash {
                return Err(anyhow!("Merchant multisig hash mismatch! Expected: {}, Got: {}",
                    hex::encode(expected_merchant_hash),
                    hex::encode(merchant_multisig_hash)
                ));
            }
        } else {
            // For single-sig: merchant_hash is blake160(pubkey)
            if merchant_secret_keys.len() != 1 {
                return Err(anyhow!("Single-sig merchant should have exactly 1 secret key"));
            }
            let secp = secp256k1::Secp256k1::new();
            let merchant_pubkey_secp = secp256k1::PublicKey::from_secret_key(&secp, &merchant_secret_keys[0]);
            let merchant_pubkey_bytes = merchant_pubkey_secp.serialize();
            use ckb_hash::blake2b_256;
            let merchant_pubkey_hash_from_privkey = &blake2b_256(&merchant_pubkey_bytes)[0..20];
            if merchant_pubkey_hash_from_privkey != expected_merchant_hash {
                return Err(anyhow!("Merchant pubkey hash mismatch!"));
            }
        }

        // Verify user hash (always single-sig)
        if user_pubkey_hash_from_privkey != expected_user_hash {
            return Err(anyhow!("User pubkey hash mismatch!"));
        }

        // Compute signing message (raw tx without cell_deps)
        let signing_message = compute_signing_message(&tx);

        // Build witness based on merchant signature type
        let witness_data = if let Some(multisig_config) = merchant_multisig_config {
            // Multisig merchant: collect threshold number of signatures
            let threshold = multisig_config.threshold() as usize;
            if merchant_secret_keys.len() < threshold {
                return Err(anyhow!("Not enough merchant secret keys: need {}, got {}", threshold, merchant_secret_keys.len()));
            }

            let mut merchant_signatures = Vec::new();
            for key in merchant_secret_keys.iter().take(threshold) {
                // Convert secp256k1::SecretKey to ckb_crypto::secp::Privkey
                let privkey_bytes = key.secret_bytes();
                let merchant_privkey = Privkey::from_slice(&privkey_bytes);
                let signature = merchant_privkey
                    .sign_recoverable(&signing_message.into())
                    .map_err(|e| anyhow!("Failed to sign with merchant key: {:?}", e))?
                    .serialize();
                merchant_signatures.extend_from_slice(&signature);
            }

            let user_sig = user_privkey
                .sign_recoverable(&signing_message.into())
                .map_err(|e| anyhow!("Failed to sign with user key: {:?}", e))?
                .serialize();

            // Multisig witness: empty_witness_args + unlock_type + multisig_config + merchant_signatures + user_signature
            let config_data = multisig_config.to_witness_data();
            [
                &EMPTY_WITNESS_ARGS[..],
                &[UNLOCK_TYPE_TIMEOUT][..],
                &config_data[..],
                &merchant_signatures[..],
                &user_sig[..],
            ]
            .concat()
        } else {
            // Single-sig merchant
            let privkey_bytes = merchant_secret_keys[0].secret_bytes();
            let merchant_privkey = Privkey::from_slice(&privkey_bytes);
            let merchant_sig = merchant_privkey
                .sign_recoverable(&signing_message.into())
                .map_err(|e| anyhow!("Failed to sign with merchant key: {:?}", e))?
                .serialize();

            let user_sig = user_privkey
                .sign_recoverable(&signing_message.into())
                .map_err(|e| anyhow!("Failed to sign with user key: {:?}", e))?
                .serialize();

            // Single-sig witness: empty_witness_args + unlock_type + merchant_sig + user_sig
            [
                &EMPTY_WITNESS_ARGS[..],
                &[UNLOCK_TYPE_TIMEOUT][..],
                &merchant_sig[..],
                &user_sig[..],
            ]
            .concat()
        };

        // Rebuild transaction with witness
        let signed_tx = tx
            .as_advanced_builder()
            .set_witnesses(vec![Bytes::from(witness_data).pack()])
            .build();

        self.update(signed_tx);
        Ok(self)
    }
}

impl From<TransactionView> for RefundTx {
    fn from(tx: TransactionView) -> Self {
        Self { tx: Some(tx) }
    }
}

impl From<Transaction> for RefundTx {
    fn from(tx: Transaction) -> Self {
        Self {
            tx: Some(tx.into_view()),
        }
    }
}

/// Internal builder implementing TxBuilder trait
struct RefundTxBuilder {
    refund_tx: RefundTx,
    request: RefundRequest,
    context: RefundContext,
}

#[async_trait::async_trait]
impl TxBuilder for RefundTxBuilder {
    async fn build_base_async(
        &self,
        _cell_collector: &mut dyn ckb_sdk::traits::CellCollector,
        _cell_dep_resolver: &dyn CellDepResolver,
        _header_dep_resolver: &dyn HeaderDepResolver,
        _tx_dep_provider: &dyn TransactionDependencyProvider,
    ) -> Result<TransactionView, TxBuilderError> {
        // Get Spillman Lock cell from funding tx output 0
        let spillman_cell = self.request.funding_tx
            .outputs()
            .get(0)
            .ok_or_else(|| TxBuilderError::Other(anyhow!("Funding transaction has no output 0")))?;

        let spillman_capacity: u64 = spillman_cell.capacity().unpack();

        // Parse timeout_since from Spillman Lock args
        let lock_script = spillman_cell.lock();
        let args_bytes: Bytes = lock_script.args().unpack();
        if args_bytes.len() != 50 {
            return Err(TxBuilderError::Other(anyhow!(
                "Invalid Spillman Lock args length: expected 50, got {}",
                args_bytes.len()
            )));
        }

        // Extract timeout_since from args (bytes 40-48)
        let timeout_since = u64::from_le_bytes(
            args_bytes[40..48]
                .try_into()
                .map_err(|_| TxBuilderError::Other(anyhow!("Failed to parse timeout_since from args")))?,
        );

        // Build input with timeout since
        let input = CellInput::new_builder()
            .previous_output(
                OutPoint::new_builder()
                    .tx_hash(self.request.funding_tx_hash.pack())
                    .index(0u32)
                    .build(),
            )
            .since(timeout_since)
            .build();

        // Calculate merchant's capacity if co-fund
        let merchant_capacity = if let Some(ref merchant_lock) = self.request.merchant_lock_script {
            let merchant_cell = CellOutput::new_builder()
                .capacity(Capacity::shannons(0))
                .lock(merchant_lock.clone())
                .build();

            merchant_cell
                .occupied_capacity(Capacity::bytes(0).unwrap())
                .unwrap()
                .as_u64()
        } else {
            0
        };

        // Calculate user capacity (spillman_capacity - merchant_capacity - fee_estimate)
        // We use a rough fee estimate here, will be refined by iterative calculation in build_internal
        let estimated_fee = 1000u64; // Rough estimate
        let user_capacity = if self.request.merchant_lock_script.is_some() {
            spillman_capacity
                .checked_sub(merchant_capacity)
                .and_then(|c| c.checked_sub(estimated_fee))
                .ok_or_else(|| TxBuilderError::Other(anyhow!("Not enough capacity for refund outputs and fee")))?
        } else {
            spillman_capacity
                .checked_sub(estimated_fee)
                .ok_or_else(|| TxBuilderError::Other(anyhow!("Not enough capacity for refund and fee")))?
        };

        // Build outputs
        let mut outputs = vec![
            CellOutput::new_builder()
                .capacity(Capacity::shannons(user_capacity))
                .lock(self.request.user_lock_script.clone())
                .build(),
        ];

        let mut outputs_data = vec![Bytes::new().pack()];

        if let Some(ref merchant_lock) = self.request.merchant_lock_script {
            outputs.push(
                CellOutput::new_builder()
                    .capacity(Capacity::shannons(merchant_capacity))
                    .lock(merchant_lock.clone())
                    .build(),
            );
            outputs_data.push(Bytes::new().pack());
        }

        // Build witness placeholder (size depends on merchant's signature type)
        let witness_size = calculate_refund_witness_size(self.context.merchant_multisig_config.as_ref());
        let witness_placeholder = vec![0u8; witness_size];

        let tx = Transaction::default()
            .as_advanced_builder()
            .input(input)
            .cell_dep(self.context.spillman_lock_dep.clone())
            .cell_dep(self.context.auth_dep.clone())
            .set_outputs(outputs)
            .set_outputs_data(outputs_data)
            .witness(Bytes::from(witness_placeholder).pack())
            .build();

        Ok(tx)
    }
}

impl RefundTxBuilder {
    /// Internal build method with iterative fee calculation
    async fn build_internal(self) -> Result<RefundTx> {
        // Get spillman cell capacity
        let spillman_cell = self.request.funding_tx
            .outputs()
            .get(0)
            .ok_or_else(|| anyhow!("Funding transaction has no output 0"))?;
        let spillman_capacity: u64 = spillman_cell.capacity().unpack();

        // Calculate merchant's capacity if co-fund
        let merchant_capacity = if let Some(ref merchant_lock) = self.request.merchant_lock_script {
            let merchant_cell = CellOutput::new_builder()
                .capacity(Capacity::shannons(0))
                .lock(merchant_lock.clone())
                .build();

            merchant_cell
                .occupied_capacity(Capacity::bytes(0).unwrap())
                .unwrap()
                .as_u64()
        } else {
            0
        };

        // Iteratively calculate fee
        let fee_rate = self.request.fee_rate; // Use parameter, default 1000 shannon/KB
        let max_iterations = 10;
        let mut current_fee = 0u64;
        let mut final_tx: Option<TransactionView> = None;

        for iteration in 0..max_iterations {
            // Calculate user capacity based on current fee
            let user_capacity = if self.request.merchant_lock_script.is_some() {
                spillman_capacity
                    .checked_sub(merchant_capacity)
                    .and_then(|c| c.checked_sub(current_fee))
                    .ok_or_else(|| anyhow!("Not enough capacity for refund outputs and fee"))?
            } else {
                spillman_capacity
                    .checked_sub(current_fee)
                    .ok_or_else(|| anyhow!("Not enough capacity for refund and fee"))?
            };

            // Build transaction with calculated capacity
            let temp_tx = self.build_tx_with_capacity(user_capacity, merchant_capacity)?;

            // Calculate actual fee for this transaction
            let tx_size = temp_tx.data().as_reader().serialized_size_in_block() as u64;
            let actual_fee = (tx_size * fee_rate + 999) / 1000; // Round up

            // Check if fee has stabilized
            if actual_fee == current_fee {
                final_tx = Some(temp_tx);
                break;
            }

            current_fee = actual_fee;

            if iteration == max_iterations - 1 {
                final_tx = Some(temp_tx);
            }
        }

        let tx = final_tx.ok_or_else(|| anyhow!("Failed to build transaction"))?;

        let mut refund_tx = self.refund_tx;
        refund_tx.update(tx);

        Ok(refund_tx)
    }

    /// Helper to build transaction with specific capacities
    fn build_tx_with_capacity(&self, user_capacity: u64, merchant_capacity: u64) -> Result<TransactionView> {
        let spillman_cell = self.request.funding_tx
            .outputs()
            .get(0)
            .ok_or_else(|| anyhow!("Funding transaction has no output 0"))?;

        let lock_script = spillman_cell.lock();
        let args_bytes: Bytes = lock_script.args().unpack();

        let timeout_since = u64::from_le_bytes(
            args_bytes[40..48]
                .try_into()
                .map_err(|_| anyhow!("Failed to parse timeout_since from args"))?,
        );

        let input = CellInput::new_builder()
            .previous_output(
                OutPoint::new_builder()
                    .tx_hash(self.request.funding_tx_hash.pack())
                    .index(0u32)
                    .build(),
            )
            .since(timeout_since)
            .build();

        let mut outputs = vec![
            CellOutput::new_builder()
                .capacity(Capacity::shannons(user_capacity))
                .lock(self.request.user_lock_script.clone())
                .build(),
        ];

        let mut outputs_data = vec![Bytes::new().pack()];

        if let Some(ref merchant_lock) = self.request.merchant_lock_script {
            outputs.push(
                CellOutput::new_builder()
                    .capacity(Capacity::shannons(merchant_capacity))
                    .lock(merchant_lock.clone())
                    .build(),
            );
            outputs_data.push(Bytes::new().pack());
        }

        let witness_size = calculate_refund_witness_size(self.context.merchant_multisig_config.as_ref());
        let witness_placeholder = vec![0u8; witness_size];

        let tx = Transaction::default()
            .as_advanced_builder()
            .input(input)
            .cell_dep(self.context.spillman_lock_dep.clone())
            .cell_dep(self.context.auth_dep.clone())
            .set_outputs(outputs)
            .set_outputs_data(outputs_data)
            .witness(Bytes::from(witness_placeholder).pack())
            .build();

        Ok(tx)
    }
}

/// Compute signing message for Spillman Lock
///
/// Spillman Lock signs the raw transaction without cell_deps
fn compute_signing_message(tx: &TransactionView) -> [u8; 32] {
    let raw_tx = tx
        .data()
        .raw()
        .as_builder()
        .cell_deps(CellDepVec::default())
        .build();

    blake2b_256(raw_tx.as_slice())
}

/// Build refund transaction (high-level API)
///
/// This function:
/// - Creates RefundRequest and RefundContext
/// - Builds the transaction with iterative fee calculation
/// - Returns unsigned transaction (to be signed by merchant first, then user)
///
/// Returns: (tx_hash, TransactionView)
///
/// # Arguments
/// * `config` - Configuration
/// * `funding_tx_hash` - The funding transaction hash
/// * `funding_tx` - The funding transaction
/// * `user_address` - User's refund destination address
/// * `merchant_address` - Merchant's refund destination address (optional, for co-fund)
/// * `output_path` - Path to save the transaction JSON
pub async fn build_refund_transaction(
    config: &Config,
    funding_tx_hash: H256,
    funding_tx: &TransactionView,
    user_address: &Address,
    merchant_address: Option<&Address>,
    fee_rate: u64,
    output_path: &str,
) -> Result<(H256, TransactionView)> {
    println!("📝 构建 Refund 交易...");

    let user_lock_script = Script::from(user_address);
    let merchant_lock_script = merchant_address.map(Script::from);

    // Get cell deps
    let spillman_tx_hash = hex::decode(config.spillman_lock.tx_hash.trim_start_matches("0x"))?;
    let spillman_out_point = OutPoint::new_builder()
        .tx_hash(ckb_types::packed::Byte32::from_slice(&spillman_tx_hash)?)
        .index(config.spillman_lock.index)
        .build();
    let spillman_dep = CellDep::new_builder()
        .out_point(spillman_out_point)
        .dep_type(DepType::Code)
        .build();

    let auth_tx_hash = hex::decode(config.auth.tx_hash.trim_start_matches("0x"))?;
    let auth_out_point = OutPoint::new_builder()
        .tx_hash(ckb_types::packed::Byte32::from_slice(&auth_tx_hash)?)
        .index(config.auth.index)
        .build();
    let auth_dep = CellDep::new_builder()
        .out_point(auth_out_point)
        .dep_type(DepType::Code)
        .build();

    // Parse keys using ckb-crypto for Spillman Lock signing
    let user_privkey = Privkey::from_str(config.user.private_key.as_ref().expect("User private_key is required"))
        .map_err(|e| anyhow!("Failed to parse user private key: {:?}", e))?;

    // Parse merchant keys and multisig config
    let (merchant_privkeys, merchant_multisig_config) = if config.merchant.is_multisig() {
        // Multisig merchant
        let secret_keys = config.merchant.get_secret_keys()?;
        let (threshold, total) = config.merchant.get_multisig_config()
            .ok_or_else(|| anyhow!("Merchant multisig config is invalid"))?;

        use crate::tx_builder::funding_v2::build_multisig_config;
        let multisig_config = build_multisig_config(&secret_keys, threshold, total)?;

        (Some(secret_keys), Some(multisig_config))
    } else {
        // Single-sig merchant
        let merchant_privkey_str = config.merchant.private_key.as_ref()
            .ok_or_else(|| anyhow!("Merchant private_key is required for single-sig"))?;
        let merchant_secret_key = {
            let key_hex = merchant_privkey_str.trim_start_matches("0x");
            let key_bytes = hex::decode(key_hex)?;
            secp256k1::SecretKey::from_slice(&key_bytes)?
        };
        (Some(vec![merchant_secret_key]), None)
    };

    // Clone merchant_privkeys for signing
    let merchant_privkeys_for_sign = merchant_privkeys.clone();

    // Extract Spillman Lock args from funding transaction
    let spillman_cell = funding_tx
        .outputs()
        .get(0)
        .ok_or_else(|| anyhow!("Funding transaction has no output 0"))?;
    let lock_script = spillman_cell.lock();
    let args_bytes: Bytes = lock_script.args().unpack();
    if args_bytes.len() != 50 {
        return Err(anyhow!(
            "Invalid Spillman Lock args length: expected 50, got {}",
            args_bytes.len()
        ));
    }

    // Create user secret key for RefundContext
    let user_privkey_hex = config.user.private_key.as_ref().expect("User private_key is required");
    let user_privkey_bytes = hex::decode(user_privkey_hex.trim_start_matches("0x"))?;
    let user_secret_key = secp256k1::SecretKey::from_slice(&user_privkey_bytes)?;

    let request = RefundRequest {
        funding_tx_hash,
        funding_tx: funding_tx.clone(),
        user_lock_script,
        merchant_lock_script,
        fee_rate,
    };

    // Clone merchant_multisig_config for later use in signing
    let merchant_multisig_config_for_sign = merchant_multisig_config.clone();

    let context = RefundContext {
        user_secret_key,
        merchant_secret_keys: merchant_privkeys,
        merchant_multisig_config,
        rpc_url: config.network.rpc_url.clone(),
        spillman_lock_dep: spillman_dep,
        auth_dep,
    };

    // Build transaction
    let refund_tx = RefundTx::new().build(request, context).await?;

    // Sign transaction with Spillman Lock witness structure
    println!("🔐 签名 Refund 交易 (Spillman Lock: Merchant + User)...");
    let refund_tx = refund_tx.sign_for_spillman_lock(
        &user_privkey,
        merchant_privkeys_for_sign.as_ref().unwrap(),
        &args_bytes,
        merchant_multisig_config_for_sign.as_ref(),
    )?;

    let tx = refund_tx.into_inner().ok_or_else(|| anyhow!("No transaction"))?;
    let tx_hash = tx.hash();

    // Print summary
    println!("✓ Refund transaction built");
    println!("  - Transaction hash: {:#x}", tx_hash);
    println!("  - Inputs count: {}", tx.inputs().len());
    println!("  - Outputs count: {}", tx.outputs().len());

    if merchant_address.is_some() {
        println!("  - Mode: Co-fund (2 outputs)");
        let user_cap: u64 = tx.outputs().get(0).unwrap().capacity().unpack();
        let merchant_cap: u64 = tx.outputs().get(1).unwrap().capacity().unpack();
        println!("  - User refund: {}", HumanCapacity::from(user_cap));
        println!("  - Merchant refund: {}", HumanCapacity::from(merchant_cap));
    } else {
        println!("  - Mode: Single (1 output)");
        let user_cap: u64 = tx.outputs().get(0).unwrap().capacity().unpack();
        println!("  - User refund: {}", HumanCapacity::from(user_cap));
    }

    // Save transaction
    let tx_json = ckb_jsonrpc_types::TransactionView::from(tx.clone());
    let json_str = serde_json::to_string_pretty(&tx_json)?;

    if let Some(parent) = std::path::Path::new(output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, json_str)?;

    println!("✓ Refund transaction saved: {}", output_path);
    println!("  ✅ Transaction is signed and ready to broadcast after timeout");

    Ok((tx_hash.unpack(), tx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refund_witness_size() {
        // Verify witness size calculation
        let empty_args_size = EMPTY_WITNESS_ARGS.len();
        let unlock_type_size = 1;
        let merchant_sig_size = 65;
        let user_sig_size = 65;

        assert_eq!(
            empty_args_size + unlock_type_size + merchant_sig_size + user_sig_size,
            REFUND_WITNESS_SIZE_SINGLE_SIG
        );
    }
}

