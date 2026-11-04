# 投票类型脚本规范

投票类型脚本与[提案类型脚本](./proposal-type-script-cn.md)配合使用。该规范中的 `vote_cell_code_hash` 和 `vote_cell_hash_type` 字段用于标识此投票类型脚本。

## 脚本

```text
code_hash: <投票类型脚本的 code hash>
hash_type: <投票类型脚本的 hash type>
args: <提案类型脚本的 20 字节 blake160 哈希>
```

`args` 字段保存本次投票所针对的提案类型脚本的 blake160 哈希。
由于提案类型脚本在整条链上是唯一的，因此不会产生歧义。

## Cell 数据

Cell 数据使用以下 Molecule 结构：

```
table Vote {
    vote: byte,
    amount: Uint64,
    dao_index: Uint16Vec,
}
```

`vote` 字段为单字节：`0` 表示“反对”，`1` 表示“赞成”。
`amount` 是所有者持有的 DAO 存款总额，以 shannon 为单位。`dao_index` 包含指向这些 DAO 存款 Cell 的 `cell_deps` 索引。

## Witness

本脚本不读取 witness。

## 解锁流程

1. 创建投票 Cell 时，该操作视为投票。脚本首先检查 `cell_deps` 中是否存在由 `args` 内所存 blake160 哈希标识的提案 Cell。不需要完整验证提案 Cell，只需确认其存在。

2. 随后，脚本在输入 Cell 中查找与对应输出 Cell 的锁定脚本相匹配的锁定脚本。该锁定脚本代表 DAO 的所有权。

3. 确保输出中恰好只有一个 Cell 包含此类型脚本。如果存在多个这样的 Cell，脚本必须失败。

4. 脚本遍历所有 `cell_deps`，查找同时满足以下全部条件的 Cell：

- 其锁定脚本与 DAO 所有者匹配。
- 其类型脚本为 [Nervos DAO](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0024-ckb-genesis-script-list/0024-ckb-genesis-script-list.md#nervos-dao)。
- 其索引出现在 Cell 数据的 `dao_index` 中。为简化实现，此处不支持 `dep_group`。`dao_index` 中应至少包含一个元素。
- 所有匹配的 DAO 存款容量之和（以 shannon 为单位）等于 Cell 数据中的 `amount`。

如果不存在这样的 `cell_dep`，脚本执行失败。

消费投票 Cell 时没有特殊含义——只是回收其占用的 CKBytes。投票 Cell 可以随时消费，无须等到投票结束。

## 设计说明

- “反对”票通常没有必要——用户可以直接不投票。不过，它可以用来撤回先前的“赞成”票：同一投票者后投的票会覆盖之前的票，因此在“赞成”后投“反对”等同于撤销先前的投票。该规则由提案类型脚本的计票逻辑执行，而非本脚本。

- 一旦提案 Cell 被消费，就无法再通过 `cell_deps` 引用它，从而阻止在提案关闭后继续投票。

- 结算时不要求投票 Cell 仍处于存活状态。投票 Cell 可以在投票后立即消费，但 DAO 存款必须在 `duration` 个区块内保持存活。

## 示例

### 示例 1：创建投票 Cell（投赞成票）

```yaml
Cell Deps:
    Proposal_Cell                                       # 本次投票所针对的提案
        Data: <提案 Cell 数据>
        Type:
            code_hash: <提案类型脚本的 code hash>
            hash_type: <提案类型脚本的 hash type>
            args:
                <20 字节 blake160 Type ID>
        Lock:
            <always-success 锁定脚本>

    DAO_Deposit_Cell                                    # 投票者的 DAO 存款，用于证明投票权重
        Data: <DAO 存款数据>
        Type:
            code_hash: <Nervos DAO code hash>           # 创世区块 DAO 类型脚本
            hash_type: 0x01                             # type
            args: <空>
        Lock:
            <投票者的锁定脚本>                           # 必须与 Vote_Cell 输出上的锁一致

Inputs:
    Funding_Cell
        Data: <空>
        Type: <无>
        Lock:
            <投票者的锁定脚本>                           # 与 DAO_Deposit_Cell 和 Vote_Cell 输出使用相同的锁

Outputs:
    Vote_Cell
        Data: <Molecule 编码的 Vote>
            vote: 0x01                                  # 1 = 赞成
            amount: <投票者的 DAO 总余额，以 shannon 为单位>
            dao_index: [1]                              # cell_deps 中索引 1（DAO_Deposit_Cell）
        Type:
            code_hash: <投票类型脚本的 code hash>
            hash_type: <投票类型脚本的 hash type>
            args:
                <提案类型脚本的 20 字节 blake160>         # 标识本次投票所针对的提案
        Lock:
            <投票者的锁定脚本>

    <任意> Change_Cell
        Data: <空>
        Type: <无>
        Lock:
            <投票者的锁定脚本>

Witnesses:
    WitnessArgs 结构（索引与 Funding_Cell 输入一致）:
        Lock: <投票者的签名>
        input_type: <无>
        output_type: <无>                              # 投票类型脚本不读取 witness
```

---

### 示例 2：消费投票 Cell（回收 CKB）

```yaml
Inputs:
    Vote_Cell                                           # 先前投出的票
        Data: <Molecule 编码的 Vote>
            vote: 0x01                                  # 1 = 赞成（原始投票内容）
            amount: <投票者的 DAO 总余额，以 shannon 为单位>
            dao_index: [1]
        Type:
            code_hash: <投票类型脚本的 code hash>
            hash_type: <投票类型脚本的 hash type>
            args:
                <提案类型脚本的 20 字节 blake160>
        Lock:
            <投票者的锁定脚本>

Outputs:
    <任意> Change_Cell
        Data: <空>
        Type: <无>
        Lock:
            <投票者的锁定脚本>                           # 投票者收回被占用的 CKBytes

Witnesses:
    WitnessArgs 结构（索引与 Vote_Cell 输入一致）:
        Lock: <投票者的签名>
        input_type: <无>
        output_type: <无>                              # 投票类型脚本不读取 witness
```
