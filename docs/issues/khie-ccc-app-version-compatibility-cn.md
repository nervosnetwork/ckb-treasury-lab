# solution1: 通过 Khie 连接钱包失败

在 [CKB Treasury Web（测试网）](https://treasury-testnet.ckbapp.dev/) 通过 Khie 连接钱包，使用 [CCC App 的 Khie 模块](https://app.ckbccc.com/#khie)作为钱包端。配对后约一秒断开，网页显示 `Invalid request metadata`，未进入连接确认阶段。

复现步骤（CCC App 的 Khie 模块已连接 MetaMask，网络为 CKB Testnet，且未与其他应用配对）：

1. 在网页选择 **连接钱包 → CCC / Khie → Khie**，保持弹窗打开。
2. 获取弹窗内 **Learn more about Khie here** 链接的完整地址，其中包含 `role=connector`。
3. 在 CCC App 的 Khie 模块右侧 **Or paste pairing code** 输入框填入该地址，点击右侧的确认按钮（Connect）。日志依次显示 `Khie peer paired` 和 `Khie peer unpaired`。

对照测试：在 CCC App 的 **Sign** 模块的钱包选择器中选择 **Khie**，连接同一钱包端。批准 **Connect** 请求后可正常读取地址、余额，未测试签名。

前端[锁定的 `@ckb-ccc/core` 是 `1.21.0`](https://github.com/nervosnetwork/ckb-treasury-lab/blob/07e177c1d2b38e41ea95200416d156d9826f57a6/solution1/web/bun.lock)，`get_info` 请求参数为 `[]`，新版接口要求携带 metadata。维护者在 [CCC PR #547](https://github.com/ckb-devrel/ccc/pull/547) 中已说明，该协议变更导致新旧连接端与钱包端不兼容。使用 npm 发布包离线测试，`1.21.0 → 1.23.0` 复现相同错误；两端同为 `1.21.0` 或 `1.23.0` 时连接成功。

Khie 于 [9 月引入 CCC](https://github.com/ckb-devrel/ccc/pull/503)，随后已有不兼容的协议变更。考虑到协议仍处于早期演进阶段，建议暂时隐藏 Khie 入口，待协议和版本兼容性更稳定后再考虑支持。
