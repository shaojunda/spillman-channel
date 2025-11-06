# Spillman Channel CLI 开发计划

> 实现单向通道 CLI 工具，支持完整的通道生命周期管理
>
> 创建时间: 2025-10-30
> 状态: ✅ 核心功能已完成（纯 CKB 通道）

## 📋 项目目标

实现一个命令行工具来验证 Spillman Channel 的完整流程，包括：
1. 通道准备（单独出资/共同出资）
2. 链下支付
3. 结算（正常结算/超时退款）

## 🎯 功能需求

### 1. set-up 命令 - 通道准备
```bash
# 用户单独出资（使用配置文件）
spillman-cli set-up --config config.toml --output-dir ./secrets

# 用户单独出资（使用 v2 实现）
spillman-cli set-up --config config.toml --output-dir ./secrets --use-v2

# 用户和商户共同出资（co-fund 模式）
spillman-cli set-up --config config.toml --output-dir ./secrets --co-fund --use-v2

# 覆盖配置参数并自动广播
spillman-cli set-up --config config.toml --output-dir ./secrets \
    --capacity 1000 \
    --timeout-timestamp 1735689600 \
    --use-v2 \
    --broadcast
```

**实现内容**：
- [x] 收集用户的 cells（通过 RPC indexer）
- [x] 构造 funding transaction（创建 Spillman Lock cell）
- [x] 构造 refund transaction（商户预签名）
- [x] 保存 refund_tx 到文件（等待用户超时后签名）
- [x] 保存 funding_tx 到文件（支持广播）
- [x] 支持单方出资和 co-fund 模式
- [x] 支持 funding_v2 新实现（TxBuilder 模式）

**输出文件**：
- `funding_tx_<timestamp>.json` - 充值交易
- `refund_tx_<timestamp>.json` - 退款交易（待商户签名）

### 2. sign-tx 命令 - 交易签名
```bash
# 用户签名
spillman-cli sign-tx --tx-file secrets/commitment_tx_100_ckb.json \
    --privkey-path privkey.txt

# 商户签名
spillman-cli sign-tx --tx-file secrets/commitment_tx_100_ckb.json \
    --privkey-path privkey.txt \
    --is-merchant
```

**实现内容**：
- [x] 读取交易文件
- [x] 读取私钥
- [x] 根据角色（用户/商户）进行签名
- [x] 保存签名后的交易到新文件

**输出文件**：
- `<original_name>_signed_<timestamp>.json`

### 3. pay 命令 - 创建承诺交易
```bash
# 创建链下支付（支持小数金额）
spillman-cli pay --amount 100.5 \
    --channel-file secrets/channel_info.json \
    --config config.toml

# 创建更大金额的支付（必须递增）
spillman-cli pay --amount 200 \
    --channel-file secrets/channel_info.json \
    --config config.toml
```

**实现内容**：
- [x] 读取通道信息
- [x] 构造 commitment transaction
  - Input: Spillman Lock cell
  - Output 0: 用户地址（找零）
  - Output 1: 商户地址（支付金额）
- [x] 用户签名
- [x] 验证金额和容量限制
- [x] 保存到独立文件

**输出文件**：
- `commitment_tx_<amount>_<timestamp>.json`

### 4. settle 命令 - 商户结算
```bash
# 签名并广播 commitment transaction
spillman-cli settle --tx-file secrets/commitment_tx_100_ckb.json \
    --config config.toml \
    --broadcast

# 仅签名，不广播
spillman-cli settle --tx-file secrets/commitment_tx_100_ckb.json \
    --config config.toml
```

**实现内容**：
- [x] 读取 commitment transaction
- [x] 商户补充签名
- [x] 广播交易到链上（可选）
- [x] 显示交易哈希

**输出**：
- 交易哈希（在终端显示）

### 5. refund 命令 - 用户退款
```bash
# 使用 v1 实现构建退款交易
spillman-cli refund --tx-file secrets/funding_tx_signed.json \
    --config config.toml

# 使用 v2 实现构建退款交易（推荐）
spillman-cli refund --tx-file secrets/funding_tx_signed.json \
    --config config.toml \
    --use-v2
```

**实现内容**：
- [x] 读取 funding transaction
- [x] 构建 refund transaction
- [x] 支持商户预签名（setup 阶段）
- [x] 支持用户超时后签名
- [x] 支持单方和 co-fund 模式
- [x] 支持 refund_v2 新实现（TxBuilder 模式）

**输出**：
- 交易哈希（在终端显示）

## 🏗️ 技术架构

### 项目结构
```
examples/
├── Cargo.toml
├── src/
│   ├── main.rs              # CLI 入口
│   ├── commands/            # 命令实现
│   │   ├── mod.rs
│   │   ├── setup.rs         # set-up 命令
│   │   ├── sign.rs          # sign-tx 命令
│   │   ├── pay.rs           # pay 命令
│   │   ├── settle.rs        # settle 命令
│   │   └── refund.rs        # refund 命令
│   ├── tx_builder/          # 交易构造
│   │   ├── mod.rs
│   │   ├── funding.rs       # Funding transaction
│   │   ├── refund.rs        # Refund transaction
│   │   └── commitment.rs    # Commitment transaction
│   ├── signer/              # 签名相关
│   │   ├── mod.rs
│   │   └── spillman_signer.rs
│   ├── storage/             # 文件存储
│   │   ├── mod.rs
│   │   └── tx_storage.rs
│   └── utils/               # 工具函数
│       ├── mod.rs
│       ├── config.rs        # 配置读取
│       └── rpc.rs           # RPC 客户端
└── secrets/                 # 密钥和交易文件存储
    └── note.md
```

### 依赖库
```toml
[dependencies]
ckb-sdk = "4.4.0"              # CKB SDK
ckb-types = "0.203.0"          # CKB 类型
ckb-jsonrpc-types = "0.203.0"  # RPC 类型
ckb-crypto = { version = "0.203.0", features = ["secp"] }
ckb-hash = "1.0.0"
anyhow = "1.0"                 # 错误处理
clap = { version = "4.0", features = ["derive"] }  # CLI 参数解析
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
hex = "0.4"
```

## 📝 实施步骤

### 阶段 1: 基础设施搭建
- [x] 创建开发计划文档
- [x] 添加 CLI 依赖（clap）
- [x] 创建项目目录结构
- [x] 创建 main.rs 和命令解析
- [x] 创建所有命令模块框架
- [x] 实现配置文件读取
- [x] 实现 RPC 客户端封装

### 阶段 2: 交易构造模块
- [x] 实现 Spillman Lock Args 编码（50 bytes）
- [x] 实现 Witness 结构编码（147 bytes）
- [x] 实现 Funding Transaction 构造（v1 和 v2）
- [x] 实现 Refund Transaction 构造（v1 和 v2）
- [x] 实现 Commitment Transaction 构造
- [x] 实现 co-funding 模式支持

### 阶段 3: 签名模块
- [x] 实现 SIGHASH_ALL 签名
- [x] 实现用户签名逻辑
- [x] 实现商户签名逻辑
- [x] 实现签名验证

### 阶段 4: CLI 命令实现
- [x] 实现 set-up 命令（v1 和 v2）
- [x] 实现 sign-tx 命令
- [x] 实现 pay 命令
- [x] 实现 settle 命令
- [x] 实现 refund 命令（v1 和 v2）

### 阶段 5: 集成测试
- [x] 测试用户单独出资流程（testnet 验证）
- [x] 测试用户和商户共同出资流程（co-fund 模式）
- [x] 测试链下支付流程（commitment transaction）
- [x] 测试商户正常结算（settle transaction）
- [x] 测试用户超时退款（refund transaction）
- [x] 更新文档（部分完成）

## 🔧 关键技术点

### 1. Spillman Lock Args（50 bytes）
```rust
struct SpillmanLockArgs {
    merchant_lock_arg: [u8; 20],     // Merchant lock script args (Blake2b-160)
    user_pubkey_hash: [u8; 20],      // User pubkey hash (Blake2b-160)
    timeout_timestamp: u64,            // Unix timestamp (little-endian)
    algorithm_id: u8,                 // 0 = single-sig, 6 = multi-sig
    version: u8,                      // 版本号（当前为 0）
}
```

**布局**：
- `merchant_lock_arg`: 20 bytes - 商户锁脚本参数
- `user_pubkey_hash`: 20 bytes - 用户公钥哈希（Blake2b-160）
- `timeout_timestamp`: 8 bytes - 超时时间戳（Unix timestamp，小端序）
- `algorithm_id`: 1 byte - 算法 ID（0 = 单签，6 = 多签）
- `version`: 1 byte - 版本号（当前为 0）

**总计**: 50 bytes

### 2. Witness 结构（147 bytes，单签模式）
```rust
enum UnlockType {
    Commitment = 0x00,  // Commitment 路径 - 需要双方签名
    Timeout = 0x01,     // Timeout 路径 - 超时后仅需用户签名
}

struct SpillmanWitness {
    empty_witness_args: [u8; 16],  // WitnessArgs placeholder: [16,0,0,0, 16,0,0,0, 16,0,0,0, 16,0,0,0]
    unlock_type: UnlockType,       // 解锁类型（1 byte）
    merchant_signature: [u8; 65],  // ECDSA 签名（必需）
    user_signature: [u8; 65],      // ECDSA 签名（必需）
}
```

**布局**（单签模式，algorithm_id=0）：
- `empty_witness_args`: 16 bytes - WitnessArgs 占位符，固定值为 `[16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0, 16, 0, 0, 0]`
- `unlock_type`: 1 byte - 解锁类型
  - `0x00` = Commitment 路径（需要双方签名）
  - `0x01` = Timeout 路径（超时后仅需用户签名）
- `merchant_signature`: 65 bytes - 商户 ECDSA 签名
- `user_signature`: 65 bytes - 用户 ECDSA 签名（必需）

**总计**: 16 + 1 + 65 + 65 = 147 bytes

**注意**：多签模式（algorithm_id=6）的 witness 结构不同，包含多签配置和多个签名。

### 3. 交易签名
- 使用 SIGHASH_ALL 模式
- 双方签名顺序：
  - Commitment: 用户先签，商户后签
  - Refund: 商户先签，用户后签

### 4. 输出结构验证
- Commitment: 必须 2 个输出（用户找零 + 商户支付）
- Refund: 必须 1 个输出（用户全额退款）

## 📊 进度追踪

| 阶段 | 状态 | 完成时间 |
|------|------|----------|
| 基础设施搭建 | ✅ 已完成 | 2025-10-31 |
| 交易构造模块 | ✅ 已完成 | 2025-12-XX |
| 签名模块 | ✅ 已完成 | 2025-10-31 |
| CLI 命令实现 | ✅ 已完成 | 2025-12-XX |
| 集成测试 | ✅ 已完成 | 2025-12-XX |

## 🔗 参考资料

- [Spillman Lock 设计文档](../docs/spillman-lock-design.md)
- [需求文档](./secrets/note.md)
- [实现状态](./IMPLEMENTATION_STATUS.md)
- [快速开始](./QUICKSTART.md)
- [CKB SDK 文档](https://github.com/nervosnetwork/ckb-sdk-rust)

## 📝 开发日志

### 2025-10-31
- ✅ 实现 Spillman Witness 结构编码（147 bytes）
  - ✅ UnlockType 枚举（Commitment/Timeout）
  - ✅ SpillmanWitness 结构体
  - ✅ 序列化/反序列化方法
- ✅ 实现签名模块 (src/signer/)
  - ✅ SpillmanSigner 签名器
  - ✅ SIGHASH_ALL 消息计算
  - ✅ 用户和商户签名逻辑
  - ✅ Commitment 交易签名流程
  - ✅ Refund 交易签名流程
- ✅ 创建交易构造模块框架
  - ✅ Funding transaction 模板
  - ✅ Refund transaction 模板
  - ✅ Commitment transaction 模板
- ✅ 修复编译错误，代码成功编译

### 2025-10-30
- ✅ 创建开发计划文档
- ✅ 添加 clap 依赖到 Cargo.toml
- ✅ 创建 CLI 主入口文件 (src/main.rs)
- ✅ 实现命令行参数解析（5个子命令）
- ✅ 创建命令模块框架 (src/commands/)
- ✅ 测试 CLI 编译和运行
- ✅ 实现 set-up 命令基础功能
  - ✅ 创建 utils 模块（config, crypto, rpc）
  - ✅ 创建 tx_builder 模块
  - ✅ 实现 Spillman Lock script 构建
  - ✅ 实现配置加载和密钥解析
  - ✅ 实现 RPC 连接和 epoch 查询
  - ✅ 保存通道信息到 JSON 文件

---

## 🎉 完成里程碑

### 2025-12-XX - 核心功能完成
- ✅ 所有 CLI 命令已实现并通过 testnet 验证
- ✅ 支持纯 CKB 通道的完整生命周期
  - Funding transaction（创建通道）
  - Commitment transaction（链下支付）
  - Settle transaction（商户结算）
  - Refund transaction（用户退款）
- ✅ 支持单方出资和 co-fund 模式
- ✅ 实现 funding_v2 和 refund_v2（TxBuilder 模式）
- ✅ Testnet 验证完成
  - Funding: [testnet.explorer.nervos.org/transaction/0xff94e467436a38dae41a1783722537c7a8de28354c6a79901d4eb0b01170e8aa](https://testnet.explorer.nervos.org/transaction/0xff94e467436a38dae41a1783722537c7a8de28354c6a79901d4eb0b01170e8aa#0)
  - Settle: [testnet.explorer.nervos.org/transaction/0xe00393ed82cee81eb1148dce3acf38e5f3501fa8816680c962cf364974fca615](https://testnet.explorer.nervos.org/transaction/0xe00393ed82cee81eb1148dce3acf38e5f3501fa8816680c962cf364974fca615)
  - Refund: [testnet.explorer.nervos.org/transaction/0xa111660ae76f27e09905935231a711b134c584197e1b1e9f67fd6464586b4360](https://testnet.explorer.nervos.org/transaction/0xa111660ae76f27e09905935231a711b134c584197e1b1e9f67fd6464586b4360)

## 📝 开发日志

### 2025-12-XX - 核心功能完成
- ✅ 完成所有 CLI 命令实现
  - ✅ `set-up` 命令（支持 v1/v2，单方/co-fund）
  - ✅ `pay` 命令（创建 commitment transaction）
  - ✅ `settle` 命令（商户结算）
  - ✅ `refund` 命令（支持 v1/v2）
  - ✅ `sign-tx` 命令（通用签名工具）
- ✅ 实现 funding_v2 模块（TxBuilder 模式）
  - ✅ 使用 CapacityBalancer 自动计算手续费
  - ✅ 支持 HumanCapacity 格式输入
  - ✅ 支持增量构造（co-funding）
- ✅ 实现 refund_v2 模块（TxBuilder 模式）
  - ✅ 与 funding_v2 保持一致的设计模式
  - ✅ 支持单方和 co-fund 退款
  - ✅ 迭代手续费计算
- ✅ Testnet 完整流程验证
  - ✅ 纯 CKB 通道测试通过
  - ✅ 所有交易类型验证成功

### 2025-10-31
- ✅ 实现 Spillman Witness 结构编码（147 bytes）
  - ✅ UnlockType 枚举（Commitment/Timeout）
  - ✅ SpillmanWitness 结构体
  - ✅ 序列化/反序列化方法
- ✅ 实现签名模块 (src/signer/)
  - ✅ SpillmanSigner 签名器
  - ✅ SIGHASH_ALL 消息计算
  - ✅ 用户和商户签名逻辑
  - ✅ Commitment 交易签名流程
  - ✅ Refund 交易签名流程

---

## 🚀 下一步计划


### 未来功能
- [x] 支持商户多签地址签名
  - [x] funding (co-funding 模式)
  - [x] refund (V2 multisig 验证通过)
  - [x] settle(V2 multisig 验证通过)
  - [x] funding (Legacy multisig)
  - [x] refund (Legacy multisig)
  - [x] settle(Legacy multisig)
- [ ] **xUDT 通道支持**：支持用户自定义代币（xUDT）的支付通道
  - 需要扩展 Spillman Lock 合约支持 xUDT
  - 需要更新交易构造逻辑处理 xUDT cells
  - 需要更新 commitment 和 refund 逻辑支持 xUDT 转账

---

**最后更新**: 2025-12-XX
**当前状态**: ✅ 核心功能已完成（纯 CKB 通道）
**下一步**: 完善文档，准备 xUDT 通道支持
