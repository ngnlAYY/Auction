# auction-worker

[文档导航](docs/README.md) · [调度运行时](crates/auction-jobs/README.md) · [结算与履约](docs/guide/settlement.md) · [邮件流程](docs/guide/mail-flows.md) · [论坛](docs/guide/forum.md)

后台任务进程：拍卖生命周期推进、慢速代理、排期、结算与履约、邮件投递、付款超时恢复、MoogleMail 收件轮询与邮箱维护、论坛同步与回帖、装备超价私信、运维告警，以及本机 Worker control。本 crate 负责装配：读取配置、连接数据库与外部服务、取得运行时所有权，然后把端口交给 [`auction-jobs`](crates/auction-jobs/README.md) 的调度运行时。

## feature

| feature | 作用 |
| --- | --- |
| `postgres` | 持久化 Worker 的全部能力 |
| `cache-redis` | 允许 Redis 缓存后端 |

没有 `postgres` 时只能以“骨架”运行（仅 control 与指标），配置了数据库会拒绝启动。

## 启动

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml auction-worker
cargo run -p auction-worker --features postgres   # 本地开发（读取 ops/config/worker.toml）
```

装配顺序：

1. 加载 Worker 配置，初始化日志、tracing 与指标，订阅终止信号。
2. 未配置数据库时：若启用了 MoogleMail、论坛或运维告警则拒绝启动，否则以骨架模式运行。
3. 连接数据库，按 `[worker.mooglemail.send_intervals]` 配置同类邮件的发送间隔。
4. 装配生命周期命令（使用与 Admin 相同的拍卖策略与排期日历）、慢速代理、论坛、邮件轮询。
5. **外部凭据运行时所有权**：启用 MoogleMail 或论坛时，Worker 用 PostgreSQL session advisory lock 取得邮件 Cookie 与论坛 Cookie 的唯一运行时 owner，同一时刻只有一个 Worker 能使用这些凭据——第二个启用了邮件或论坛的 Worker 会因取不到所有权而启动失败。每秒检查一次所有权，一旦失效（例如数据库连接断开）立即触发排空并以非零状态退出，由进程管理器重启后重新竞争。
6. 启用 `[worker.control]` 时绑定 control socket（见下文），发布心跳，启动全部循环。

## 外部能力的启用条件

| 能力 | 配置 | 依赖 |
| --- | --- | --- |
| MoogleMail 收件、投递、履约 | `[worker.mooglemail] enabled = true` 与各世界设置 | 凭据（数据库凭据需 `credential_data_key`，或 `.keyenv`、`mooglemail_cookie`）、`user_login_data_key` |
| 论坛同步与回帖 | `[worker.forum] enabled = true` | `ehentai_cookie` 或数据库论坛凭据（`credential_data_key`） |
| 运维告警邮件 | `[worker.operations_alert] enabled = true` | 收件人、告警世界必须在已启用的 MoogleMail 世界内 |
| 邮件诊断归档 | `[worker.mooglemail.diagnostics]`（默认启用） | `credential_data_key`，否则 `equipment_data_key` 作为加密父密钥 |

诊断归档目录默认是 Worker 配置文件旁的 `mail-diagnostics/`，按保留天数（默认 7）与总字节上限（默认 256 MiB）清理；归档已加密，用 `auction-cli mail export-evidence` 离线导出。

## Worker control

启用后 Worker 在 Unix socket（默认 `/run/hv-auction/worker/control.sock`，组 `auction-worker-control`）上提供 `live`/`ready`/`status`、邮件轮询进度、手动触发轮询与 `/metrics`。连接按内核 peer 凭据检查 `allowed_peer_uids`/`allowed_peer_gids`。Admin、`auction-cli service` 与 `auction-server` 都通过该 socket 判断 Worker 状态，详见 [auction-jobs](crates/auction-jobs/README.md#worker-control)。

`/v1/health/ready` 只有在调度运行时开始服务且未进入排空时才返回 200。

## 退出

- 第一次终止信号：停止领取新任务，在途任务在 10 秒内排空；超时或第二次信号时强制退出。
- 运行时故障（control 服务停止、装备私信循环停止、持久化循环意外退出、凭据所有权失效）同样触发排空，并**总是以非零状态退出**，由 systemd（`Restart=on-failure`）或 `auction-server` 处理重启。已领取但未完成的任务依靠持久租约恢复：过期后可被重新领取，处于发送中的外部写入只读对账，不会因为退出而重复发送。

## 部署

unit 示例见 [`ops/deploy/systemd/auction-worker.service.example`](ops/deploy/systemd/auction-worker.service.example)（`RuntimeDirectory=hv-auction/worker` 管理 control socket 目录）。每个部署只运行一个 Worker 实例。
