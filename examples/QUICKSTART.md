# Spillman Channel CLI Tool - Quick Start Guide

A complete command-line interface for managing Spillman one-way payment channels on CKB.

## 🎯 Overview

The Spillman Channel CLI implements a complete one-way payment channel workflow:

1. **Phase 1: Channel Setup**
   - Construct refund transaction (timeout path)
   - Merchant pre-signs refund transaction (guarantees user can refund after timeout)
   - Construct and broadcast funding transaction with Spillman Lock

2. **Phase 2: Off-chain Payments**
   - User generates commitment transactions
   - Each commitment increases payment to merchant
   - All transactions off-chain, zero fees

3. **Phase 3: Settlement**
   - **Option A:** Merchant settles with latest commitment (normal case)
   - **Option B:** User broadcasts pre-signed refund after timeout

## 📋 Prerequisites

### 1. Deployed Contract

The Spillman Lock contract is already deployed on CKB Testnet:

```toml
[spillman_lock]
code_hash = "0x41fa54ee27a517db245b014116fe2baff1dcb639d42fc14be43c315ea3cef9f2"
hash_type = "type"
tx_hash = "0x3ad0f4b3f08927b79d8a94bbad5f265694e969ab2ddfde178893e1c6a954dd5f"
index = 0

[auth]
tx_hash = "0x3f0fe5376b847b0c286184bb59d38765841e135d7d64f87b2bf7014c6316eee2"
index = 0
```

### 2. Test Accounts

You need two accounts with CKB on testnet:

- **User** (payer) - needs ~1100 CKB for channel + fees
- **Merchant** (payee) - needs minimal balance for transactions

Get testnet CKB from: [CKB Testnet Faucet](https://faucet.nervos.org/)

### 3. Generate Keys

Use `ckb-cli` to generate accounts:

```bash
# Generate user account
ckb-cli account new

# Generate merchant account
ckb-cli account new
```

## 🚀 Quick Start

### Step 1: Configuration

Copy the template and configure:

```bash
cd examples
cp config.toml.example config.toml
```

Edit `config.toml`:

```toml
[network]
rpc_url = "https://testnet.ckb.dev"

[user]
# User's private key (without 0x prefix)
private_key = "your_user_private_key_here"
address = "ckt1..."

[merchant]
# Merchant's private key (without 0x prefix)
private_key = "your_merchant_private_key_here"
address = "ckt1..."

[spillman_lock]
code_hash = "0x41fa54ee27a517db245b014116fe2baff1dcb639d42fc14be43c315ea3cef9f2"
hash_type = "type"
tx_hash = "0x3ad0f4b3f08927b79d8a94bbad5f265694e969ab2ddfde178893e1c6a954dd5f"
index = 0

[auth]
tx_hash = "0x3f0fe5376b847b0c286184bb59d38765841e135d7d64f87b2bf7014c6316eee2"
index = 0
```

### Step 2: Build the CLI

```bash
cd examples
cargo build --release
```

The binary will be at `../target/release/spillman-cli`

### Step 3: Create Channel (Set-up)

```bash
# Create a 1000 CKB channel with 24-day timeout
spillman-cli set-up --co-fund --use-v2
```

### Step 4: Make Payments (Off-chain)

Create commitment transactions (off-chain, zero fees):

```bash
# Payment 1: Pay 100 CKB to merchant
spillman-cli pay \
  --amount 100 \
  --channel-file secrets/channel_info.json

# Payment 2: Pay 200 CKB (cumulative)
spillman-cli pay \
  --amount 200 \
  --channel-file secrets/channel_info.json \
  --config config.toml

# Payment 3: Pay 300 CKB (cumulative)
spillman-cli pay \
  --amount 300 \
  --channel-file secrets/channel_info.json \
  --config config.toml
```

**Important:** Each new payment amount must be greater than the previous one!

### Step 5A: Merchant Settlement (Normal Case)

Merchant settles with the latest commitment:

```bash
spillman-cli settle \
  --tx-file secrets/commitment_300_ckb_1762228200.json \
  --config config.toml
```

### Step 5B: User Refund (Timeout Case)

If merchant doesn't settle, user can refund after timeout:

```bash
spillman-cli refund --tx-file /Users/shaojunda/apps/app5/spillman-channel-add-example/examples/secrets/funding_tx_signed.json --use-v2
```

## 🔍 Key Concepts

### Spillman Lock Args (50 bytes)

```
[merchant_pubkey_hash: 20 bytes]  // Merchant's pubkey hash (Blake2b-160)
[user_pubkey_hash: 20 bytes]      // User's pubkey hash (Blake2b-160)
[timeout_timestamp: 8 bytes]      // Timeout in seconds (little-endian)
[version: 2 bytes]                // Version (0x0100)
```

### Two Unlock Paths

**1. Commitment Path (Payment)**
- **Unlocker:** Merchant
- **When:** Anytime before timeout
- **Witness:** `[EMPTY_WITNESS_ARGS][0x00][merchant_sig][user_sig]`
- **Outputs:**
  - Output 0: User (change)
  - Output 1: Merchant (payment + min capacity)

**2. Timeout Path (Refund)**
- **Unlocker:** User
- **When:** After timeout
- **Witness:** `[EMPTY_WITNESS_ARGS][0x01][merchant_sig_presigned][user_sig]`
- **Outputs:**
  - Output 0: User (full refund)

### Timestamp Format

The `timeout_timestamp` uses **seconds-level Unix timestamp** with CKB's "median of previous 37 blocks" rule:

- Not a specific block timestamp
- Calculated as median of previous 37 block headers
- Prevents miner manipulation
- More stable than individual block timestamps

**Reference:** [CKB RFC-0017: Transaction Valid Since](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0017-tx-valid-since/0017-tx-valid-since.md)


## ⚠️ Important Notes

### Security

- 🔐 **NEVER** commit private keys to Git
- 🔐 **NEVER** use test keys on mainnet
- 🔐 Add `config.toml` and `secrets/` to `.gitignore`
- 💰 Test with small amounts first

### Timeout Recommendations

| Use Case | Timeout (seconds) | Approximate | Description |
|----------|------------------|-------------|-------------|
| Testing | 86,400 | 1 day | Quick testing |
| Short-term | 604,800 | 7 days | Week-long channel |
| Standard | 2,073,600 | 24 days | Recommended |
| Long-term | 7,776,000 | 90 days | Quarterly channel |


### Channel Capacity Planning

Consider minimum occupied capacity when planning channel size:

```
Usable Capacity = Channel Capacity - Merchant Min Capacity - Fees
```

For 1000 CKB channel:
- ~61 CKB reserved for merchant's minimum capacity
- ~0.0001 CKB for fees
- **~938.9999 CKB** available for payments

## 🔗 Resources

- [Spillman Lock Design Document](../docs/spillman-lock-design.md)
- [CKB Transaction Structure](https://docs.nervos.org/docs/basics/concepts/transaction/)
- [CKB Cell Model](https://docs.nervos.org/docs/basics/concepts/cell-model/)
- [CKB Testnet Faucet](https://faucet.nervos.org/)
- [CKB Explorer (Testnet)](https://pudge.explorer.nervos.org/)

## 🤝 Contributing

Issues and Pull Requests are welcome!

## 📄 License

MIT License
