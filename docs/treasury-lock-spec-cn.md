# 国库锁定脚本规范

本文档描述国库系统所使用的锁定脚本。

## 简介

使用国库锁定脚本的 Cell 称为国库 Cell。与 [cellbase](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0022-transaction-structure/0022-transaction-structure.md#exceptions) 类似，国库 Cell 只能由共识创建。国库 Cell 可通过两种方式消费：销毁方式或奖励方式。

## 脚本

锁定脚本结构如下：

```text
code_hash: <国库锁定脚本二进制文件的 code hash>
hash_type: type
args: <8 字节区块号>
```

该 8 字节区块号按 64 位无符号小端整数解析。它表示 Cell 创建时的区块号，由 CKB 节点设置。

## Witness

对应的 witness 必须采用 WitnessArgs Molecule 格式。完整的数据结构如下：

```text
WitnessArgs:
    lock: <1 字节 consuming_method>
    input_type: <>
    output_type: <>
```

单字节 `consuming_method` 可取 `0`（销毁方式）或 `1`（奖励方式）。

## 解锁流程

根据 `consuming_method`，国库锁定脚本可通过销毁或奖励两种方式之一解锁。

### 销毁方式

选择此方式时，脚本使用 `ckb_load_header`（`source = 4, header_deps`）遍历所有区块头，以获取全部区块号。脚本解析每个区块头并提取区块号，其中最大的区块号被视为“当前区块号”。交易创建者应将最新区块的区块头放入 `header_deps`。

随后，用“当前区块号”减去 `args` 中的区块号，计算出持续区块数。

该持续区块数必须大于预定义值 `burn_expiry_blocks`，此值存储在后文所述的配置 Cell 中。

为鼓励任何人销毁已过期的国库 Cell，系统包含一套激励机制。激励金额的计算方式如下：

```text
激励金额 =（持续区块数 - burn_expiry_blocks）* burn_incentive_rate + base_burn_incentive
```

配置参数 `burn_incentive_rate` 和 `base_burn_incentive` 也存储在配置 Cell 中。激励金额以 shannon 而非 CKBytes 为单位。

该设计可确保当无人销毁 Cell 时，激励金额会随时间增长，从而应对交易费高于激励金额的情况。

锁定脚本取当前 Cell 的容量并减去“激励金额”，然后查找容量恰好等于该差值的对应输出 Cell。此输出 Cell 的 `script args`、`cell data` 和 `type script` 长度必须均为零，且 `code_hash` 和 `hash_type` 都设为零，这意味着该 Cell 永远无法花费（零锁）。

剩余容量可在交易费和找零之间分配，如何分配由交易创建者负责。

### 奖励方式

奖励方式从配置 Cell 中读取 `proposal_code_hash` 和 `proposal_hash_type`。如果输入 Cell 中存在类型脚本，其以下字段均与配置匹配，则国库 Cell 解锁：

- `code_hash`
- `hash_type`

必须遵守以下规则：

1. 按容量从大到小排列输入的国库 Cell。设 Cell 总数为 N，前 N - 1 个 Cell 的容量之和必须小于提案所需的 CKBytes。这样可以防止随意加入国库 Cell。
2. 必须恰好存在一个由零锁锁定的找零 Cell，其脚本参数和 Cell 数据为空。该 Cell 的容量等于 N 个国库 Cell 的总容量减去提案所需的 CKBytes。
3. 交易费不由国库 Cell 支付，可由提案创建者提供。

国库 Cell 不支付交易费，是因为交易费会动态变化，无法预先确定，因此留给提案创建者承担。实际情况下，交易费通常很少。

每笔交易最多只能销毁一个国库 Cell。由于每个区块都会生成一个新的国库 Cell，销毁的金额很少，可以接受。

## 配置 Cell

配置 Cell 部署在链上，使用与国库锁定脚本不同的密钥锁定，以便将配置更新委托给另一个团队。其类型脚本必须遵循 Type ID 规则。国库锁定脚本通过类型脚本哈希引用配置 Cell，该哈希被硬编码在脚本中。

配置 Cell 中的数据采用 Molecule 格式编码，结构如下：

```
table ConfigCell {
    burn_expiry_blocks: Uint32,
    burn_incentive_rate: Uint64,
    base_burn_incentive: Uint64,
    proposal_code_hash: Byte32,
    proposal_hash_type: byte
}
```

`proposal_code_hash` 和 `proposal_hash_type` 用于标识提案类型脚本。
