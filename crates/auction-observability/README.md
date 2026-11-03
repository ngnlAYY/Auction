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

### 邮件循环口径

`poll_isolation_enabled = true` 时，`mail-main`、`mail-isekai` 的轮次耗时只覆盖收件，不包含独立维护和纯观察；关闭开关时仍覆盖原三分支 join，结果沿用收件分支的口径。新路径增加以下固定 `loop` label，均只计 Worker 阶段指标，不映射为 `BusinessEvent::MailIntake`，不增加业务收件事件与耗时：

| label | 计时与结果 |
| --- | --- |
| `mail-maintenance-main`、`mail-maintenance-isekai` | 从 `sweep` 开始到返回，包含数据库等待、凭据排队和全部远端操作；超时或 `Err` 为 `error`，`Ok(report)` 含 `compose_error` 或 `sent_state_error` 也为 `error`，其余为 `success` |
| `mail-read-observation-main`、`mail-read-observation-isekai` | 仅在完整 Read Mail 纯观察实际执行时记录；成功取得完整列表数量为 `success`，超时或错误为 `error` |

在途阶段因排空超时或任务取消而中断时，守卫记录一次 `canceled`；停止后没有开始的阶段不计轮次。维护与观察不绑定内联或持久重试指标。`auction_worker_loop_iteration_duration_seconds` 的有限桶为 `0.01、0.05、0.1、0.25、0.5、1、5、30、60、120、300` 秒，可用于观察维护 p95 是否低于 60 秒；调度延迟从叠加 jitter 后的计划唤醒时刻开始计算。

Read Mail 日志的 `read_observed` 始终是完整列表数量；观察降频只改变采样节奏，失败不输出部分数量。配置与回退见[配置参考](../../ops/config/README.md#mooglemail-收件与维护)。

## 心跳与健康

`HeartbeatPublisher` 构造并发布安全心跳，周期由三个角色的装配根调度：每 10 秒写入数据库，TTL 30 秒。内容包括角色、实例 ID、版本、启动时间、readiness、tracing 健康与审计写入失败计数；停止时发布 stopping。`HealthService` 汇总心跳供 Admin 仪表盘展示各进程状态，过期心跳不能证明进程就绪。

## 源码结构

| 路径 | 内容 |
| --- | --- |
| `crates/auction-observability/src/telemetry/` | subscriber 装配、OTLP 批处理器与 exporter 健康 |
| `crates/auction-observability/src/metrics/` | 注册表、业务/Worker/连接池/tracing 指标 |
| `crates/auction-observability/src/health/` | 心跳模型、发布器、Admin 仪表盘快照 |
