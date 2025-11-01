#!/bin/bash

# Check if testnet.json exists
if [ -f "testnet.json" ]; then
    echo "⚠️  警告: testnet.json 已存在！"
    echo "这个文件包含之前的部署信息。如果继续，将会创建新的部署。"
    echo ""
    read -p "是否删除现有文件并继续？(y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "❌ 部署已取消"
        exit 1
    fi
    echo "🗑️  删除 testnet.json..."
    rm testnet.json
    echo ""
fi

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
