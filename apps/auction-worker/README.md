# auction-worker

[文档导航](../../docs/README.md) · [调度运行时](../../crates/auction-jobs/README.md) · [结算与履约](../../docs/guide/settlement.md) · [邮件流程](../../docs/guide/mail-flows.md) · [论坛](../../docs/guide/forum.md)

后台任务进程：拍卖生命周期推进、慢速代理、排期、结算与履约、邮件投递、付款超时恢复、MoogleMail 收件轮询与邮箱维护、论坛同步与回帖、装备超价私信、运维告警，以及本机 Worker control。本 crate 负责装配：读取配置、连接数据库与外部服务、取得运行时所有权，然后把端口交给 [`auction-jobs`](../../crates/auction-jobs/README.md) 的调度运行时。

## feature

| feature | 作用 |
| --- | --- |
| `postgres` | 持久化 Worker 的全部能力 |
| `cache-redis` | 允许 Redis 缓存后端 |

没有 `postgres` 时只能以“骨架”运行（仅 control 与指标），配置了数据库会拒绝启动。

## 启动

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml auction-worker
cargo run --locked -p auction-worker --features postgres   # 本地开发；优先读取本地角色文件 ops/config/worker.toml
```

`ops/config/worker.toml` 是运行时默认查找路径，需由部署者准备；仓库模板位于 [`ops/config/examples/worker.toml`](../../ops/config/examples/worker.toml)，不要把模板中的联调配置直接当作已配置的部署环境。

装配顺序：

1. 加载 Worker 配置，初始化日志、tracing 与指标，订阅终止信号。
2. 未配置数据库时：若启用了 MoogleMail、论坛或运维告警则拒绝启动，否则以骨架模式运行。
3. 连接数据库，按 `[worker.mooglemail.send_intervals]` 配置同类邮件的发送间隔。
4. 装配生命周期命令（使用与 Admin 相同的拍卖策略与排期日历）、慢速代理、论坛、邮件轮询。
5. **外部凭据运行时所有权**：实际装载 MoogleMail 或论坛凭据时，Worker 用 PostgreSQL session advisory lock 取得对应凭据种类的唯一 owner，同一时刻只有一个 Worker 能使用该类凭据；竞争同一 owner 的第二个实例启动失败。每秒检查一次已取得的所有权，一旦失效（例如数据库连接断开）立即触发排空并以非零状态退出，由进程管理器重启后重新竞争。
6. 启用 `[worker.control]` 时绑定 control socket（见下文），发布心跳，启动全部循环。

MoogleMail 默认通过 `poll_isolation_enabled = true` 为每个世界独立运行收件与维护，维护按自己的日程先执行 `sweep`，再按默认一小时的最小间隔观察完整 Read Mail 列表。省略维护日程时跟随 Admin 托管覆盖后的收件日程；两个世界使用启动时计算的相位平移日程。参数只在 `worker.toml` 中配置，详见[配置参考](../../ops/config/README.md#mooglemail-收件与维护)。设置开关为 `false` 并重启可恢复原三分支 join 路径，重启须遵守部署时段门禁。

## 外部能力的启用条件

| 能力 | 配置 | 依赖 |
| --- | --- | --- |
| MoogleMail 收件、投递、履约 | `[worker.mooglemail] enabled = true` 与世界设置；默认关闭，启用时默认仅 main，isekai 需加入 `additional_worlds` | 凭据、`equipment_data_key` 与 `user_login_data_key`（均含对应 key ID）；数据库凭据另需 `credential_data_key` |
| 论坛同步、回帖与装备 PM | `[worker.forum] enabled = true`，默认关闭 | `equipment_data_key` 与论坛凭据；装备 PM 不依赖 MoogleMail 开关 |
| 运维告警邮件 | `[worker.operations_alert] enabled = true`，默认关闭 | 收件人、告警世界必须在已启用的 MoogleMail 世界内 |
| 邮件诊断归档 | `[worker.mooglemail.diagnostics]`（默认启用） | `credential_data_key`，否则 `equipment_data_key` 作为加密父密钥 |

诊断归档目录默认是 Worker 配置文件旁的 `mail-diagnostics/`，按保留天数（默认 7）与总字节上限（默认 256 MiB）清理；归档已加密，用 `auction-cli mail export-evidence` 离线导出。

周排期与早鸟期分别由 `[auction.schedule] enabled`、`[auction.schedule.early_bird] enabled` 控制，默认均关闭，早鸟期仅在周排期启用时生效。关闭邮件或论坛不会停用数据库中的拍卖生命周期、慢速代理和结算计算；外部履约缺少证据或传输端口时保持等待，不能当作已完成。

凭据来源不是逐项失败回退：配置完整的 `credential_data_key` 时，两类外部能力都只读取数据库中适用 scope 的 active 凭据；未配置时，邮件使用显式 `keyenv_file` 或敏感配置 `mooglemail_cookie`，论坛使用 `ehentai_cookie`。数据库没有适用凭据不会改用本地 Cookie。

需要联合账号身份证据的履约与资金流程还依赖论坛目标资料读取端口；当前装配仅在论坛启用时提供该端口。只有邮件 Cookie 可用并不代表这些流程已满足全部证据条件。

## Worker control

`[worker.control]` 默认关闭；启用后 Worker 在 Unix socket（默认 `/run/hv-auction/worker/control.sock`，组 `auction-worker-control`）上提供 `live`/`ready`/`status`、邮件轮询进度、手动触发轮询与 `/metrics`。连接按内核 peer 凭据检查显式配置的 `allowed_peer_uids`/`allowed_peer_gids`。Admin、`auction-cli service` 与 `auction-server` 都通过该 socket 判断 Worker 状态，详见 [auction-jobs](../../crates/auction-jobs/README.md#worker-control)。

`/v1/health/ready` 在 control 已标记 serving 且未进入排空时返回 200；无数据库骨架也会标记 serving，因此 200 不能单独证明持久化任务或外部能力可用，还需核对数据库配置、轮询器与相关能力状态。

持久化 Worker 从 common 注入[审计就绪策略](../../ops/config/README.md#就绪策略)并注册审计分类指标。该策略影响数据库心跳及其审计计数，不扩展 control 的运行/排空契约；HTTP 缓存不用于 Worker control 或心跳。

## 退出

- 第一次终止信号：关闭入口，立即取消尚未登记交接的工作且不释放 claim；已登记交接或关键段的 owner 共享一次默认 60 秒截止，之后普通收口默认 10 秒。PM 与主 runner 一起等待，不随主 runner 结束直接 abort。超时或第二次信号强制非零退出，跳过可选 tracing 刷新。
- 独立邮箱维护任务同样受运行时监督与排空：停止后不开始新 `sweep` 或 Read Mail 观察，在途阶段保留原总预算与未知外写围栏。
- 运行时故障（control 服务停止、装备私信循环停止、持久化循环意外退出、凭据所有权失效）同样触发排空，并**总是以非零状态退出**，由 systemd（`Restart=on-failure`）或 `auction-server` 处理重启。已领取但未完成的任务依靠持久租约恢复：过期后可被重新领取，处于发送中的外部写入只读对账，不会因为退出而重复发送。

## 部署

unit 示例见 [`ops/deploy/systemd/auction-worker.service.example`](../../ops/deploy/systemd/auction-worker.service.example)（`RuntimeDirectory=hv-auction/worker` 管理 control socket 目录）。每个部署只运行一个 Worker 实例。

delivery/PM/discard 在持久围栏调用前登记交接，围栏调用开始即受保护；关闭先赢时不发起围栏，已登记后即使进入排空也继续等待原围栏回执。登记本身不授权外写，只有围栏确认提交才执行外部操作；确认失败或提交未知仍不外写。论坛在已有持久 claim 下的第一个实际 POST 前登记，后续必要 POST 沿用原活动。交接与关键段均阻止排空结束，结果按原端口提交后才 complete。装备准备、Sent/compose 回读和 PM Sent 核对都属于原关键段，告警邮件复用 delivery。未收口 drop 只减少活动计数并记录未知边界，不补写持久状态、不授权重发。既有单项超时、lease 与所有权校验保持原值。配置、unit 120 秒和完整旧路径回退见[排空与停止预算](../../ops/config/README.md#排空与停止预算)。
