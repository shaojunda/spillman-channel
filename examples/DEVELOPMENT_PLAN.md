# Spillman Channel CLI 开发计划

> 实现单向通道 CLI 工具，支持完整的通道生命周期管理
>
> 创建时间: 2025-10-30
> 状态: 开发中

## 📋 项目目标

实现一个命令行工具来验证 Spillman Channel 的完整流程，包括：
1. 通道准备（单独出资/共同出资）
2. 链下支付
3. 结算（正常结算/超时退款）

## 🎯 功能需求

### 1. set-up 命令 - 通道准备
```bash
# 用户单独出资
spillman-cli set-up --user-address <addr>

# 用户和商户共同出资
spillman-cli set-up --user-address <addr> --merchant-address <addr>
```

**实现内容**：
- [ ] 收集用户的 cells（通过 RPC indexer）
- [ ] 构造 funding transaction（创建 Spillman Lock cell）
- [ ] 构造 refund transaction（用户先签名）
- [ ] 保存 refund_tx 到文件（等待商户签名）
- [ ] 保存 funding_tx 到文件（等待商户签名后再广播）

**输出文件**：
- `funding_tx_<timestamp>.json` - 充值交易
- `refund_tx_<timestamp>.json` - 退款交易（待商户签名）

### 2. sign-tx 命令 - 交易签名
```bash
spillman-cli sign-tx --tx-file <path> --privkey-path <path> [--is-merchant]
```

**实现内容**：
- [ ] 读取交易文件
- [ ] 读取私钥
- [ ] 根据角色（用户/商户）进行签名
- [ ] 保存签名后的交易到新文件

**输出文件**：
- `<original_name>_signed_<timestamp>.json`

### 3. pay 命令 - 创建承诺交易
```bash
spillman-cli pay --amount <amount>
```

**实现内容**：
- [ ] 读取通道信息
- [ ] 构造 commitment transaction
  - Input: Spillman Lock cell
  - Output 0: 用户地址（找零）
  - Output 1: 商户地址（支付金额）
- [ ] 用户签名
- [ ] 验证金额递增（可选）
- [ ] 保存到独立文件

**输出文件**：
- `commitment_tx_<amount>_<timestamp>.json`

### 4. settle 命令 - 商户结算
```bash
spillman-cli settle --tx-file <path> --privkey-path <path>
```

**实现内容**：
- [ ] 读取 commitment transaction
- [ ] 商户补充签名
- [ ] 广播交易到链上
- [ ] 显示交易哈希

**输出**：
- 交易哈希（在终端显示）

### 5. refund 命令 - 用户退款
```bash
spillman-cli refund --tx-file <path>
```

**实现内容**：
- [ ] 读取 refund transaction（已有商户签名）
- [ ] 验证超时条件
- [ ] 用户补充签名
- [ ] 广播交易到链上
- [ ] 显示交易哈希

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
- [x] 实现 Spillman Lock Args 编码（49 bytes）
- [x] 实现 Witness 结构编码（147 bytes）
- [ ] 实现 Funding Transaction 构造（待使用 SDK CapacityTransferBuilder）
- [ ] 实现 Refund Transaction 构造（待实现）
- [ ] 实现 Commitment Transaction 构造（待实现）

### 阶段 3: 签名模块
- [x] 实现 SIGHASH_ALL 签名
- [x] 实现用户签名逻辑
- [x] 实现商户签名逻辑
- [x] 实现签名验证

### 阶段 4: CLI 命令实现
- [ ] 实现 set-up 命令
- [ ] 实现 sign-tx 命令
- [ ] 实现 pay 命令
- [ ] 实现 settle 命令
- [ ] 实现 refund 命令

### 阶段 5: 集成测试
- [ ] 测试用户单独出资流程
- [ ] 测试用户和商户共同出资流程
- [ ] 测试链下支付流程
- [ ] 测试商户正常结算
- [ ] 测试用户超时退款
- [ ] 更新文档

## 🔧 关键技术点

### 1. Spillman Lock Args（49 bytes）
```rust
struct SpillmanLockArgs {
    merchant_pubkey_hash: [u8; 20],  // Blake2b-160
    user_pubkey_hash: [u8; 20],      // Blake2b-160
    timeout_epoch: [u8; 8],          // u64 小端序
    version: u8,                     // 版本号
}
```

### 2. Witness 结构（147 bytes）
```rust
struct SpillmanWitness {
    empty_witness_args: [u8; 16],  // WitnessArgs placeholder
    unlock_type: u8,               // 0x00 = Commitment, 0x01 = Timeout
    merchant_signature: [u8; 65],  // ECDSA 签名
    user_signature: [u8; 65],      // ECDSA 签名
}
```

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
| 交易构造模块 | 🚧 进行中 | - |
| 签名模块 | ✅ 已完成 | 2025-10-31 |
| CLI 命令实现 | 🚧 进行中 | - |
| 集成测试 | ⏳ 待开始 | - |

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
  - ⚠️ 待使用 SDK CapacityTransferBuilder 完善实现
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

**最后更新**: 2025-10-31
**当前状态**: 开发中 - 完成核心数据结构和签名模块
**下一步**: 使用 SDK 完善交易构造，实现各 CLI 命令
**预计完成**: TBD
