# solution1: hidden bond recovery action for proposals below the YES threshold

On [CKB Treasury Web (testnet)](https://treasury-testnet.ckbapp.dev/), a proposal received 400 CKB in YES votes, below the 1,000 CKB threshold.

After voting closed, I opened **Proposals** as the proposal creator. The card showed **Counting opens: Expired**, but had no **Recycle proposal bond** button before recovery became available. Clicking **Check eligibility** only said the votes were below the threshold, without explaining that the bond could be recovered later or how long to wait.

By comparison, a proposal with no votes showed the remaining blocks and estimated wait, then displayed **Recycle proposal bond** once the recovery time was reached.

The [wait message only covers the no-YES-votes case](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L288-L290), while the [recovery button stays hidden until recovery is available](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L311).

For proposals below the threshold, please keep **Recycle proposal bond** visible while waiting, disabled with the remaining blocks and estimated recovery time. Enable it when recovery becomes available.

---

# solution1: 赞成票不足门槛后，网页回收入口不可见

在 [CKB Treasury Web（测试网）](https://treasury-testnet.ckbapp.dev/)，一条提案的赞成票为 400 CKB，低于 1,000 CKB 门槛。

投票结束后，使用提案创建账户进入 **发起提案**。这条提案显示 **可计票：已到期**，但在可回收时间之前没有 **回收提案抵押** 按钮。点击 **检查能否通过** 也只提示票数不足，没有说明之后可以回收或还要等多久。

作为对照，无人投票的提案会显示剩余区块和预计等待时长，达到可回收时间后出现 **回收提案抵押** 按钮。

目前[等待提示仅覆盖无赞成票的情况](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L288-L290)，[回收按钮则只在可回收后才显示](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/src/main.tsx#L311)。

建议为赞成票不足门槛的提案提供明确的回收入口：提前显示 **回收提案抵押**，等待期间禁用并显示剩余区块和预计可回收时间，满足条件后启用。

**截图 1（对照）：无人投票，已有回收等待提示**

<img width="1212" height="890" alt="无人投票，已有回收等待提示" src="https://github.com/user-attachments/assets/e1332c6e-0156-43cf-b38a-061f91aa53aa" />

**截图 2：赞成票不足门槛，回收入口不可见且缺少等待提示**

<img width="1202" height="862" alt="赞成票不足门槛，回收入口不可见且缺少等待提示" src="https://github.com/user-attachments/assets/d0d93c99-1b72-4ded-8498-940b0bd45099" />

**截图 3（对照）：无人投票，达到可回收时间后出现回收按钮**

<!-- 在此插入截图 3 -->
