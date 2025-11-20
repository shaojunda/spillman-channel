# Spillman Lock 合约设计

> 基于 Spillman Channel 的单向支付通道 CKB Lock Script
>
> 创建时间: 2025-10-28
> 参考: Spillman Channel (Bitcoin Wiki Example 7)

## 1. 合约概述

**Spillman Lock** 是一个用于实现单向支付通道的 CKB Lock Script，支持两种解锁路径：

1. **Commitment Path（承诺结算路径）**: 商户随时可以用用户签名的承诺交易结算
2. **Timeout Path（超时退款路径）**: 超时后用户可以全额退款

**命名由来**：致敬 Spillman Channel 设计者 Jeremy Spillman（2013）

## 2. 核心概念

### 2.1 Spillman Channel 工作流程

```
阶段 1: 通道创建
┌─────────────────────────────────────────────────────────┐
│ 1. 用户构造退款交易（Refund Transaction）              │
│    - Input: 未来的通道 cell（尚不存在）                │
│    - Output: 用户地址（全额）                           │
│    - Since: timeout_timestamp（时间锁，Unix 秒）       │
│                                                          │
│ 2. 商户为退款交易签名（保险机制）                       │
│    - 确保用户不会永久失去资金                           │
│                                                          │
│ 3. 用户广播充值交易（Funding Transaction）             │
│    - Input: 用户的 UTXO                                 │
│    - Output: Spillman Lock cell（双方公钥）            │
│    - 现在通道创建完成！                                 │
└─────────────────────────────────────────────────────────┘

阶段 2: 链下支付（高频、零手续费）
┌─────────────────────────────────────────────────────────┐
│ 用户不断创建新的承诺交易（Commitment Transactions）    │
│                                                          │
│ Commitment 1:                                            │
│   Input: Spillman Lock cell (1000 CKB)                  │
│   Output 0: 用户地址 900 CKB (找零)                     │
│   Output 1: 商户地址 100 CKB (支付)                     │
│   Witness: 用户签名 ✓                                   │
│                                                          │
│ Commitment 2:                                            │
│   Input: Spillman Lock cell (1000 CKB)                  │
│   Output 0: 用户地址 700 CKB (找零)                     │
│   Output 1: 商户地址 300 CKB (支付) ← 累计金额增加     │
│   Witness: 用户签名 ✓                                   │
│                                                          │
│ Commitment 3:                                            │
│   Input: Spillman Lock cell (1000 CKB)                  │
│   Output 0: 用户地址 500 CKB (找零)                     │
│   Output 1: 商户地址 500 CKB (支付)                     │
│   Witness: 用户签名 ✓                                   │
│                                                          │
│ 每个新承诺覆盖旧的（金额更大）                         │
│ 商户只保留最新的承诺                                    │
└─────────────────────────────────────────────────────────┘

阶段 3: 结算
┌─────────────────────────────────────────────────────────┐
│ 选项 A: 商户结算（正常情况）                            │
│   1. 商户拿出最新承诺（例如 500 CKB）                   │
│   2. 补充商户自己的签名                                 │
│   3. 广播上链                                           │
│   4. 商户获得 500 CKB，用户获得 500 CKB                │
│                                                          │
│ 选项 B: 用户退款（商户不结算）                          │
│   1. 等待 timeout_timestamp 到期                        │
│   2. 用户广播退款交易（商户已预签名）                   │
│   3. 用户获得全部 1000 CKB                              │
│   4. 商户什么都得不到（损失全部收入）                   │
└─────────────────────────────────────────────────────────┘
```

### 2.2 关键特性

| 特性 | 说明 |
|------|------|
| **单向流动** | 资金只能从用户 → 商户，不能反向 |
| **金额递增** | 每个新承诺给商户的金额 >= 之前的承诺 |
| **链下支付** | 所有承诺交易都在链下，不上链 = 不花手续费 |
| **时间锁保护** | 退款交易的 Since 保护用户，商户必须在超时前结算 |
| **极简状态** | 只保留最新承诺，无需撤销机制（与 Lightning Network 不同） |

## 3. Args 结构（50 bytes）

```rust
/// Lock Script Args 编码
struct SpillmanLockArgs {
    merchant_lock_arg: [u8; 20],     // 0..20:  商户锁地址参数 (Blake2b-160) - 查询前缀
    user_pubkey_hash: [u8; 20],      // 20..40: 用户公钥哈希 (Blake2b-160)
    timeout_timestamp: [u8; 8],      // 40..48: 超时时间戳 (u64 小端序, Unix timestamp 秒)
    algorithm_id: u8,                // 48:     商户签名算法 ID
    version: u8,                     // 49:     合约版本号
}
// 总长度: 50 bytes
```

**字段说明**：

- `merchant_lock_arg`: 商户锁地址参数（20 bytes，Blake2b-160）**放在最前面以支持前缀查询**
  - **单签（algorithm_id=0）**：`blake160(merchant_pubkey)`
  - **多签（algorithm_id=6）**：`blake160(multisig_config)`，其中 multisig_config 格式为 `S | R | M | N | PubKeyHash1 | PubKeyHash2 | ...`
- `user_pubkey_hash`: 用户的公钥哈希（20 bytes，Blake2b-160）
- `timeout_timestamp`: 超时时间戳，**u64 小端序**，Unix timestamp 秒格式，使用 CKB 的 Since 字段传递
- `algorithm_id`: 商户签名算法 ID
  - `0`: CKB 单签（secp256k1_blake160_sighash_all）
  - `6`: CKB Legacy 多签（secp256k1_blake160_multisig_all）
  - `7`: CKB V2 多签（secp256k1_blake160_multisig_all）
- `version`: 合约版本号，当前为 0，方便未来升级

**字段顺序设计考虑**：

将 `merchant_lock_arg` 放在最前面的原因：

1. **商户需要频繁查询**：商户需要管理成百上千个通道，可能需要高效查询所有通道
2. **CKB Indexer 前缀查询**：`get_cells` API 只支持 args 的**前缀匹配**（prefix），无法匹配中间字段
3. **用户查询替代方案**：
   - 用户通常只有少数通道，可以在本地钱包数据库记录
   - 用户可以通过商户 API 查询自己的通道
   - 用户可以通过扫描商户前缀来过滤自己的通道

**字节序说明**：
- `timeout_timestamp` 使用**小端序** (little-endian)，与 CKB Transaction 的 Since 字段保持一致
- 时间戳格式为 Unix timestamp 秒级（seconds since Unix epoch）

**多签配置格式（multisig_config）**：

当 `algorithm_id=6` 时，表示商户使用多签地址，multisig_config 的格式为：

```
S (1 byte)  | R (1 byte) | M (1 byte) | N (1 byte) | PubKeyHash1 (20 bytes) | PubKeyHash2 (20 bytes) | ...
```

- **S**: 格式版本，当前为 0
- **R**: require_first_n，指定前 R 个签名必须包含（0 表示任意 M 个）
- **M**: threshold，需要的签名数量
- **N**: pubkey_cnt，总公钥数量
- **PubKeyHashX**: 每个公钥的 blake160 哈希（20 bytes）

总长度：`4 + N * 20` 字节

示例：2-of-3 多签（任意 2 个签名）
```
[0x00, 0x00, 0x02, 0x03, hash1..., hash2..., hash3...]
```

## 4. Witness 结构

### 4.1 Commitment Path（承诺结算）

商户用最新的承诺交易结算，需要用户和商户的双签名：

#### 商户单签（algorithm_id=0）

```rust
struct CommitmentWitnessSingleSig {
    empty_witness_args: [u8; 16],  // WitnessArgs placeholder
    unlock_type: u8,               // 0x00 = Commitment Path
    merchant_signature: [u8; 65],  // 商户的 CKB 签名（商户上链时补充）
    user_signature: [u8; 65],      // 用户的 CKB 签名（已在承诺交易中提供）
}
```

**总长度**: 16 + 1 + 65 + 65 = **147 bytes**

#### 商户多签（algorithm_id=6）

```rust
struct CommitmentWitnessMultiSig {
    empty_witness_args: [u8; 16],      // WitnessArgs placeholder
    unlock_type: u8,                   // 0x00 = Commitment Path
    multisig_config: [u8; 4+N*20],     // 多签配置：S|R|M|N|Hash1|Hash2|...|HashN
    merchant_signatures: [u8; M*65],   // M 个商户签名（商户上链时补充）
    user_signature: [u8; 65],          // 用户的 CKB 签名（已在承诺交易中提供）
}
```

**总长度**: 16 + 1 + (4+N\*20) + M\*65 + 65 bytes

**示例（2-of-3 多签）**:
- empty_witness_args: 16 bytes
- unlock_type: 1 byte
- multisig_config: 4 + 3\*20 = 64 bytes
- merchant_signatures: 2\*65 = 130 bytes
- user_signature: 65 bytes
- **总计**: 276 bytes

**签名顺序**：
1. 用户先签名（链下创建承诺交易时）
2. 商户后签名（上链结算时，单签提供 1 个签名，多签提供 M 个签名）

### 4.2 Timeout Path（超时退款）

用户在超时后全额退款，也需要双签名（商户在创建时预签名）：

#### 商户单签（algorithm_id=0）

```rust
struct TimeoutWitnessSingleSig {
    empty_witness_args: [u8; 16],  // WitnessArgs placeholder
    unlock_type: u8,               // 0x01 = Timeout Path
    merchant_signature: [u8; 65],  // 商户的 CKB 签名（创建时预签名）
    user_signature: [u8; 65],      // 用户的 CKB 签名（超时后补充）
}
```

**总长度**: 16 + 1 + 65 + 65 = **147 bytes**

#### 商户多签（algorithm_id=6）

```rust
struct TimeoutWitnessMultiSig {
    empty_witness_args: [u8; 16],      // WitnessArgs placeholder
    unlock_type: u8,                   // 0x01 = Timeout Path
    multisig_config: [u8; 4+N*20],     // 多签配置：S|R|M|N|Hash1|Hash2|...|HashN
    merchant_signatures: [u8; M*65],   // M 个商户签名（创建时预签名）
    user_signature: [u8; 65],          // 用户的 CKB 签名（超时后补充）
}
```

**总长度**: 16 + 1 + (4+N\*20) + M\*65 + 65 bytes

**签名顺序**：
1. 商户先签名（通道创建前预签名退款交易）
2. 用户后签名（超时后补充）

### 4.3 统一的输出结构

**核心约束**：Output 0 必须是用户地址

这个设计可以统一两种场景，并提供强安全保证：

| 场景 | Output 0 | Output 1 | 输出数量 |
|------|---------|---------|---------|
| **Commitment Path** | 用户地址（找零） | 商户地址（支付金额） | **必须恰好 2 个** |
| **Timeout Path** | 用户地址（全额退款） | ❌ 无 | **必须恰好 1 个** |

**手续费处理**：
- 通道创建时预留手续费：例如充值 1001 CKB（实际可用容量 + 预留手续费）
- 承诺/退款交易：
  - Input: 1001 CKB (Spillman Lock cell)
  - Outputs 总和: 例如 1000 CKB
  - 手续费 = Input - Outputs (例如 1 CKB)
- SIGHASH_ALL 锁定交易结构，不能后续添加输出

**设计优势**：

1. ✅ **统一验证逻辑**：合约只需验证 "Output 0 是用户地址"
2. ✅ **兼容退款**：全额退款只需一个输出
3. ✅ **防止作弊**：强制约束，不能随意构造输出到其他地址
4. ✅ **用户保护最大化**：用户的资金（找零或退款）总是在 Output 0

**为什么这样设计？**
避免第三方作恶，因为任何人都可以使用这个合约，用户签名的时候可能是不确认交易结构的，又由于所有 commitment 交易都是链下完成，如果不做限制中间服务商可以伪造交易，让用户签名从而窃取用户资产。固定位置之后即便链下交易出现问题，也可以将范围固定在用户和商户之间。

**合约强制验证**：
```rust
// ✅ 合约验证 Output 0 是用户地址
// 这样即使双方协商，也必须遵守规则：
// - Commitment: 用户拿找零（Output 0）
// - Timeout: 用户拿全额（Output 0）
```

### 4.4 为什么都是双签名？

这是 Spillman Lock 在 CKB 上的自定义 2-of-2 签名实现：

```
Spillman Lock = 2-of-2 多签 Lock Script
  ↓
任何解锁都需要用户 + 商户双签名
  ↓
承诺交易：用户先签，商户后签（商户可随时结算）
退款交易：商户先签，用户后签（用户在超时后退款）
  ↓
强制约束：Output 0 必须是用户地址
```

**对比 Bitcoin Spillman Channel**：
- Bitcoin：资金锁定在 2-of-2 多签地址（OP_CHECKMULTISIG）
- CKB：资金锁定在 Spillman Lock（实现 2-of-2 多签 + 输出约束）
- 机制相同，但 CKB 版本增加了输出结构验证

## 5. 解锁逻辑

### 5.1 Commitment Path（承诺结算路径）

**谁可以解锁**：商户（拿着用户签名的承诺交易）

**何时可以解锁**：任何时间（超时前）

**需要什么**：
1. 用户的签名（已在承诺交易中提供）
2. 商户的签名（商户上链时补充）

**验证内容**：
1. ✅ 用户签名有效（验证用户确实授权了这笔交易）
2. ✅ 商户签名有效（验证商户确实要结算）
3. ✅ 输出结构验证：
   - 必须恰好 2 个输出
   - Output 0: 用户地址（找零）
   - Output 1: 商户地址（支付金额）

**为什么需要验证输出结构？**

虽然 SIGHASH_ALL 签名已经锁定了交易结构（双方都同意），但合约仍需验证输出结构的原因：

| 验证项 | 是否需要合约验证？ | 原因 |
|-------|-----------------|------|
| 容量守恒 | ❌ 不需要 | CKB 底层已保证 `sum(inputs) >= sum(outputs)` |
| 输出金额 | ❌ 不需要 | SIGHASH_ALL 锁定了所有金额 |
| **Output 0 地址** | ✅ **需要** | 防止链下共识绕过合约约束（见下文） |
| **Output 1 地址** | ✅ **需要** | 保持 Spillman Channel 的语义清晰 |
| 用户签名 | ✅ **需要** | 验证用户授权了这笔交易 |
| 商户签名 | ✅ **需要** | 验证商户确实要结算 |

**为什么选择 SIGHASH_ALL 模式？**

与 Submarine Lock 的 OTX 模式不同：

| 考虑因素 | OTX 模式 | SIGHASH_ALL 模式 | Spillman Lock 选择 |
|---------|---------|-----------------|-------------------|
| 手续费灵活性 | ✅ 第三方可添加 | ❌ 必须预先确定 | ❌ 必须预先确定 |
| 交易结构保证 | ⚠️ 只保护核心输出 | ✅ 完全锁定 | ✅ 完全锁定 |
| 双方协作 | 单方签名 | 双方签名 | ✅ 双方签名 |
| 输出结构验证 | 必需（OTX 模式） | 预先确定 | ✅ 双重保护 |

**原因**：
1. **承诺交易是双方协作的结果**：SIGHASH_ALL 保证双方都同意
2. **输出结构验证**：防止第三方作恶，防止双方链下协商绕过通道语义
3. **双重保护**：签名 + 合约验证 = 更强的安全保证

### 5.2 Timeout Path（超时退款路径）

**谁可以解锁**：用户

**何时可以解锁**：超时后（current_timestamp >= timeout_timestamp）

**需要什么**：
1. 用户的签名
2. 商户的签名（创建时预签名）
3. 当前时间戳 >= timeout_timestamp

**验证内容**：
1. ✅ 超时验证（确保在超时后）
2. ✅ 用户签名有效
3. ✅ 商户签名有效（预签名）
4. ✅ **退款交易结构正确**（Output 0 是用户地址）← 关键安全检查！


### 5.3 为什么需要严格的输出结构验证？

这是**关键的安全设计**，防止作弊并保持通道语义：

#### Commitment Path 验证（双输出强制约束）

```
验证内容:
✅ 必须恰好 2 个输出
✅ Output 0 必须是用户地址（找零）
✅ Output 1 必须是商户地址（支付金额）

为什么必须恰好 2 个输出？

原因 1：交易结构固定
- 承诺交易的语义明确：Output 0 给用户，Output 1 给商户
- 手续费在通道创建时预留（例如 1001 CKB = 1000 容量 + 1 手续费）
- SIGHASH_ALL 签名锁定交易结构，不能后续添加输出

原因 2：防止不必要的复杂性
- 不允许额外输出，保持交易结构简洁
- 降低验证复杂度，减少出错风险

为什么需要验证？

攻击场景 1（如果不验证 Output 0）:
- 双方协商构造：Output 0 = 商户地址, Output 1 = 商户地址
- 结果：用户的找零也给了商户 ❌

攻击场景 2（如果不验证 Output 1）:
- 双方协商构造：Output 0 = 用户, Output 1 = 第三方
- 结果：支付给第三方，违背通道语义 ❌

防御措施:
✅ 强制约束输出结构
✅ 保持 Spillman Channel 的清晰语义
✅ 防止链下共识绕过合约约束
```

#### Timeout Path 验证（单输出强制约束）

```
验证内容:
✅ 必须恰好 1 个输出
✅ Output 0 必须是用户地址（全额退款）

为什么必须恰好 1 个输出？

原因 1：SIGHASH_ALL 签名机制
- 商户预签名时使用 SIGHASH_ALL，锁定了整个交易结构
- 签名包含所有 inputs 和所有 outputs
- 后续无法添加任何 input 或 output

原因 2：手续费已在通道创建时预留
- 充值时：1001 CKB (1000 通道容量 + 1 手续费)
- 退款时：Output 0 = 1000 CKB，手续费 = 1 CKB
- 不需要额外的找零输出

为什么需要验证？

攻击场景（如果不验证）:
1. 恶意商户在创建时预签名假退款交易
   退款交易（假）:
     Output 0: 商户地址 (全额！)  ← 假装退款，实际给商户
     商户签名: ✓

2. 用户超时后尝试退款，补充用户签名
3. 如果合约只验证双签名，交易通过！
4. 结果：用户以为能退款，实际钱全给了商户 ❌

防御措施:
✅ 强制 Output 0 是用户地址
✅ 即使商户预签了假退款交易，合约会拒绝
✅ 用户的退款权利得到保证
```

#### 对比两种路径

| 路径 | 验证内容 | 输出数量 | Output 0 | Output 1 | 原因 |
|------|---------|---------|---------|---------|------|
| **Commitment Path** | 双签名 + 输出结构 | **必须 2 个** | 用户地址（找零） | 商户地址（支付） | 保持通道语义 |
| **Timeout Path** | 双签名 + 超时 + 输出结构 | **必须 1 个** | 用户地址（全额） | ❌ 无 | 保证退款权利 |

## 6. 解锁路径对比

| 特性 | Commitment Path（承诺结算） | Timeout Path（超时退款） |
|------|---------------------------|------------------------|
| **解锁者** | 商户 | 用户 |
| **时间限制** | 超时前任何时间 | 必须超时后 |
| **签名要求** | 用户签名 + 商户签名 | 用户签名 + 商户签名（预签名） |
| **输出结构** | Output 0: 用户（找零），Output 1: 商户（支付） | Output 0: 用户（全额） |
| **商户激励** | 及时结算获得收入 | 不结算将失去所有收入 |
| **用户保护** | 可以用退款交易兜底 | 超时后可全额退款 |
| **Witness 大小** | 147 bytes | 147 bytes |

## 7. 完整使用流程

### 7.1 阶段 1：通道创建

**步骤 1：用户构造退款交易**
**步骤 2：商户为退款交易签名**
**步骤 3：用户广播充值交易**

### 7.2 阶段 2：链下支付

**用户创建承诺交易**
**商户验证承诺交易**


### 7.3 阶段 3A：商户结算（正常情况）

### 7.4 阶段 3B：用户退款（超时情况）


## 8. 安全性分析

### 8.1 用户无法作弊的场景

**场景 1：用户尝试发送金额减少的承诺**

```
旧承诺: 商户 300 CKB, 用户 700 CKB
新承诺: 商户 200 CKB, 用户 800 CKB  ← 作弊！

商户验证:
❌ verify_amount_increase() 失败
→ 拒绝接受
→ 保留旧承诺（300 CKB）
```

**场景 2：用户尝试伪造签名**

```
用户构造承诺但用假签名

合约验证:
❌ verify_signature_with_auth() 失败
→ 交易上链失败
```

**场景 3：用户尝试超额支付（超过容量）**

```
通道容量: 1000 CKB
用户已支付: 950 CKB
新承诺: 1100 CKB  ← 超过容量！

商户验证:
❌ verify_amount_increase() 失败
→ 拒绝接受
```

### 8.2 商户无法作弊的场景

**场景 1：商户尝试上链旧承诺（金额更少）**

```
旧承诺: 商户 300 CKB
新承诺: 商户 500 CKB

商户尝试上链旧承诺（只拿 300）:
✅ 技术上可行（用户已签名）
→ 但商户自己损失（少拿 200 CKB）
→ 商户不会这么做（不理性）
```

**经济激励保证**：
- 商户总是会选择金额最大的承诺（对自己最有利）
- 这是 Spillman Channel 的核心设计：靠经济激励，而非技术限制

**场景 2：商户不上链结算**

```
用户已支付: 500 CKB
商户一直不结算

用户保护:
✅ 用户有退款交易（商户已预签名）
✅ 时间锁到期后，用户可以全额退款
✅ 商户如果不结算，什么都拿不到
```

### 8.3 时间锁安全边界

```
建议配置:
通道有效期: 24 天 (2,073,600 秒)
安全边界: 2 天 (172,800 秒)

要求:
- 商户应在超时前至少 2 天结算
- 给商户足够时间上链（防止网络拥堵）
```

## 9. 商户多签验证流程

### 9.1 多签验证的完整数据流

当商户使用多签地址（algorithm_id=6）时，验证流程如下：

```
1. Spillman Lock Args（链上存储）:
   merchant_lock_arg(20) = blake160(multisig_config)
   user_pubkey_hash(20)
   timeout(8)
   algorithm_id(1) = 6
   version(1) = 0

2. Witness（交易时提供）:
   empty_witness_args(16)
   unlock_type(1)
   multisig_config(4+N*20)      ← 完整的多签配置
   merchant_signatures(M*65)     ← M 个商户签名
   user_signature(65)            ← 用户签名

3. Spillman Lock 验证:
   a. 从 witness 提取 multisig_config
   b. 验证 blake160(multisig_config) == args 中的 merchant_lock_arg
   c. 从 witness 移除 multisig_config，留下签名
   d. 传递给 commitment/timeout path 验证函数

4. 调用 CKB Auth 合约验证商户签名:
   spawn_cell(
     algorithm_id: "06",
     signature: hex(multisig_config + merchant_signatures),  ← 完整数据
     message: hex(signing_message),                          ← 签名消息（见下文）
     lock_arg: hex(merchant_lock_arg)                        ← 20字节哈希
   )

5. Auth 合约验证:
   a. 从 signature 提取 multisig_config
   b. 验证 blake160(multisig_config) == lock_arg
   c. 解析 R, M, N 参数和公钥列表
   d. 验证 M 个签名的有效性
```

### 9.2 签名消息的生成

```rust
// 签名消息生成
fn compute_signing_message(tx: &TransactionView) -> [u8; 32] {
    // 1. 获取 raw transaction（不包含 witnesses）
    // 2. 清空 cell_deps（设为默认值）
    // 3. 对结果进行 blake2b_256 哈希
    let tx = tx
        .data()
        .raw()
        .as_builder()
        .cell_deps(Default::default())
        .build();
    blake2b_256(tx.as_slice())
}
```

**签名消息构成**：
```
message = blake2b_256(
    version +
    cell_deps (empty) +
    header_deps +
    inputs +
    outputs +
    outputs_data
)
```

**关键点**：
- 使用 **Blake2b-256** 哈希算法
- 只包含 **raw transaction** 部分（不含 witnesses）
- **cell_deps 被清空**（设为空数组）
- 包含：inputs（包括 since）、outputs、outputs_data、header_deps
- 不包含：witnesses（签名本身）、cell_deps

**为什么清空 cell_deps？**
- 签名时只关注交易的核心内容（资金流向）
- cell_deps 是辅助数据（代码、类型脚本等），不影响资产安全
- 简化签名流程，不同的 cell_deps 配置可以使用相同的签名

### 9.3 为什么 witness 需要包含完整的 multisig_config？

这是 CKB 多签的标准设计，原因如下：

**问题**：Args 只存储 20 字节哈希，Auth 合约需要知道：
- M（需要几个签名）
- N（有几个公钥）
- 公钥列表（用于验证签名）

**解决方案**：
- ✅ 发送方在 witness 中提供完整的 multisig_config
- ✅ Spillman Lock 验证 `blake160(witness中的config) == args中的hash`
- ✅ Auth 再次验证哈希匹配（双重保护）
- ✅ Auth 使用 config 中的信息验证签名

**优点**：
1. **节省链上存储**：Args 长度固定（50 bytes），不随公钥数量增长
2. **灵活性**：支持任意 M-of-N 配置（2-of-3, 3-of-5 等）
3. **安全性**：双重哈希验证防止伪造配置
4. **标准兼容**：符合 CKB secp256k1_blake160_multisig_all 的设计

### 9.4 商户输出地址验证

无论单签还是多签，Spillman Lock 都会验证输出的商户地址：

**单签（algorithm_id=0）**：
```rust
// 期望的商户输出
Script {
    code_hash: SECP256K1_CODE_HASH,
    hash_type: Type,
    args: merchant_lock_arg  // 20字节，直接来自 Spillman Lock args
}
```

**多签（algorithm_id=6）**：
```rust
// 期望的商户输出
Script {
    code_hash: SECP256K1_MULTISIG_CODE_HASH,
    hash_type: Type,
    args: merchant_lock_arg  // 20字节，blake160(multisig_config)
}
```

这确保了资金只能流向正确的商户地址。

---

**文档版本**: v1.2

**创建时间**: 2025-10-28

**最后更新**: 2025-11-04（时间锁从 Epoch 格式改为 Timestamp 格式）

**参考文档**:
- [Bitcoin Wiki Example 7 vs Spillman](./bitcoin-wiki-example7-vs-spillman.md)
- [不用绑卡，边聊边付：一个 Web3 版的支付实验](https://talk.nervos.org/t/web3/9621)
- [CKB Auth 协议](https://github.com/nervosnetwork/ckb-auth)
