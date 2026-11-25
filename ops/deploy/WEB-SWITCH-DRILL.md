# 页面栈冷回切演练（浏览器部分）

[ops/README.md](../README.md) · [ops/deploy/RECOVERY.md](RECOVERY.md) · [ops/deploy/web-switch-drill-receipt.template.json](web-switch-drill-receipt.template.json) · [apps/auction-cli/README.md](../../apps/auction-cli/README.md)

本页拥有冷回切演练里**必须由人在冷浏览器中完成的步骤**与人工签收清单。可自动核对的部分（逐响应的构建号、入口 HTML 摘要、脚本策略与 `ETag`/`304`/`Vary`/`Cache-Control`）由 `auction-cli web drill-verify` 按回执核对，判定条件以[apps/auction-cli/src/web/drill/validation.rs](../../apps/auction-cli/src/web/drill/validation.rs)为准，本页不重述。回执字段取值以[ops/deploy/web-switch-drill-receipt.template.json](web-switch-drill-receipt.template.json)为准。

演练在最终 tag 的发布包实物与隔离环境上进行，由维护者明确要求后执行；不连接生产数据库、游戏账号或论坛。本页不表示已经做过真实演练。

## 前置

- 已用最终 tag 的外层 Release tarball 完成页面栈安装，`<web_root>/RECEIPT.v1` 记录该实物的 `tag`、`build_id` 与 `installed_at`。演练必须在 `installed_at` 之后开始，早于它的回执会被拒绝。
- 执行人与审核人是两个人。浏览器使用全新配置文件（无缓存、无 Service Worker、无扩展），开发者工具勾选“停用缓存”**不得**开启：本演练要核对真实缓存行为。
- 旧壳 build（Dioxus）与新版 build（Solid）的 build ID 与入口 HTML 摘要已按[回执模板](web-switch-drill-receipt.template.json)的 `surfaces` 记录。

## 阶段与浏览器步骤

三个阶段依次为：`new`（指针指向新版）、`legacy`（`web switch --to old` 回切旧壳）、`new-again`（`web switch --to new` 再切新版）。每个阶段、每一端（Public 与 Admin）都依次做：

1. 冷启动浏览器，打开该端默认入口，保存响应头与正文摘要进回执的 `responses`。
2. 刷新一次取得 `304` 观测；再打开该端 `/api/` 下一个不存在的路径取得 `404` 观测。
3. 在开发者工具的网络面板核对每个清单内对象（含延迟加载的 chunk）均为 200 或 304，没有 404、没有被替换成 `index.html` 的回退响应。

回切阶段（`legacy`）另做下面三项人工核对，结果写入回执 `manual_signoff`，每项都要证据文件名（截图或网络导出，文件名不含秘密与玩家信息）：

| 签收项 | 做法 | 通过标准 |
| --- | --- | --- |
| `wasm_started` | 回切后冷打开旧壳页面 | Wasm 启动完成，页面可交互，控制台无启动错误 |
| `legacy_interaction_state_changed` | 在旧壳上操作一个由事件处理器驱动的控件（如语言或主题切换） | 控件状态变化可见且刷新后保持 |
| `legacy_to_new_entry_read_ok` | 从旧壳页面进入新版 `/consignment` 订单或权利入口 | 响应为独立入口的 Solid 页面与 Solid 脚本策略，且读取成功（无错误页） |

再次切回新版（`new-again`）后，另核新页面的晚加载与未决请求恢复：打开订单或权利页后切断再恢复网络，页面应按原样重放未决请求，不产生新命令。

## 填写与校验

1. 把回执写成 `auction-web-switch-drill.v1`，三项 `manual_signoff` 的 `result` 填 `passed` 并附证据文件名；任一项失败或未做，如实填 `not_run` 或 `failed`，`conclusion.result` 不得为 `passed`。
2. 审核人对回执文件计算 SHA-256 并带外交给校验命令：

```bash
auction-cli web drill-verify --receipt <回执文件> --receipt-sha256 <审核人给出的摘要> --web-root <web_root>
```

校验通过前，不得把本次演练作为启用寄售服务的依据。回执被改写、早于 `installed_at`、build ID 或 tag 与实物不符、缺任一人工项，都会被拒绝，须重新演练而不是修补回执。

## 回退前的数据库核对

演练只涉及页面栈指针，不改数据库。若演练结论要求回退到旧程序而数据库已跨过寄售迁移，先做[跨 breaking 迁移回退汇总核对](RECOVERY.md#寄售跨-breaking-迁移回退汇总核对)。
