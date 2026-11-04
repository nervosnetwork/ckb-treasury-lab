# solution1: JoyID 原生 CKB 模式投票失败

## 问题描述

在 [CKB Treasury Web（测试网）](https://treasury-testnet.ckbapp.dev/) 使用 JoyID 原生 CKB 的子密钥（`sub_key`）模式投票时，`send_transaction` 在验证阶段返回 `-302 TransactionFailedToVerify`，输入锁脚本报错 `21`。

JoyID 的 BTC Testnet 模式可以投票成功，但该路径使用不同的 CKB 地址和 OmniLock，不涉及 CoTA 子密钥解锁。

## 报错日志

```text
RPC error: -302
TransactionFailedToVerify: Verification failed Script(TransactionScriptError {
  source: Inputs[0].Lock,
  cause: ValidationFailure: see error code 21 on page
  https://nervosnetwork.github.io/ckb-script-error-codes/by-type-hash/d23761b364210735c19c60561d213fb3beae2fd6172743719eff6920e020baac.html#21
})
```

[JoyID 官方文档](https://docs.joyid.dev/guide/ckb/sign-raw-transaction#subkey-unlock)要求子密钥解锁时，CoTA Cell 必须位于 `cell_deps[0]`。失败交易中的 CoTA Cell 归属正确、lock 与输入一致，但第 0 项是 DAO 存款，不满足这一要求。

## 相关代码位置

- [solution1/sdk/src/vote.ts:195–200](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/sdk/src/vote.ts#L195-L200)：补齐手续费后，调用 `prioritizeCellDeps` 调整依赖顺序，再签名并发送交易。
- [solution1/sdk/src/utils.ts:310–323](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/sdk/src/utils.ts#L310-L323)：将 DAO 依赖移到最前面，CoTA 依赖随之移到 DAO 后面。
- [CCC packages/joy-id/src/ckb/index.ts:207–246](https://github.com/ckb-devrel/ccc/blob/a80ca8dee79a97b034ff430d148eb3db896690eb/packages/joy-id/src/ckb/index.ts#L207-L246)：仅在 `keyType=sub_key` 且 `witness.outputType` 为空时生成子密钥证明，并将 CoTA 依赖添加到开头。
- [CCC packages/core/src/ckb/transaction.ts:1590–1599](https://github.com/ckb-devrel/ccc/blob/a80ca8dee79a97b034ff430d148eb3db896690eb/packages/core/src/ckb/transaction.ts#L1590-L1599)：`addCellDepsAtStart` 只添加缺失的依赖，不会移动已存在的依赖，因此重新 prepare 也不能自动恢复 CoTA 的位置。

原因：补齐手续费后，SDK 再次调整依赖顺序，将 CoTA 移到 DAO 后面；后续准备步骤未恢复 CoTA 的首位位置，导致最终交易不满足 JoyID 的解锁要求。

最小修复：在 SDK 的 `castVote` 中，将 `prioritizeCellDeps` 移到 `completeFeeBy` 之前，并删除补费后的那次调用：

```typescript
prioritizeCellDeps(tx, deposits);
await tx.completeFeeBy(signer, feeRateOf(config));
const txHash = await signer.sendTransaction(tx);
```

[投票合约](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/contracts/vote-type-script/src/main.rs#L129-L170)会跳过 CoTA，因此该建议只涉及 SDK 的依赖顺序；DAO 依赖仍须位于首个 `dep_group` 之前。
