# auction-observability

tracing 初始化、指标约定、健康检查语义和关联 ID。

源码布局：`src/lib.rs` 只保留模块声明和公共导出；tracing 实现在 `src/telemetry/mod.rs`，
心跳与 dashboard 安全投影实现在 `src/health/mod.rs`。

`init(service, configured_filter)` 安装 JSON 格式的 `tracing-subscriber`。未配置
`log_filter` 时默认 `info,{service}=debug`。Public、Admin 和 Worker 都可以通过
`init_with_config` 挂载默认关闭、异步有界队列的 OTLP/HTTP exporter；Public/Worker 的端点、
批次、采样和关闭等待时间来自主配置 `[tracing]`，Admin 的有效值来自 runtime `[tracing]`。
每个角色的 OTLP headers 只从敏感 TOML 读取，启动时解析后交给 exporter，既不进入普通配置、
日志、指标，也不写入 runtime 文件。导出队列有界，进程停止时会在限定时间内刷新并关闭。

## 结构化日志约定

使用结构化字段，不要把值拼进消息文本：

```rust
info!(auction_id = %id, command_id = %command_id, "bid accepted");
```

事件名可以保留稳定的英文，字段名必须清楚。

## 禁止记录的内容

会话凭据、论坛 Cookie、邮件 Cookie、密码、数据库连接串和其他密钥材料**不得**出现在日志、
DTO、快照或提交的配置中。配置解析错误也不得回显 TOML 原文。

## 健康 dashboard

`HeartbeatStore` 定义服务心跳的发布/读取窄端口，`HealthService` 只生成不含 details 原文的
process/health 快照。Admin `/api/v1/admin/dashboard` 已接入该安全投影；Admin runtime 启用
OTLP 时，心跳还会携带安全的 exporter 成功/失败计数和闭集错误码。mail、realtime、credential
aggregation、delivery、fulfillment、recovery 和 operations-alert owner 已分别接入安全汇总；
缺少对应端口、数据库能力或当前数据源失败时仍返回 `unknown`/`source_unavailable`，不会用示例
数据掩盖缺失。Public/Admin/Worker 装配根已发布启动/就绪/周期/停止心跳，Worker control 还
通过 peer-authenticated Unix socket 提供 live/ready/status 和同一指标 registry。

## 指标与待补齐

Public/Admin 已提供默认关闭的进程本地 Prometheus `/metrics`，暴露旧版同等的进程资源快照
（CPU、内存、文件描述符、线程数和启动时间）以及进程身份和启用状态。
Worker 在启用 control 时通过同一 peer-authenticated Unix socket 暴露 `/metrics`，并注册旧版
固定的 9 类 loop 指标：循环结果/耗时、批次项目/容量/饱和、重试模式/次数和调度延迟。
OTLP exporter 已使用自有有界批处理器，队列满时会增加丢弃计数，并通过 tracing health 与
Prometheus 同步；数据库适配器也会登记 PostgreSQL/SQLite 的当前借出、空闲、总数和最大连接
数快照。`auction-storage` 现在在 SQLx 池的普通 `Executor`、事务和显式 acquisition 边界
维护旧版 pgx 对应的获取累计次数、获取等待时长、空池等待和取消次数，并以同名固定指标
暴露；未接入该存储边界的外部池仍只能提供默认累计值。现在还暴露固定低基数的
`auction_business_events_total` 与 `auction_business_event_duration_seconds`：Public 的竞价/代理
命令、Worker 的邮件/拍卖生命周期/履约/recovery/告警循环都会记录成功、拒绝或错误结果及耗时。
业务指标不携带用户、拍品、命令 ID、邮箱或外部错误文本；需要逐笔证据时仍以数据库审计和事实
记录为准。真实部署切换仍需对照；外部广播 sink 是超出旧版的可选扩展，只有冻结新协议后才新增
对应状态指标。
