#!/bin/bash

echo "=== Step 1: Generate and sign transactions ==="
ckb-cli --url https://testnet.ckb.dev deploy gen-txs \
    --deployment-config ./testnet.toml \
    --migration-dir ./migrations \
    --from-address ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqwv48lup30y6ap3al3hgvk32ckmrpnsu9szgrgrq \
    --sign-now \
    --info-file testnet.json

echo ""
echo "=== Step 2: Broadcast transactions to chain ==="
ckb-cli --url https://testnet.ckb.dev deploy apply-txs \
    --info-file testnet.json \
    --migration-dir ./migrations

echo ""
echo "=== Deployment completed! ==="
echo "Check testnet.json for contract details"
