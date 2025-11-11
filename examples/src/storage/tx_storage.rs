use anyhow::Result;
use ckb_types::core::TransactionView;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Transaction information for storage
#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionInfo {
    /// Transaction type: "funding", "refund", "commitment"
    pub tx_type: String,

    /// Transaction in JSON format
    pub transaction: ckb_jsonrpc_types::TransactionView,

    /// Additional metadata
    pub metadata: TransactionMetadata,
}

/// Transaction metadata
#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionMetadata {
    /// Timestamp when created
    pub created_at: u64,

    /// Whether the transaction is signed
    pub signed: bool,

    /// Who has signed (for multi-sig transactions)
    pub signers: Vec<String>,

    /// Payment amount (for commitment transactions)
    pub payment_amount: Option<u64>,

    /// Channel capacity
    pub channel_capacity: Option<u64>,
}

impl TransactionInfo {
    pub fn new(tx_type: &str, transaction: TransactionView, signed: bool) -> Self {
        Self {
            tx_type: tx_type.to_string(),
            transaction: ckb_jsonrpc_types::TransactionView::from(transaction),
            metadata: TransactionMetadata {
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                signed,
                signers: Vec::new(),
                payment_amount: None,
                channel_capacity: None,
            },
        }
    }

    pub fn with_payment_amount(mut self, amount: u64) -> Self {
        self.metadata.payment_amount = Some(amount);
        self
    }

    pub fn with_channel_capacity(mut self, capacity: u64) -> Self {
        self.metadata.channel_capacity = Some(capacity);
        self
    }

    pub fn add_signer(mut self, signer: &str) -> Self {
        self.metadata.signers.push(signer.to_string());
        self
    }
}

/// Save transaction to file
pub fn save_transaction(tx_info: &TransactionInfo, file_path: &str) -> Result<()> {
    let json = serde_json::to_string_pretty(tx_info)?;

    // Ensure parent directory exists
    if let Some(parent) = Path::new(file_path).parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(file_path, json)?;
    println!("✓ Transaction saved to: {}", file_path);

    Ok(())
}

/// Load transaction from file
pub fn load_transaction(file_path: &str) -> Result<TransactionInfo> {
    let json = fs::read_to_string(file_path)?;
    let tx_info: TransactionInfo = serde_json::from_str(&json)?;
    Ok(tx_info)
}

/// Generate filename for transaction
pub fn generate_tx_filename(tx_type: &str, suffix: Option<&str>) -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    if let Some(s) = suffix {
        format!("secrets/{}_{}_{}.json", tx_type, s, timestamp)
    } else {
        format!("secrets/{}_{}.json", tx_type, timestamp)
    }
}
