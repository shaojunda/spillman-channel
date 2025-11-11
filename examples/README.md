# Spillman Channel Examples

This directory contains examples demonstrating the full lifecycle of a Spillman Channel on CKB Testnet.

## 📚 Documentation

- **[QUICKSTART.md](./QUICKSTART.md)** - Complete usage guide

## 🚀 Quick Start

### 1. Configure

```bash
cd examples
cp config.toml.example config.toml
# Edit config.toml with your keys and addresses
```

## Prerequisites

1. **Deploy the contract**: Deploy Spillman Lock contract first
   ```bash
   cd deployment
   # Follow deployment/README.md
   ```

2. **Configure**: Update `config.toml` with:
   - User private key and address
   - Merchant private key and address
   - Spillman Lock contract code_hash (from deployment)
   - Channel parameters (capacity, timeout, fee)

3. **Test CKB**: Get testnet CKB from [faucet](https://faucet.nervos.org/)

## What the Example Does

The example demonstrates the complete Spillman Channel flow:

### Phase 1: Channel Creation
1. User constructs a refund transaction (with timeout lock)
2. Merchant signs the refund transaction
3. User broadcasts the funding transaction

### Phase 2: Off-chain Payments
1. User creates commitment transaction #1 (100 CKB to merchant)
2. User creates commitment transaction #2 (300 CKB to merchant)
3. User creates commitment transaction #3 (500 CKB to merchant)
4. Each commitment is signed by user and verified by merchant

### Phase 3A: Merchant Settlement
1. Merchant takes the latest commitment (500 CKB)
2. Merchant adds their signature
3. Merchant broadcasts the transaction on-chain
4. Merchant receives 500 CKB, user receives 500 CKB change

### Phase 3B: User Refund (Alternative)
1. If merchant doesn't settle before timeout
2. User waits for timeout epoch
3. User broadcasts the refund transaction (pre-signed by merchant)
4. User receives full refund (1000 CKB)

## Key Concepts

- **Off-chain Payments**: Commitments are created off-chain, saving transaction fees
- **Economic Incentive**: Merchant always uses the latest commitment (highest payment)
- **User Protection**: Refund transaction ensures user can recover funds after timeout
- **2-of-2 Multisig**: All unlocks require both user and merchant signatures

