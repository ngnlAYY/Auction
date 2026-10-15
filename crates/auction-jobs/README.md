# auction-jobs

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

持久化任务的调度、重试、游标推进与本机 Worker control；不复制拍卖状态机。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [源码布局](#源码布局)
- [生命周期、慢速代理与结算](#生命周期慢速代理与结算)
- [Worker control](#worker-control)
- [边界约束](#边界约束)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/worker/runtime](src/worker/runtime) | 循环组合、端口装配、任务所有权与退出 |
| [src/worker/auction](src/worker/auction) | 生命周期与慢速代理调度 |
| [src/worker/settlement](src/worker/settlement) | 结算/履约编排 |
| [src/worker/recovery](src/worker/recovery) | 恢复阶段与证据收口 |
| [src/worker/mail](src/worker/mail) | 轮询、续租、重试与完成前缀 |
| [src/worker/forum](src/worker/forum) | 主题/固定帖同步与命令导入 |
| [src/worker/realtime](src/worker/realtime)、[operations](src/worker/operations) | 实时消费与运维告警 |
| [src/control](src/control) | Unix control 协议、peer 认证与客户端 |

## 职责与实现边界

Worker 决定何时调用应用命令、如何按持久化状态重试；领域/应用层拥有业务决定，存储层拥有 SQL 与锁。

## 源码布局

`lib.rs` 只负责导出，`worker/` 按业务 owner 拆分；`runtime/model.rs` 持有端口/参数，
`runtime/runner.rs` 组合循环、drain 与任务退出。control 协议、handler、Unix server 和 Admin client
独立放在 `control/transport/`，调度器通过 `WorkerControlHandle` 接入状态与唤醒。

## 生命周期、慢速代理与结算

| 循环 | 编排要求 |
| --- | --- |
| Auction | 生命周期、slow due、结算依次执行，各阶段失败不跳过后续阶段 |
| Fulfillment | 按可用并发槽领取，持久化 prepared/enqueued 与 delivery 关联，不提前占满租约 |
| Recovery | 保持 case → return → action → relist → refund 顺序，各阶段独立收口 |
| Delivery | claim 后先查 transport 可用性，再进入 sending；续租、最终写回和未知结果均受 fencing 保护 |
| Mail | main/isekai 独立 mailbox、cron、唤醒器、游标与租约；按旧到新处理 |
| Forum | 独立顺序循环，先发布再读回帖，发布失败不跳过导入；逐主题领取内容租约 |
| Operations alert | scan、dispatch、reconcile 各自 cadence、预算与失败边界 |

`run_persistent_until_shutdown` 消费持久化轮次；领取后冻结 owner、精确租约与 state_version。
周拍卖只在配置 allowlist、runtime 开关和数据库 enabled world 的交集中新建；已有轮次按冻结事实执行，
配置变更不以旧命令 ID 重发新负载。结算使用持久化 claim 的 due_at，不以实际领取延迟改变业务时间。
慢速代理扫描候选后交竞价事务重新校验，过期 generation 安全跳过。

Delivery 有效并发取配置与 transport 的较小值，MoogleMail 当前上限为 1。
发送前无凭据只安全释放 claim 并在本轮排除该世界；进入 sending 后依赖失败或超时按未知结果处理，不盲目重寄。
Send、Return、Take 的证明由各 owner 校验，成功不取决于 HTTP 成功页，见[邮件诊断](../../docs/operations/mail-diagnostics.md)。

Mail 仅对可重试错误与超时做有界 inline retry，处理前先查持久化完成证明。
批次预算独立于 lease；耗尽只提交已完成连续前缀及 cursor，一封未完成则失败退避。
处理中途确定失败时，同一 fenced 提交保存成功前缀与失败退避；不采用失败页的整体 next_cursor，也不跨事务 commit 后 fail。
完成证明和已接受决定允许恢复提交前已完成的步骤，不能按取件后的资产变化重新解释旧邮件。
诊断 resolve 使用独立有界窗口，不改变业务重试分类、完成证明或 cursor。

Forum 固定 #1/#2 的 exact post ID、原始正文与本地持久化证明齐备后才导入 #3+ 普通出价。
可重试身份/存储错误停止后续楼层；真实连续处理楼层遇待重试命令即停，不能从 post ID 或正文版本推算。
导入前读取持久 pending 边界；原帖删除、命令被编辑掉或原命令未收口时，必须在执行后续竞价前停止。
未知 POST 仅精确标题只读对账，未命中后等待 CLI 持久人工再授权；代理行不进入论坛命令账本。

实时 runner 持久化连续 offsets/failures；当前广播边界是 Public 本地 Hub，未冻结外部 sink 不得伪装为已发送。
各循环默认值、预算及有效并发见[配置手册](../../ops/config/README.md#worker-循环预算)。

## Worker control

只经 UDS 提供 live、ready、status、`/metrics` 和全局邮件唤醒，不启动第二个指标监听器。
socket 位于受信 runtime 目录，持 `0600` 实例锁，只清理已证明失效且 inode 未变的旧 socket；
新 socket 固定配置 group 与 `0660`，连接须通过显式 UID/GID allowlist，并受请求/响应大小与处理预算限制。

唤醒使用有界内存 replay window，只表示已接受，让已装配 poller 执行一轮；
不能代替 `mail_poll_state` 的持久化游标、租约或业务完成证明。协议与部署参数见[Worker 入口](../../apps/worker/README.md)。

## 边界约束

所有业务状态迁移经应用命令，游标、账本与任务进度必须持久化；外部副作用在本地提交后执行。
本机 control HTTP 仅用于经 peer 认证的运维，不承载 Public/Admin 业务写入。

进入 drain 后门禁不可逆，停止新领取并收口在途 claim；每个循环及并发槽等待后都复核门禁。
某组失败仍继续收口其他组，所有任务共享同一总截止时间。信号、10 秒总上限与二次信号强制退出
由装配根和 `auction-process` 拥有，不能为后续任务重新计时。

论坛扫描区分未确认 POST 与纯读取超时；前者保持人工对账，后者可正常重试。
历史未知写入只有两个固定楼层原始正文都与目标精确一致才能解除，不能以本地版本或后来渲染的新正文代替。

## 维护与验证

```bash
cargo check --locked -p auction-jobs
```

仅静态编译；默认检查与测试范围见[开发指南](../../docs/development.md)，运行证据见[验证状态](../../docs/status.md)。
修改行为时同步对应契约，外部调用验证须由用户明确要求。
