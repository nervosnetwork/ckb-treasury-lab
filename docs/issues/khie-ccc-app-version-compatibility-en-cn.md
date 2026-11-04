# solution1: wallet connection via Khie fails

On [CKB Treasury Web (testnet)](https://treasury-testnet.ckbapp.dev/), wallet connection via Khie fails when using [CCC App's Khie module](https://app.ckbccc.com/#khie) as the wallet provider. About one second after pairing, the session disconnects and the web UI shows `Invalid request metadata` before any connection approval appears.

Steps to reproduce (CCC App's Khie module is connected to MetaMask on CKB Testnet and is not paired with another application):

1. On CKB Treasury Web, choose **Connect wallet → CCC / Khie → Khie** and keep the dialog open.
2. Copy the full address of the **Learn more about Khie here** link in the dialog, including `role=connector`.
3. Paste it into the **Or paste pairing code** field on the right of CCC App's Khie module and click the checkmark (**Connect**). The log shows `Khie peer paired`, followed by `Khie peer unpaired`.

As a control, choose **Khie** in the wallet selector of CCC App's **Sign** module and connect to the same wallet provider. After approving **Connect**, the application can read the address and balance. Signing was not tested.

The frontend [pins `@ckb-ccc/core` to `1.21.0`](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/bun.lock). Its `get_info` request uses `params: []`, while the newer interface requires metadata. In [CCC PR #547](https://github.com/ckb-devrel/ccc/pull/547), the maintainer explicitly states that this protocol change makes old and new connectors and providers incompatible. Offline tests using published npm packages reproduced the error with a `1.21.0` client and a `1.23.0` provider; both `1.21.0 → 1.21.0` and `1.23.0 → 1.23.0` connected successfully.

Khie was [introduced into CCC in September](https://github.com/ckb-devrel/ccc/pull/503) and has since undergone a breaking protocol change. Given that the protocol is still at an early stage, I suggest hiding the Khie option for now and reconsidering support once the protocol and cross-version compatibility are more stable.

---

# solution1: 通过 Khie 连接钱包失败

在 [CKB Treasury Web（测试网）](https://treasury-testnet.ckbapp.dev/) 通过 Khie 连接钱包，使用 [CCC App 的 Khie 模块](https://app.ckbccc.com/#khie)作为钱包端。配对后约一秒断开，网页显示 `Invalid request metadata`，未进入连接确认阶段。

复现步骤（CCC App 的 Khie 模块已连接 MetaMask，网络为 CKB Testnet，且未与其他应用配对）：

1. 在网页选择 **连接钱包 → CCC / Khie → Khie**，保持弹窗打开。
2. 获取弹窗内 **Learn more about Khie here** 链接的完整地址，其中包含 `role=connector`。
3. 在 CCC App 的 Khie 模块右侧 **Or paste pairing code** 输入框填入该地址，点击右侧的确认按钮（Connect）。日志依次显示 `Khie peer paired` 和 `Khie peer unpaired`。

对照测试：在 CCC App 的 **Sign** 模块的钱包选择器中选择 **Khie**，连接同一钱包端。批准 **Connect** 请求后可正常读取地址、余额，未测试签名。

前端[锁定的 `@ckb-ccc/core` 是 `1.21.0`](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/bun.lock)，`get_info` 请求参数为 `[]`，新版接口要求携带 metadata。维护者在 [CCC PR #547](https://github.com/ckb-devrel/ccc/pull/547) 中已说明，该协议变更导致新旧连接端与钱包端不兼容。使用 npm 发布包离线测试，`1.21.0 → 1.23.0` 复现相同错误；两端同为 `1.21.0` 或 `1.23.0` 时连接成功。

Khie 于 [9 月引入 CCC](https://github.com/ckb-devrel/ccc/pull/503)，随后已有不兼容的协议变更。考虑到协议仍处于早期演进阶段，建议暂时隐藏 Khie 入口，待协议和版本兼容性更稳定后再考虑支持。
