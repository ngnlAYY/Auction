# auction-observability

[文档导航](../../docs/README.md) · [运维手册](../../ops/README.md#监控与日志)

日志、指标、tracing 与服务心跳。三个角色进程在启动时由装配根初始化，本 crate 不读配置文件、不监听端口——`/metrics` 路由由 `auction-http`（Public/Admin）与 `auction-jobs`（Worker control）挂载。

## 日志

`init_with_config` 安装 JSON 格式的 `tracing-subscriber`（关闭 ANSI），过滤器取公共配置的 `log_filter`，省略时为 `info,<服务名>=debug`。`init_json_stderr` 只供 `auction-cli` 在资金证据阶段输出闭集诊断日志。日志不得包含连接串、Cookie、密钥、登录 key 或邮件正文；各 owner 只记录安全分类与标识。

## Tracing（可选）

启用后挂载 OTLP/HTTP span exporter：有界队列（`queue_capacity`）、批量大小与超时、采样比例（百万分率）、关闭时有界刷新；导出头只来自敏感文件中对应角色的 `*_otlp_headers`。导出失败只降级健康状态与指标，不影响业务；支持从入站请求附着 W3C trace context。

## 指标

`ProcessMetrics` 为每个启用指标的进程建立独立的 Prometheus 注册表；`auction_process_info` 使用 `service`/`role`/`version` label，业务和 Worker 指标使用各自的固定闭集 label。关闭指标时不注册 collector，记录器为 no-op：

| 指标族 | 说明 |
| --- | --- |
| `auction_process_up`、`auction_process_info`、`auction_metrics_enabled` | 进程存活与版本信息（另含 prometheus 默认的进程 collector） |
| `auction_business_events_total`、`auction_business_event_duration_seconds` | 固定闭集的业务事件计数与耗时 |
| `auction_worker_loop_iterations_total`、`auction_worker_loop_iteration_duration_seconds`、`auction_worker_schedule_delay_seconds` | Worker 各循环的轮次、耗时与调度延迟 |
| `auction_worker_batch_items_total`、`auction_worker_batch_capacity_total`、`auction_worker_batch_saturated_total` | 批次利用率与饱和次数 |
| `auction_worker_retry_attempts_total`、`auction_worker_retry_mode_bound` | 重试次数与重试模式 |
| `auction_postgres_pool_*` | PostgreSQL 连接池大小、空闲、获取次数与等待时长、空池等待与取消次数 |
| `auction_tracing_*` | tracing 是否启用、导出失败/丢弃 span、最近成功/失败时间、是否降级 |

指标在配置中默认关闭；Public/Admin 只有在 loopback TCP 或 Unix listener 上才允许开启，避免直接暴露到公网。反向代理示例对公网屏蔽 `/metrics`。

## 心跳与健康

`HeartbeatPublisher` 构造并发布安全心跳，周期由三个角色的装配根调度：每 10 秒写入数据库，TTL 30 秒。内容包括角色、实例 ID、版本、启动时间、readiness、tracing 健康与审计写入失败计数；停止时发布 stopping。`HealthService` 汇总心跳供 Admin 仪表盘展示各进程状态，过期心跳不能证明进程就绪。

## 源码结构

| 路径 | 内容 |
| --- | --- |
| `crates/auction-observability/src/telemetry/` | subscriber 装配、OTLP 批处理器与 exporter 健康 |
| `crates/auction-observability/src/metrics/` | 注册表、业务/Worker/连接池/tracing 指标 |
| `crates/auction-observability/src/health/` | 心跳模型、发布器、Admin 仪表盘快照 |
