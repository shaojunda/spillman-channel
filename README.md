# Spillman Channel

A unidirectional payment channel implementation on CKB, based on the Spillman Channel design.

## What is Spillman Channel?

Spillman Channel is one of the earliest payment channel designs (2013), enabling high-frequency, off-chain micropayments without on-chain transaction fees. It's particularly suited for scenarios like:

- Content streaming payments (pay-per-second)
- Gaming micropayments
- IoT device payments
- Any use case requiring frequent small transactions

## Key Features

- **Unidirectional payments**: Funds flow only from user → merchant
- **Off-chain transactions**: All commitment transactions happen off-chain (zero fees)
- **Timeout protection**: Users can always reclaim funds after timeout
- **xUDT support**: Works with both CKB and xUDT tokens
- **Multisig support**: Supports merchant multisig addresses (Legacy & V2)
- **Economic incentives**: Merchant is incentivized to settle before timeout

## How It Works

```
1. Channel Setup
   - User creates refund transaction (timeout protected)
   - Merchant pre-signs refund transaction (insurance)
   - User broadcasts funding transaction (channel opens)

2. Off-chain Payments
   User creates commitment transactions with increasing amounts:
   - Commitment 1: merchant gets 100 CKB
   - Commitment 2: merchant gets 300 CKB
   - Commitment 3: merchant gets 500 CKB
   (All happen off-chain, no fees)

3. Settlement
   Option A: Merchant settles (normal case)
   - Merchant broadcasts latest commitment
   - Gets payment (e.g., 500 CKB)

   Option B: User refunds (timeout case)
   - User waits for timeout
   - Broadcasts refund transaction
   - Gets full amount back (merchant loses income)
```

## Contract Specifications

### Lock Script Args (50 bytes)
```
[merchant_lock_arg(20)] + [user_pubkey_hash(20)] + [timeout(8)] + [algorithm_id(1)] + [version(1)]
```

### Unlock Paths

**Commitment Path (0x00)**
- Merchant can settle anytime before timeout
- Requires both merchant and user signatures
- Must have exactly 2 outputs: user (change) + merchant (payment)

**Timeout Path (0x01)**
- User can refund after timeout
- Requires both signatures (merchant pre-signed)
- Has 1-2 outputs: user (full refund) + optional merchant capacity

### Supported Algorithm IDs
- `0`: Single-sig (secp256k1_blake160_sighash_all)
- `6`: Multisig Legacy (secp256k1_blake160_multisig_all, hash_type=Type)
- `7`: Multisig V2 (secp256k1_blake160_multisig_all, hash_type=Data1)

## Project Structure

```
spillman-channel/
├── contracts/
│   └── spillman-lock/        # Main lock script
├── tests/                    # Integration tests (15 test cases)
├── docs/                     # Design documentation
└── migrations/               # Deployment scripts
```

## Development

### Prerequisites

- Rust 1.71.0+
- [ckb-cli](https://github.com/nervosnetwork/ckb-cli)

### Build

```bash
# Build all contracts
make build

# Run tests
make test

# Check contract size
ls -lh build/release/spillman-lock
```

### Test Coverage

The project includes comprehensive test coverage with 15 test cases:

- ✅ Commitment path (single-sig & multisig)
- ✅ Timeout path (single-sig & multisig)
- ✅ xUDT support (commitment & refund)
- ✅ Co-funding scenarios
- ✅ Error cases (invalid outputs, witness, args, etc.)
- ✅ Multiple inputs validation
- ✅ Timestamp-based timeout

All tests pass with 100% success rate.

## Deployment

Contracts are deployed on:
- **Testnet**: See [`testnet/migrations`](./deployment/testnet/migrations/)
- **Mainnet**: TBD

## Security Considerations

1. **Timeout Safety**: Use reasonable timeout periods (e.g., 7 days)
2. **Fee Management**: Pre-fund transaction fees in channel capacity
3. **Signature Verification**: Both parties must verify all signatures
4. **Amount Validation**: Commitment amounts must be monotonically increasing

## Documentation

- [Design Document](./docs/spillman-lock-design.md) - Detailed technical design (Chinese)
- [Spillman vs Bitcoin Wiki Example 7](./docs/bitcoin-wiki-example7-vs-spillman.md)
- [Web3 Payment Experiment](https://talk.nervos.org/t/web3/9621) (Chinese)

## References

- [Bitcoin Wiki - Payment Channels](https://en.bitcoin.it/wiki/Payment_channels)
- [CKB Auth Protocol](https://github.com/nervosnetwork/ckb-auth)
- [ckb-script-templates](https://github.com/cryptape/ckb-script-templates)

## License

MIT

---

*This project was bootstrapped with [ckb-script-templates](https://github.com/cryptape/ckb-script-templates).*

