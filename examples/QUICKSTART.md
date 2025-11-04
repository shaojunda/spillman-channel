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
code_hash = "0x895a2daeaa274daadfd02b0976e5762e50bec04c4902b4f85fc99f7912cc1277"
hash_type = "type"
tx_hash = "0x895a2daeaa274daadfd02b0976e5762e50bec04c4902b4f85fc99f7912cc1277"
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
code_hash = "0x895a2daeaa274daadfd02b0976e5762e50bec04c4902b4f85fc99f7912cc1277"
hash_type = "type"
tx_hash = "0x895a2daeaa274daadfd02b0976e5762e50bec04c4902b4f85fc99f7912cc1277"
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

**Output:**
```
═══════════════════════════════════════════════════════
  🚀 创建 Spillman 支付通道
═══════════════════════════════════════════════════════

📋 加载配置...
✓ 配置加载完成

👤 用户地址: ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsq...
🏪 商户地址: ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsq...

🔐 生成密钥和参数...
✓ 用户公钥哈希: 0x0ab3eb4f27290496c3685a2af01585d7ddf61ceb
✓ 商户公钥哈希: 0x4475cce6c406033c9141a5308e8672192153a358

⏰ 时间参数:
  - 当前时间戳: 1736005200 (2025-01-04 12:00:00 UTC)
  - 超时时间戳: 1738078800 (2025-01-28 12:00:00 UTC)
  - 超时时长: 24 天 (2,073,600 秒)

📝 构建 Spillman Lock Script...
✓ Spillman Lock Script 已创建

📝 Step 1: 构建 Refund Transaction (超时退款路径)...
✓ Refund transaction built
  - Inputs: 1 (Spillman Lock cell)
  - Outputs: 1 (User refund)
  - Mode: Single (1 output)

🔐 Step 2: 商户和用户预签 Refund 交易...
✓ Merchant signature added
✓ User signature added
✓ Refund transaction saved: secrets/refund_tx_1762228000.json
  ⚠️  This guarantees user can refund after timeout!

📝 Step 3: 构建 Funding Transaction...
✓ Funding transaction built
  - Transaction hash: 0x2e57d66cbc26e863afd7903b60ab789d0e98cd557f7f2a2b0c066b9b3ad8dd00
  - Inputs: User's cells
  - Outputs: [0] Spillman Lock (1000 CKB), [1] User change

✓ Funding transaction saved: secrets/funding_tx_signed.json
✓ Channel info saved: secrets/channel_info.json

✅ 通道创建成功！

📌 安全保证：
  ✓ Refund 交易已由商户预签
  ✓ 用户可在超时后取回全部资金
  ✓ 现在可以安全地广播 Funding 交易
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

**Output:**
```
═══════════════════════════════════════════════════════
  💸 创建 Commitment Transaction (链下支付)
═══════════════════════════════════════════════════════

📋 加载配置...
✓ 配置加载完成

📂 加载通道信息...
✓ 通道信息:
  - 用户地址: ckt1...
  - 商户地址: ckt1...
  - 通道容量: 1000 CKB
  - Funding TX: 0x2e57d66cbc...
  - Output Index: 0

🔍 从链上查询 Spillman Lock cell...
✓ Spillman Lock cell 信息:
  - Capacity: 1000 CKB
  - Script hash: 0x...

💰 支付详情:
  - 商户最小占用容量: 61 CKB (61 shannons)
  - 用户支付金额: 100 CKB
  - 商户实际收到: 161 CKB (100 支付 + 61 最小占用)

📝 构建 Commitment 交易...
✓ Commitment transaction built
  - Transaction hash: 0x29e9d1acd72327b29de5bc3a5a6c6e446e2c482a11901eff924364a3d5b01fea
  - Payment to merchant: 100 CKB (payment) + 61 CKB (min capacity) = 161 CKB
  - Change to user: 838 CKB
  - Estimated fee: 0.00001 CKB

✓ Commitment transaction saved: secrets/commitment_100_ckb_1762228100.json

✅ Commitment Transaction 创建成功!
```

**Important:** Each new payment amount must be greater than the previous one!

### Step 5A: Merchant Settlement (Normal Case)

Merchant settles with the latest commitment:

```bash
spillman-cli settle \
  --tx-file secrets/commitment_300_ckb_1762228200.json \
  --config config.toml
```

**Output:**
```
═══════════════════════════════════════════════════════
  🏦 商户结算 Commitment Transaction
═══════════════════════════════════════════════════════

📋 加载配置...
✓ 配置加载完成

🔑 加载商户私钥...
✓ 商户私钥加载完成

📄 加载 Commitment 交易: secrets/commitment_300_ckb_1762228200.json
✓ 交易加载完成
  - TX Hash: 0x29e9d1acd72327b29de5bc3a5a6c6e446e2c482a11901eff924364a3d5b01fea
  - Inputs: 1
  - Outputs: 2

✓ Witness 结构验证通过

🔐 商户签名交易...
✓ 签名完成
✓ 交易签名更新完成
  - New TX Hash: 0x5f8e7d6c5b4a39281f0e9d8c7b6a59483f2e1d0c9b8a79685f4e3d2c1b0a9988

📡 广播交易到链上...
✓ 交易已广播
  - TX Hash: 0x5f8e7d6c5b4a39281f0e9d8c7b6a59483f2e1d0c9b8a79685f4e3d2c1b0a9988

✅ 结算成功！

📌 后续操作:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

🔍 查询交易状态：
  ckb-cli rpc get_transaction --hash 0x5f8e7d6c...

⏳ 等待交易上链确认...
  交易确认后，支付金额将到达商户地址
```

**Result:**
- Merchant receives: 361 CKB (300 payment + 61 minimum capacity)
- User receives: 638 CKB (change)
- Channel closed ✓

### Step 5B: User Refund (Timeout Case)

If merchant doesn't settle, user can refund after timeout:

```bash
spillman-cli refund --tx-file /Users/shaojunda/apps/app5/spillman-channel-add-example/examples/secrets/funding_tx_signed.json --use-v2
```

**Output:**
```
📝 构建 Refund 交易...
✓ Refund transaction built
  - Transaction hash: 0x...
  - Inputs count: 1
  - Outputs count: 1
  - Mode: Single (1 output)
  - User refund: 999 CKB

🔐 签名 Refund 交易 (Spillman Lock: Merchant + User)...
✓ Refund transaction saved: output/refund_tx_v2.json

✅ Transaction is signed and ready to broadcast after timeout
```

**Result:**
- User receives: ~999 CKB (full refund minus fees)
- Merchant receives: 0 CKB

## 📊 Command Reference

### `set-up` - Create Channel

Creates a new payment channel with Spillman Lock.

**⚠️ Critical Security Flow:**
1. Constructs refund transaction (timeout path)
2. Merchant pre-signs refund transaction
3. User signs refund transaction
4. Constructs and broadcasts funding transaction

This order guarantees the user can always recover funds after timeout, even if merchant becomes uncooperative.

```bash
spillman-cli set-up \
  --user-address <USER_ADDRESS> \
  --merchant-address <MERCHANT_ADDRESS> \
  --capacity-ckb <AMOUNT> \
  --timeout-timestamp <SECONDS> \
  --config <CONFIG_FILE> \
  [--co-fund]
```

**Parameters:**
- `--user-address`: User's CKB address
- `--merchant-address`: Merchant's CKB address (optional, uses user address if omitted)
- `--capacity-ckb`: Channel capacity in CKB
- `--timeout-timestamp`: Timeout duration in seconds (e.g., 2073600 = 24 days)
- `--config`: Path to config file (default: `config.toml`)
- `--co-fund`: Enable co-funding mode (both parties contribute)

**Outputs:**
- `secrets/refund_tx_<timestamp>.json` - **Pre-signed refund transaction** (created first!)
- `secrets/funding_tx_signed.json` - Signed funding transaction
- `secrets/channel_info.json` - Channel metadata

**Security Guarantee:**
The refund transaction is constructed and fully signed (by both merchant and user) BEFORE the funding transaction is broadcast. This ensures:
- ✅ User can always refund after timeout
- ✅ Merchant cannot hold funds hostage
- ✅ Trust-minimized channel setup

### `pay` - Create Payment

Creates a commitment transaction for off-chain payment.

```bash
spillman-cli pay \
  --amount <CKB_AMOUNT> \
  --channel-file <CHANNEL_INFO_FILE> \
  --config <CONFIG_FILE>
```

**Parameters:**
- `--amount`: Payment amount in CKB (must be greater than previous payments)
- `--channel-file`: Path to channel info file (default: `secrets/channel_info.json`)
- `--config`: Path to config file (default: `config.toml`)

**Outputs:**
- `secrets/commitment_<amount>_ckb_<timestamp>.json` - Signed commitment transaction

**Notes:**
- Payment is off-chain, zero fees
- Each payment must exceed the previous amount
- User signature is added automatically
- Merchant adds signature during settlement

### `settle` - Merchant Settlement

Merchant settles a commitment transaction on-chain.

```bash
spillman-cli settle \
  --tx-file <COMMITMENT_FILE> \
  --config <CONFIG_FILE>
```

**Parameters:**
- `--tx-file`: Path to commitment transaction file
- `--config`: Path to config file (default: `config.toml`)

**Notes:**
- Adds merchant signature to commitment
- Broadcasts transaction to CKB network
- Closes the channel
- Merchant receives payment + minimum capacity
- User receives change

### `refund` - User Refund

User refunds channel funds after timeout.

```bash
spillman-cli refund \
  --tx-file <FUNDING_TX_FILE> \
  --config <CONFIG_FILE> \
  --use-v2
```

**Parameters:**
- `--tx-file`: Path to funding transaction file
- `--config`: Path to config file (default: `config.toml`)
- `--use-v2`: Use refund v2 builder (recommended)

**Notes:**
- Only works after timeout period
- Uses pre-signed refund transaction from setup
- Returns full channel capacity to user
- Merchant loses all potential income

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

### Minimum Occupied Capacity

CKB requires each cell to have minimum capacity based on its size:

```
Minimum Capacity = Cell Size (bytes) × 1 CKB
```

For a typical lock script (~61 bytes):
- Merchant receives: **Payment Amount + ~61 CKB**
- User's change: **Channel Capacity - (Payment + Min Capacity) - Fee**

**Example:**
- Channel: 1000 CKB
- Payment: 100 CKB
- Merchant gets: 100 + 61 = **161 CKB**
- User gets: 1000 - 161 - 0.00001 = **838.99999 CKB**

### Timestamp Format

The `timeout_timestamp` uses **seconds-level Unix timestamp** with CKB's "median of previous 37 blocks" rule:

- Not a specific block timestamp
- Calculated as median of previous 37 block headers
- Prevents miner manipulation
- More stable than individual block timestamps

**Reference:** [CKB RFC-0017: Transaction Valid Since](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0017-tx-valid-since/0017-tx-valid-since.md)

## 📁 Project Structure

```
examples/
├── config.toml.example      # Configuration template
├── config.toml              # Your configuration (gitignored)
├── QUICKSTART.md            # This guide
├── Cargo.toml               # Rust project manifest
├── secrets/                 # Generated transactions (gitignored)
│   ├── channel_info.json
│   ├── funding_tx_signed.json
│   ├── refund_tx_*.json
│   └── commitment_*_ckb_*.json
└── src/
    ├── main.rs              # CLI entry point
    ├── commands/            # Command implementations
    │   ├── setup.rs         # Channel setup
    │   ├── pay.rs           # Payment creation
    │   ├── settle.rs        # Merchant settlement
    │   └── refund.rs        # User refund
    ├── tx_builder/          # Transaction builders
    │   ├── funding_v2.rs    # Funding transaction
    │   ├── commitment.rs    # Commitment transaction
    │   └── refund_v2.rs     # Refund transaction
    ├── storage/             # Transaction storage
    │   └── tx_storage.rs
    └── utils/               # Utilities
        ├── config.rs        # Configuration loading
        ├── crypto.rs        # Cryptography helpers
        └── rpc.rs           # RPC helpers
```

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

### Transaction Fees

- **Setup:** ~1 CKB (includes refund transaction)
- **Payment:** 0 CKB (off-chain)
- **Settlement:** ~0.00001 CKB
- **Refund:** ~0.00001 CKB

### Channel Capacity Planning

Consider minimum occupied capacity when planning channel size:

```
Usable Capacity = Channel Capacity - Merchant Min Capacity - Fees
```

For 1000 CKB channel:
- ~61 CKB reserved for merchant's minimum capacity
- ~0.0001 CKB for fees
- **~938.9999 CKB** available for payments

## 🛠️ Troubleshooting

### "Invalid funding tx hash: Invalid length"

**Cause:** Transaction hash includes `0x` prefix

**Solution:** The tool automatically handles `0x` prefix. Check your `channel_info.json` format.

### "Merchant signature already present"

**Cause:** Trying to settle an already-settled transaction

**Solution:** Use a different commitment transaction or create a new channel.

### "Timeout not reached"

**Cause:** Trying to refund before timeout period

**Solution:** Wait until current timestamp > timeout_timestamp.

### "Insufficient capacity"

**Cause:** Payment amount + minimum capacity exceeds channel capacity

**Solution:**
- Reduce payment amount, or
- Create a larger channel

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
