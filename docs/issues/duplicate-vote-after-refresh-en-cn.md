# solution1: duplicate vote submission after refreshing the page

## Description

After I submitted a YES vote using UniSat on [CKB Treasury Web (testnet)](https://treasury-testnet.ckbapp.dev/), the page showed a success message and a transaction hash, but the vote status was slow to update. I refreshed to check the status and found that I could still vote on the same proposal. Thinking the first vote had not gone through, I submitted another YES vote. Both transactions were confirmed.

Both votes belong to the same account and proposal, with the same direction and weight of **40,000 CKB**. The second transaction spent the first transaction's change output and created another Vote Cell, leaving the first Vote Cell unconsumed.

## Impact

The duplicate occupies another **137 CKB** of Vote Cell capacity; the two cells occupy **274 CKB** in total.

*The 40,000 CKB is the DAO-backed voting weight. These two equal YES votes from the same account can contribute at most 40,000 CKB, not 80,000 CKB, to the tally.*

After voting closes, these funds [cannot be recovered through the web UI](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L304-L306); users must [construct and submit a withdrawal transaction using the SDK/CLI](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/sdk/src/vote.ts#L217-L242).

## Transactions

- [First YES vote — block 22,955,920](https://testnet.explorer.nervos.org/transaction/0xc39a73457fbbd0c97d6512ce24509a6eebe528764719c25697d989fa4d376704)
- [Second YES vote — block 22,955,925](https://testnet.explorer.nervos.org/transaction/0xd5cec955a6dcf71c8acc3cbcfc4e7c72b92806bb871b9482baf890aff7d3b3df)

## Cause analysis

- **Pending-vote state is lost on reload.** The frontend displays the transaction hash before [waiting for confirmation and refreshing the data](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L223-L243). A full reload resets the [in-memory `busy` state](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L77-L79). The Vote action depends on [`myVote` in the current snapshot](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L263-L266); if the vote is missing, the UI [offers Vote again](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L304-L306) without checking the previously submitted transaction.
- **The SDK does not check for an existing vote.** [`castVote`](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/sdk/src/vote.ts#L68-L200) queries DAO deposits, selects funding inputs and sends the transaction without checking whether the same voter has already voted on the proposal.
- **The vote contract permits duplicates across transactions.** Its [multiple-vote check](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/contracts/vote-type-script/src/main.rs#L73-L86) limits vote outputs within one transaction, not across separate transactions.
- **On-chain counting prevents these votes from being added together.** It requires the [2-byte prefix of the voter lock hash to fall within the counting range](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/contracts/counting-type-script/src/main.rs#L166-L175), rejects [duplicate voter locks within a counting cell](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/contracts/counting-type-script/src/main.rs#L201-L207), and rejects [overlapping ranges when combining counting cells](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/contracts/proposal-type-script/src/main.rs#L423-L424).
- The [page totals](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/chain.ts#L57-L63) and [SDK's counting-cell builder](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/sdk/src/counting.ts#L193-L211) also deduplicate by voter lock, keeping the largest weight for the selected direction.

Possible trigger, inferred from the code: The first snapshot after reload may be fetched before the first vote is confirmed or indexed, leaving the Vote action available. When `castVote` later selects funding inputs, the first transaction's change may already be available. Without a duplicate check, the second submission can proceed.

## Suggested fixes

- **Primary fix — frontend, `solution1/web/src/main.tsx`:** Persist pending-vote state and transaction hashes by network, proposal ID and voter lock. Restore this state and check the transaction status after reload, blocking resubmission while confirmation is pending. Distinguish “submitted” from “confirmed”.
- **Supplementary check — SDK, `castVote` in `solution1/sdk/src/vote.ts`:** Add an optional check enabled by the web UI before constructing the transaction. Block resubmission if a live vote matches the voter, proposal, direction and weight, catching existing votes that the page has not yet displayed. This supplements the frontend's pending-state protection.

This check covers only equal-weight, same-direction repeat submissions.

---

# solution1: 投票后刷新页面仍可重复提交投票

## 问题描述

在 [CKB Treasury Web（测试网）](https://treasury-testnet.ckbapp.dev/) 使用 UniSat 提交赞成票后，页面显示了“投票成功”和交易哈希，但投票状态更新较慢。我刷新页面查看状态，发现同一提案仍可投票，以为之前没有投成功，便再次提交了赞成票，结果两笔交易均成功上链。

两笔交易属于同一账户、同一提案，均为 **40,000 CKB** 权重的赞成票。第二笔使用了第一笔的找零输出，创建了新的选票 Cell，第一笔的选票 Cell 未被消费。

## 影响

重复交易额外占用 **137 CKB** 的选票 Cell 容量，两张共占用 **274 CKB**。

*40,000 CKB 是 DAO 存款支持的票权；这两张同账户的等额赞成票最多合计计入 40,000 CKB，不能叠加为 80,000 CKB。*

投票结束后，这部分 CKB [无法通过网页取回](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L304-L306)，需自行借助 SDK/CLI [构造并发送撤回交易](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/sdk/src/vote.ts#L217-L242)。

## 交易记录

- [第一笔赞成票 — 区块 22,955,920](https://testnet.explorer.nervos.org/transaction/0xc39a73457fbbd0c97d6512ce24509a6eebe528764719c25697d989fa4d376704)
- [第二笔赞成票 — 区块 22,955,925](https://testnet.explorer.nervos.org/transaction/0xd5cec955a6dcf71c8acc3cbcfc4e7c72b92806bb871b9482baf890aff7d3b3df)

## 原因分析

- **刷新后丢失待确认投票状态。** 前端先显示交易哈希，再[等待确认并刷新数据](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L223-L243)。完整刷新会重置[内存中的 `busy` 状态](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L77-L79)。投票入口只依赖[当前快照中的 `myVote`](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L263-L266)；快照未查到选票时，页面[仍允许投票](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L304-L306)，不会核对此前提交的交易。
- **SDK 没有检查已有选票。** [`castVote`](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/sdk/src/vote.ts#L68-L200) 在查询 DAO 存款、选币和发送交易的过程中，没有检查同一账户是否已经对该提案投票。
- **投票合约允许不同交易创建重复选票。** [多选票检查](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/contracts/vote-type-script/src/main.rs#L73-L86)只限制同一笔交易内的选票输出数量，不保证不同交易之间的选票唯一。
- **链上计票不允许这两张票累加票权。** 合约要求[投票者锁哈希的 2 字节前缀属于计票范围](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/contracts/counting-type-script/src/main.rs#L166-L175)，拒绝[同一计票单元内的重复投票者锁](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/contracts/counting-type-script/src/main.rs#L201-L207)，并在合并计票单元时拒绝[重叠范围](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/contracts/proposal-type-script/src/main.rs#L423-L424)。
- [页面统计票数](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/chain.ts#L57-L63)和 [SDK 构造计票单元](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/sdk/src/counting.ts#L193-L211)时，也会按投票者锁脚本去重，同一方向只保留最大的票权。

可能的触发过程（根据代码推导）：刷新后首次查询选票时，第一笔可能尚未确认或被索引，因此仍显示投票入口。SDK 稍后选币时可能已能获取第一笔的新找零，第二次提交便在缺少重复校验的情况下继续。

## 建议修复方向

- **主要修复：前端 — `solution1/web/src/main.tsx`：** 按网络、提案 ID 和投票者锁脚本保存待确认状态及交易哈希。刷新后恢复状态并核对交易，等待确认期间禁止重复提交，明确区分“已提交”和“已确认”。
- **补充检查：SDK — `solution1/sdk/src/vote.ts` 的 `castVote`：** 增加由网页启用的可选检查，在组装交易前查询同账户、同提案、同方向、同票权的存活选票，发现后阻止重复提交。这用于补充拦截页面尚未显示的已有选票。

本检查仅覆盖等额同向重复提交。
