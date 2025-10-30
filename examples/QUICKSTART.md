# Spillman Channel Full Flow Example - Quick Start

A complete demonstration of the Spillman Channel workflow, from channel creation to settlement.

## 🎯 Features

This example implements the three phases of Spillman Channel:

1. **Phase 1: Channel Setup**
   - User constructs refund transaction
   - Merchant pre-signs refund transaction
   - User broadcasts funding transaction, creating Spillman Lock cell

2. **Phase 2: Off-chain Payments**
   - User creates multiple commitment transactions
   - Each new commitment increases payment to merchant
   - All transactions off-chain, zero fees

3. **Phase 3: Settlement**
   - **Option A**: Merchant settles with latest commitment (normal case)
   - **Option B**: User refunds after timeout (merchant didn't settle)

## 📋 Prerequisites

### 1. Deploy Spillman Lock Contract

First, deploy the Spillman Lock contract to CKB Testnet:

```bash
# In project root directory
cd /Users/shaojunda/apps/app5/spillman-channel

# Build contract
make build

# Deploy to testnet (configure deployment/config.toml first)
cd deployment
# Follow deployment/README.md instructions
```

After successful deployment, you'll get the contract's `code_hash` and `tx_hash`.

### 2. Prepare Test Accounts

You need two test accounts:

- **User**: Payer, needs sufficient CKB balance
- **Merchant**: Payee

Get testnet CKB from:
- [CKB Testnet Faucet](https://faucet.nervos.org/)

### 3. Generate Private Keys and Addresses

Use CKB CLI or other tools:

```bash
# Generate account with ckb-cli
ckb-cli account new
```

## 🚀 Usage Steps

### Step 1: Configuration

Copy the configuration template and fill in actual values:

```bash
cd examples
cp config.toml.example config.toml
```

Edit `config.toml`:

```toml
[network]
rpc_url = "https://testnet.ckb.dev"

[user]
# User's private key (remove 0x prefix)
private_key = "your_user_private_key_here"
# User's address
address = "ckt1..."

[merchant]
# Merchant's private key (remove 0x prefix)
private_key = "your_merchant_private_key_here"
# Merchant's address
address = "ckt1..."

[channel]
# Channel capacity in CKB
capacity_ckb = 1000
# Timeout in epochs (144 epochs ≈ 24 days, 1 epoch ≈ 4 hours)
timeout_epochs = 144
# Transaction fee in shannon (1 CKB = 100000000 shannon)
tx_fee_shannon = 100000000

[spillman_lock]
# Spillman Lock contract code hash (obtained after deployment)
code_hash = "0x..."
# Hash type: type/data/data1/data2
hash_type = "type"
```

### Step 2: Run Example

```bash
# In examples directory
cargo run --bin full_flow
```

### Step 3: View Output

The program will output complete flow information:

```
🚀 Spillman Channel Full Flow Example
======================================

📋 Loading configuration...
✓ Configuration loaded
✓ User pubkey: 02...
✓ Merchant pubkey: 03...

🔗 Connecting to CKB network...
✓ Connected to https://testnet.ckb.dev
✓ Current epoch: 12345
✓ Timeout epoch: 12489 (+144 epochs)

👤 User address: ckt1...
🏪 Merchant address: ckt1...

🔐 Building Spillman Lock script...
✓ Spillman Lock script created

📝 Phase 1: Channel Setup
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

💰 Channel parameters:
  Capacity: 1000 CKB
  Fee: 1 CKB
  Total required: 1001 CKB

📦 Collecting user's cells...
✓ Collected 2 cells with total capacity: 1500 CKB

🔨 Step 1: Constructing refund transaction...
✓ Refund transaction prepared (merchant pre-signed)

📤 Step 2: Broadcasting funding transaction...
Funding Transaction Structure:
  Inputs:
    [0] 0x1234...5678:0 - 1000 CKB
  Outputs:
    [0] Spillman Lock Cell - 1000 CKB
    [1] User Change Cell - (remaining) CKB

📝 Phase 2: Off-chain Payments (Commitment Transactions)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

💳 Commitment Transaction #1
  Input: Spillman Lock Cell - 1000 CKB
  Outputs:
    [0] User address - 900 CKB (change)
    [1] Merchant address - 100 CKB (payment)
  Witness: User signature ✓
  Status: Signed by user, held by merchant (off-chain)

💳 Commitment Transaction #2
  Input: Spillman Lock Cell - 1000 CKB
  Outputs:
    [0] User address - 700 CKB (change)
    [1] Merchant address - 300 CKB (payment)
  Witness: User signature ✓
  Status: Signed by user, held by merchant (off-chain)

💳 Commitment Transaction #3
  Input: Spillman Lock Cell - 1000 CKB
  Outputs:
    [0] User address - 500 CKB (change)
    [1] Merchant address - 500 CKB (payment)
  Witness: User signature ✓
  Status: Signed by user, held by merchant (off-chain)

✓ Merchant holds the latest commitment (500 CKB payment)

📝 Phase 3: Settlement
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

🏪 Option A: Merchant Settlement (Normal Case)
  1. Merchant takes the latest commitment (500 CKB)
  2. Merchant adds their signature
  3. Merchant broadcasts to CKB network
  Result:
    - Merchant receives: 500 CKB
    - User receives: 500 CKB (change)

⏰ Option B: User Refund (Timeout Case)
  Conditions:
    - Current epoch >= timeout epoch (12489)
    - Merchant did not settle
  Steps:
    1. User waits for timeout
    2. User broadcasts pre-signed refund transaction
    3. User adds their signature
  Result:
    - User receives: 1000 CKB (full refund)
    - Merchant receives: 0 CKB (loses all income)

📊 Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✓ Channel capacity: 1000 CKB
✓ Total payments made: 3 commitments
✓ Final state: 500 CKB to merchant, 500 CKB to user
✓ Timeout protection: 144 epochs

🎉 Spillman Channel flow completed successfully!
```

## 📝 Code Structure

```
examples/
├── config.toml.example      # Configuration template
├── config.toml              # Actual configuration (create yourself)
├── QUICKSTART.md            # This document
└── src/
    └── full_flow.rs         # Main program
```

### Core Modules

`full_flow.rs` contains the following features:

1. **Configuration Management**
   - `load_config()`: Load configuration file
   - `Config` struct: Store all configuration items

2. **Key Handling**
   - `parse_privkey()`: Parse private key
   - `pubkey_hash()`: Calculate pubkey hash (Blake2b-160)

3. **Spillman Lock**
   - `SpillmanLockArgs`: Lock args structure (49 bytes)
   - `build_spillman_lock_script()`: Build Lock script

4. **RPC Interaction**
   - `get_current_epoch()`: Get current epoch
   - `collect_user_cells()`: Collect user's live cells

5. **Transaction Construction**
   - Funding transaction
   - Refund transaction
   - Commitment transactions

## 🔍 Key Concepts

### Spillman Lock Args (49 bytes)

```rust
struct SpillmanLockArgs {
    merchant_pubkey_hash: [u8; 20],  // Merchant pubkey hash (Blake2b-160)
    user_pubkey_hash: [u8; 20],      // User pubkey hash (Blake2b-160)
    timeout_epoch: u64,              // Timeout epoch (little-endian)
    version: u8,                     // Version number
}
```

### Two Unlock Paths

1. **Commitment Path**
   - Unlocker: Merchant
   - Time: Anytime (before timeout)
   - Signatures: User signature + Merchant signature
   - Outputs: Output 0 to user (change), Output 1 to merchant (payment)

2. **Timeout Path**
   - Unlocker: User
   - Time: After timeout
   - Signatures: User signature + Merchant signature (pre-signed)
   - Outputs: Output 0 to user (full refund)

## ⚠️ Important Notes

### Current Implementation Status

This example is currently a **workflow demonstration** version, showcasing the complete Spillman Channel workflow and data structures.

**Completed:**
1. ✅ Configuration management with deployed contract info
2. ✅ Key handling and cryptography
3. ✅ Spillman Lock script construction (49-byte args)
4. ✅ RPC connection and epoch queries
5. ✅ Flow visualization

**To Implement for Production:**
6. ⚠️ Cell collection using DefaultCellCollector
7. ⚠️ Transaction building using CapacityTransferBuilder
8. ⚠️ Transaction signing with SecpCkbRawKeySigner
9. ⚠️ Broadcasting with send_transaction RPC

## 🛠️ Full Implementation Guide

To implement real transactions, use **ckb-sdk-rust** components. The contract is already deployed to testnet with these parameters:

```toml
[spillman_lock]
code_hash = "0x41fa54ee27a517db245b014116fe2baff1dcb639d42fc14be43c315ea3cef9f2"
hash_type = "type"
tx_hash = "0x3f0fe5376b847b0c286184bb59d38765841e135d7d64f87b2bf7014c6316eee2"
index = 1

[auth]
tx_hash = "0x3f0fe5376b847b0c286184bb59d38765841e135d7d64f87b2bf7014c6316eee2"
index = 0
```

### Step-by-Step Implementation

#### 1. Cell Collection

Use `DefaultCellCollector` to gather user's live cells:

```rust
use ckb_sdk::traits::{DefaultCellCollector, CellCollector};

let cell_collector = DefaultCellCollector::new(&ckb_client);
let cells = cell_collector.collect_live_cells(
    &user_lock_script,
    true,  // with_data
)?;
```

#### 2. Build Funding Transaction

Use `CapacityTransferBuilder` to create the funding transaction:

```rust
use ckb_sdk::tx_builder::transfer::CapacityTransferBuilder;

let builder = CapacityTransferBuilder::new(vec![(
    spillman_lock_script.clone(),
    channel_capacity,
)]);
```

#### 3. Balance and Sign

```rust
use ckb_sdk::{
    tx_builder::CapacityBalancer,
    traits::{SecpCkbRawKeySigner, DefaultTransactionDependencyProvider},
    unlock::SecpSighashUnlocker,
};

// Add balancer
let balancer = CapacityBalancer::new_simple(
    user_lock_script.clone(),
    placeholder_witness,
    fee_rate,
);

// Sign
let signer = SecpCkbRawKeySigner::new_with_secret_keys(vec![user_privkey]);
let unlockers = vec![SecpSighashUnlocker::from(Box::new(signer) as Box<_>)];

let tx = balancer.build_balanced(
    &mut cell_collector,
    &cell_dep_resolver,
    &header_dep_resolver,
    &tx_builder,
    &unlockers,
)?;
```

#### 4. Broadcast

```rust
let json_tx: ckb_jsonrpc_types::Transaction = tx.data().into();
let tx_hash = ckb_client.send_transaction(
    json_tx.into(),
    Some(json_types::OutputsValidator::Passthrough),
)?;

println!("Transaction sent: {:?}", tx_hash);
```

### Reference Examples

See complete working examples in ckb-sdk-rust:
- [`transfer_from_sighash.rs`](https://github.com/nervosnetwork/ckb-sdk-rust/blob/master/examples/transfer_from_sighash.rs) - Basic transfer
- [`send_ckb_example.rs`](https://github.com/nervosnetwork/ckb-sdk-rust/blob/master/examples/send_ckb_example.rs) - Complete flow

### Additional Resources

- [CKB SDK Rust Documentation](https://github.com/nervosnetwork/ckb-sdk-rust)
- [Transaction Structure](https://docs.nervos.org/docs/basics/concepts/transaction/)
- [Cell Model](https://docs.nervos.org/docs/basics/concepts/cell-model/)

### Security Reminders

- 🔐 **NEVER** use test private keys on mainnet
- 🔐 **NEVER** commit config files with real private keys to Git
- 🔐 Use `.gitignore` to exclude `config.toml`
- 💰 Use small amounts when testing on testnet

### Fee Explanation

- Funding: reserve fee as `capacity + fee`
- Commitment/Refund: fee = Inputs - Outputs
- Recommended fee: 0.001 - 1 CKB

### Timeout Recommendations

| Scenario | Timeout Epochs | Approximate | Description |
|----------|---------------|-------------|-------------|
| Testing | 10 | ~1.7 days | Quick testing |
| Short-term | 72 | ~12 days | Short-term channel |
| Standard | 144 | ~24 days | Recommended |
| Long-term | 1008 | ~24 weeks | Long-term channel |

⚠️ Epoch duration is approximately 4 hours, but varies with network conditions.

## 🔗 Resources

- [Spillman Lock Design Document](../docs/spillman-lock-design.md)
- [Bitcoin Wiki Example 7 vs Spillman](../docs/bitcoin-wiki-example7-vs-spillman.md)
- [CKB Testnet Faucet](https://faucet.nervos.org/)
- [CKB Explorer (Testnet)](https://pudge.explorer.nervos.org/)
- [CKB Developer Docs](https://docs.nervos.org/)

## 🤝 Contributing

Issues and Pull Requests are welcome!

For production-grade implementation, refer to:
- [CKB SDK Examples](https://github.com/nervosnetwork/ckb-sdk-rust/tree/master/examples)
- [CKB Transaction Builder](https://github.com/nervosnetwork/ckb-sdk-rust/blob/master/src/tx_builder/mod.rs)

## 📄 License

MIT License
