# Bitcoin Wiki Example 7 vs Spillman Channel

> 对比分析：是同一个方案吗？

## TL;DR

**是的！完全一样。**

Bitcoin Wiki 的 **Example 7: Rapidly-adjusted (micro)payments to a pre-determined party** 就是 **Spillman Channel** 的原始描述。

Spillman Channel 这个名字来自于设计者 Jeremy Spillman，他在 2013 年提出了这个方案。

---

## Bitcoin Wiki Example 7 的描述

### 工作原理

**目标**：Alice 想向 Bob 进行高频微支付（例如按秒计费的服务）

**步骤**：

#### 1. 准备阶段（Setup）

```
Step 1: Alice 创建退款交易（Refund Transaction）

退款交易：
Input:
  - 未来的 2-of-2 多签 UTXO (待创建)
  - nLockTime: 未来某个时间（如 24 小时后）

Output:
  - Alice 的地址: 全额退款

注意：这个交易引用的 input 还不存在！
```

```
Step 2: Bob 签署退款交易

Bob 为什么要签？
- 给 Alice 保险
- 确保 Alice 不会永久失去资金
- 如果 Bob 跑路或不结算，Alice 可以在超时后取回
```

```
Step 3: Alice 广播充值交易（Funding Transaction）

充值交易：
Input:
  - Alice 的 UTXO

Output:
  - 2-of-2 多签地址 (Alice + Bob): 1 BTC

现在通道创建完成！
```

#### 2. 支付阶段（Payment）

```
Alice 不断创建新的支付承诺（Commitment Transactions）

承诺 1:
Input: 多签 UTXO (1 BTC)
Output 1: Bob 0.1 BTC
Output 2: Alice 0.9 BTC (找零)
Signature: Alice ✓

承诺 2:
Input: 多签 UTXO (1 BTC)
Output 1: Bob 0.3 BTC  ← 累计金额增加
Output 2: Alice 0.7 BTC
Signature: Alice ✓

承诺 3:
Input: 多签 UTXO (1 BTC)
Output 1: Bob 0.5 BTC
Output 2: Alice 0.5 BTC
Signature: Alice ✓

...

每个新承诺覆盖旧的（金额更大）
Bob 只保留最新的
```

#### 3. 结算阶段（Settlement）

```
选项 A: Bob 结算（正常情况）

Bob 随时可以：
1. 拿出最新的承诺（例如 0.5 BTC）
2. 补充自己的签名
3. 广播上链
4. 获得 0.5 BTC，Alice 获得 0.5 BTC
```

```
选项 B: Alice 退款（Bob 不结算）

如果 Bob 一直不结算：
1. nLockTime 到期后
2. Alice 广播退款交易
3. Alice 获得全部 1 BTC
4. Bob 什么都得不到（损失全部收入）
```

### 关键特性

**1. 单向流动**
```
资金只能从 Alice → Bob
不能反向
```

**2. 金额递增**
```
每个新承诺给 Bob 的金额 >= 之前的承诺
Bob 总是保留金额最大的（对自己最有利）
```

**3. 链下支付**
```
所有承诺交易都在链下
不上链 = 不花手续费
可以无限次支付
```

**4. 时间锁保护**
```
退款交易的 nLockTime 保护 Alice
- Bob 必须在超时前结算
- 否则 Alice 取回全部
```

---

## Spillman Channel 的描述

### 工作原理

（从之前分析的 CKB 社区文章）

**三阶段**：

1. **创建阶段**：买方和卖方协作构建带时间锁的退款交易
2. **支付阶段**：买方不断签发新的"支付承诺"，累计金额递增
3. **结算阶段**：卖方用最新承诺上链，或买方超时后退款

### 关键机制

**1. 退款交易预签名**
```
卖方先签署退款交易（保险）
买方获得安全凭证后才充值
```

**2. 累计承诺**
```
每个新承诺表示累计总额
新承诺覆盖旧承诺
```

**3. 时间锁**
```
CKB 的 since 字段（类似 Bitcoin 的 nLockTime）
```

---

## 对比分析

### 完全相同的部分

| 特性 | Bitcoin Wiki Example 7 | Spillman Channel |
|------|----------------------|------------------|
| **通道类型** | 单向 | 单向 |
| **退款保护** | nLockTime | since (时间锁) |
| **预签名** | Bob 预签退款交易 | 卖方预签退款交易 |
| **承诺方式** | 累计金额 | 累计金额 |
| **结算权** | Bob 随时可以 | 卖方随时可以 |
| **超时退款** | Alice 取回全部 | 买方取回全部 |
| **状态管理** | 极简（只保留最新） | 极简（只保留最新） |

### 实现细节的差异

| 特性 | Bitcoin Wiki Example 7 | Spillman Channel (CKB) |
|------|----------------------|------------------------|
| **区块链** | Bitcoin | CKB |
| **多签实现** | OP_CHECKMULTISIG | JavaScript Lock Script |
| **时间锁** | nLockTime (交易字段) | since (交易字段) |
| **脚本语言** | Bitcoin Script | JavaScript (CKB-VM) |

### 命名的由来

**Spillman Channel** 这个名字来自 **Jeremy Spillman**：

```
2013 年：Jeremy Spillman 在 Bitcoin 邮件列表提出
→ 第一个可行的单向支付通道设计
→ 被写入 Bitcoin Wiki 作为 Example 7
→ 后来被称为 "Spillman Channel"
```

**历史**：
- 2013: Spillman 提出
- 2015: Lightning Network 论文（基于双向通道）
- 2016+: Lightning Network 实现
- 现在: Spillman Channel 仍用于某些场景（简单）

---

## 为什么叫 "Example 7"？

Bitcoin Wiki 的 Contract 页面列举了多个智能合约例子：

```
Example 1: Providing a deposit
Example 2: Escrow and dispute mediation
Example 3: Using external state
Example 4: Micropayment channels (other approach)
Example 5: Trading across chains
Example 6: ...
Example 7: Rapidly-adjusted micropayments  ← Spillman Channel
Example 8: ...
```

**Example 7** 是第 7 个例子，重点是"快速调整的微支付"。

---

## 核心代码对比

### Bitcoin Example 7 伪代码

```python
# 1. 创建退款交易（Alice）
refund_tx = Transaction(
    inputs=[
        Input(
            prev_output=future_multisig_utxo,  # 还不存在
            sequence=0xfffffffe,  # 允许 nLockTime
        )
    ],
    outputs=[
        Output(
            value=1.0,  # 全额
            script=alice_address,
        )
    ],
    nLockTime=current_time + 24 * 3600,  # 24 小时后
)

# 2. Bob 签署退款交易
refund_tx_signed_by_bob = bob.sign(refund_tx)

# 3. Alice 广播充值交易
funding_tx = Transaction(
    inputs=[alice_utxo],
    outputs=[
        Output(
            value=1.0,
            script=multisig(alice_pubkey, bob_pubkey),  # 2-of-2
        )
    ],
)
broadcast(funding_tx)

# 4. Alice 创建支付承诺
commitment_tx = Transaction(
    inputs=[
        Input(prev_output=funding_tx.outputs[0])
    ],
    outputs=[
        Output(value=0.3, script=bob_address),    # 给 Bob
        Output(value=0.7, script=alice_address),  # 找零
    ],
)
commitment_tx_signed = alice.sign(commitment_tx)

# 5. 发送给 Bob
send_to_bob(commitment_tx_signed)

# 6. Bob 结算
commitment_tx_fully_signed = bob.sign(commitment_tx_signed)
broadcast(commitment_tx_fully_signed)
```

### Spillman Channel (CKB) 伪代码

```rust
// 1. 创建退款交易（买方）
let refund_tx = Transaction {
    inputs: vec![
        CellInput {
            previous_output: future_multisig_outpoint,  // 还不存在
            since: 0,
        }
    ],
    outputs: vec![
        CellOutput {
            capacity: 1000 * CKB,  // 全额
            lock: user_lock,
        }
    ],
    header_deps: vec![future_header],  // 时间锁依赖
};

// 2. 卖方签署退款交易
let refund_tx_signed_by_merchant = merchant.sign(&refund_tx)?;

// 3. 买方广播充值交易
let funding_tx = Transaction {
    inputs: vec![user_utxo],
    outputs: vec![
        CellOutput {
            capacity: 1000 * CKB,
            lock: multisig_lock(user_pubkey, merchant_pubkey),  // 2-of-2
            type_: None,
        }
    ],
};
broadcast(&funding_tx)?;

// 4. 买方创建支付承诺
let commitment_tx = Transaction {
    inputs: vec![
        CellInput {
            previous_output: funding_tx.outputs[0],
            since: 0,
        }
    ],
    outputs: vec![
        CellOutput {
            capacity: 300 * CKB,
            lock: merchant_lock,  // 给卖方
        },
        CellOutput {
            capacity: 700 * CKB,
            lock: user_lock,      // 找零
        },
    ],
};
let commitment_tx_signed = user.sign(&commitment_tx)?;

// 5. 发送给卖方
send_to_merchant(&commitment_tx_signed)?;

// 6. 卖方结算
let commitment_tx_fully_signed = merchant.sign(&commitment_tx_signed)?;
broadcast(&commitment_tx_fully_signed)?;
```

**对比**：几乎完全一样，只是语法和具体实现不同！

---

## 历史演进

### Spillman Channel (2013)

```
特点：
✅ 单向支付
✅ 简单
✅ 链下微支付
❌ 不能双向
❌ 用户资金被锁定
```

### Lightning Network (2015+)

```
改进：
✅ 双向支付
✅ 撤销机制（惩罚作弊）
✅ 多跳路由
❌ 更复杂
❌ 需要持续在线
```

### 现在的应用

**Spillman Channel 仍有用武之地**：
- 单向场景（只付款）
- 简单性优先
- 移动端轻量应用
- 不需要双向的情况

**Lightning Network 用于**：
- 双向支付
- P2P 转账
- 路由网络
- 完整功能

---

## 结论

### 是同一个方案吗？

**是的！完全一样！**

| 名称 | 来源 | 本质 |
|------|------|------|
| **Bitcoin Wiki Example 7** | 技术文档 | 描述了单向支付通道的工作原理 |
| **Spillman Channel** | 学术命名 | 以设计者 Jeremy Spillman 命名 |
| **Rapidly-adjusted micropayments** | 功能描述 | 快速调整的微支付 |

**三个名字，同一个东西！**

### 为什么有多个名字？

**历史原因**：
```
2013: Jeremy Spillman 提出 → 发在邮件列表
     → 被写入 Bitcoin Wiki (Example 7)
     → 社区称为 "Spillman Channel"

后来: 不同文档用不同名字
     - 学术论文: "Spillman Channel"
     - Bitcoin Wiki: "Example 7"
     - 通俗说法: "One-way payment channel"
```

### 核心机制完全相同

1. ✅ 退款交易预签名（保险）
2. ✅ 充值到 2-of-2 多签地址
3. ✅ 链下签发累计承诺
4. ✅ 接收方随时可以结算
5. ✅ 超时后发送方可以退款
6. ✅ 单向流动
7. ✅ 金额递增

### CKB 上的 Spillman Channel 是移植

**CKB 社区文章的创新点**：
- 不是发明新方案
- 而是将 Bitcoin 的 Spillman Channel 移植到 CKB
- 利用 CKB 的特性（JavaScript Lock Script）
- 实现方式更灵活

**本质相同**：
- 都是单向支付通道
- 都用时间锁保护
- 都用预签名退款
- 都是累计承诺

---

## 参考资料

### Bitcoin Wiki
- **Contract 页面**：https://en.bitcoin.it/wiki/Contract
- **Example 7**：Rapidly-adjusted (micro)payments to a pre-determined party

### 原始讨论
- **Bitcoin 邮件列表** (2013): Jeremy Spillman 的提案
- **Bitcoin-dev mailing list archives**

### Lightning Network 论文
- **Lightning Network Whitepaper** (2015):
  - 引用了 Spillman Channel
  - 作为单向通道的例子
  - 对比双向通道的改进

### CKB 实现
- **CKB 社区文章**：https://talk.nervos.org/t/web3/9621
- 将 Spillman Channel 移植到 CKB

---

## 总结

你的判断**完全正确**！

```
Bitcoin Wiki Example 7
  = Spillman Channel
  = 单向支付通道的经典设计
  = Jeremy Spillman (2013) 的提案
```

**CKB 社区文章讨论的单向通道**就是这个方案在 CKB 上的实现。

这个方案虽然简单（比 Lightning Network 简单得多），但对于**"只付款"的移动端场景**来说，**可能反而是更合适的选择**！

关键优势：
- ✅ 实现简单（几百行代码）
- ✅ 资源消耗低
- ✅ 不需要 NAT 穿透
- ✅ 可以离线

关键限制：
- ❌ 单向（但我们只需要付款）
- ❌ 资金锁定（可以通过短时间锁缓解）
- ❌ 不能路由（但可以直连 FSP）

对于你们的移动端产品，这可能是**MVP 的最佳选择**！
