# auction-observability

[文档导航](../../docs/README.md) · [运维手册](../../ops/README.md#监控与日志)

日志、指标、tracing 与服务心跳。三个角色进程在启动时由装配根初始化，本 crate 不读配置文件、不监听端口——`/metrics` 路由由 `auction-http`（Public/Admin）与 `auction-jobs`（Worker control）挂载。

## 日志

`init_with_config` 安装 JSON 格式的 `tracing-subscriber`（关闭 ANSI），过滤器取公共配置的 `log_filter`，省略时为 `info,<服务名>=debug`。`init_json_stderr` 只供 `auction-cli` 在资金证据阶段输出闭集诊断日志。日志不得包含连接串、Cookie、密钥、登录 key 或邮件正文；各 owner 只记录安全分类与标识。

### 重复错误去重

`RateLimitedLog` 是由调用循环显式持有的 helper：每个固定 `target`/固定 `kind` 建立一个实例，键不含错误正文、账号、业务标识或其他动态值。窗口从首条开始，固定 60 秒；这是运维日志聚合周期，不参与业务调度，暂不增加窗口配置字段。窗口内首条（包括 ERROR）由调用方按原级别与原安全字段正常输出，其余只累加抑制计数。到期时在 `auction_observability::log_dedup` target 输出一条 WARN 汇总，含 `log_target`、`kind`、`suppressed_count`、`window_seconds` 与 `window_complete`；没有重复时不输出汇总。正文和动态字段不保存在 helper 中，汇总不继承 span。

调用方先用原事件的 `tracing::enabled!` 检查过滤，再调用 `should_log()`，避免为不可见日志生成汇总；同时必须在已有 Tokio 循环的 `select!` 中持续轮询 `flush_when_due()`，因此停止报错后仍会到期汇总。等待可取消，不创建独立后台任务或全局锁。实例丢弃时补齐尚未汇总的计数，未到期则 `window_complete = false`；进程被强制终止时无法保证汇总。采用显式 helper 可限定接入范围，避免全局 Layer 干预其他日志或 OTLP span。

当前只接入 Worker delivery 循环的 `delivery_cycle_failed` 日志；不改变轮询节奏、重试、租约、指标、持久化或结果未知不重发的不变量，其他日志仍走原路径。默认在 WARN 汇总可见时启用；回退可在现有 `log_filter` 追加 `auction_observability::log_dedup=off`，重启后该 helper 放行所有原始日志，也可移除调用点恢复原日志。汇总 target 被过滤时不执行抑制，避免计数不可见。配置文件与重启规则见[配置参考](../../ops/config/README.md#commontoml)。

## Tracing（可选）

启用后挂载 OTLP/HTTP span exporter：有界队列（`queue_capacity`）、批量大小与超时、采样比例（百万分率）、关闭时有界刷新；导出头只来自敏感文件中对应角色的 `*_otlp_headers`。导出失败只降级健康状态与指标，不影响业务；支持从入站请求附着 W3C trace context。

## 指标

`ProcessMetrics` 为每个启用指标的进程建立独立的 Prometheus 注册表；`auction_process_info` 使用 `service`/`role`/`version` label，业务和 Worker 指标使用各自的固定闭集 label。关闭指标时不注册 collector，记录器为 no-op：

| 指标族 | 说明 |
| --- | --- |
| `auction_process_up`、`auction_process_info`、`auction_metrics_enabled` | 进程存活与版本信息（另含 prometheus 默认的进程 collector） |
| `auction_business_events_total`、`auction_business_event_duration_seconds` | 固定闭集的业务事件计数与耗时 |
| `auction_http_requests_total` | Public/Admin HTTP 响应数，label 为 `role`、`route`、`method`、精确状态码 `status` |
| `auction_http_request_duration_seconds` | HTTP 响应生成耗时，label 为 `role`、`route`、`method` |
| `auction_websocket_connections`、`auction_websocket_connections_total` | Public 已升级连接的当前数与累计数，label 为 `role` |
| `auction_http_rate_limit_rejections_total` | 实际频率拒绝次数，label 为 `role`、`kind`（`login`、`mutation`、`realtime`） |
| `auction_worker_loop_iterations_total`、`auction_worker_loop_iteration_duration_seconds`、`auction_worker_schedule_delay_seconds` | Worker 各循环的轮次、耗时与调度延迟 |
| `auction_worker_batch_items_total`、`auction_worker_batch_capacity_total`、`auction_worker_batch_saturated_total` | 批次利用率与饱和次数 |
| `auction_worker_retry_attempts_total`、`auction_worker_retry_mode_bound` | 重试次数与重试模式 |
| `auction_postgres_pool_*` | PostgreSQL 连接池大小、空闲、获取次数与等待时长、空池等待与取消次数 |
| `auction_tracing_*` | tracing 是否启用、导出失败/丢弃 span、最近成功/失败时间、是否降级 |
| `auction_audit_write_failures_total{class,sqlstate_class,kind}` | 原始审计 SQL 失败尝试累计，包含回滚后重试成功的尝试；关闭 exporter 不影响底层计数 |
| `auction_audit_readiness_blocked` | 当前审计模式是否因累计失败阻断就绪，0/1 |
| `auction_readiness_shadow_mismatch_total{class}` | shadow 模式实际审计门禁检查的新旧差异累计，transient 为预期差异，permanent/unknown 应为零 |
| `auction_readiness_cache_requests_total{outcome}` | HTTP 请求使用缓存的方式：`hit`、`refresh`、`coalesced`、关闭缓存的 `bypass` |
| `auction_readiness_checks_total{result}` | 实际底层 HTTP 检查数：`ready`、`not_ready`（含异常/取消）、`timeout`；不计缓存命中和合并等待者 |
| `auction_readiness_check_duration_seconds` | 实际底层 HTTP 检查耗时直方图，秒 |
| `auction_readiness_migration_checks_total{branch}` | 实际数据库迁移检查按固定分支累计；不因 HTTP 缓存命中或合并等待者重复累计 |
| `auction_readiness_migration_shadow_mismatch_total{class}` | 迁移 shadow 新旧判定差异；唯一预期差异为 `compatible_ahead` |

指标在配置中默认关闭；Public/Admin 只有在 loopback TCP 或 Unix listener 上才允许开启，避免直接暴露到公网。反向代理示例对公网屏蔽 `/metrics`；应用默认拒绝携带转发头的指标请求，方法、拒绝优先级与回退见[路由契约](../../docs/contracts/routes.md#健康与指标publicadmin)。

### HTTP 与连接口径

HTTP 指标由 `boundary::enforce` 统一记录，覆盖业务 API、健康检查、`/metrics`、静态资源和未知路径，包括 request-id 拒绝、方法错误、限流、panic 与超时生成的响应。`role` 只有 `public`、`admin`；`route` 来自 axum `MatchedPath`，缺少模板时固定为 `__unmatched__`（当前也聚合 SPA 和静态资源 fallback）；`method` 为 GET、HEAD、POST、PUT、PATCH、DELETE、OPTIONS、TRACE、CONNECT，其他方法聚合为 `OTHER`。`status` 为协议三位精确状态码，非协议值聚合为 `OTHER`。只建立实际出现的路由组合，不把原始路径、资源指纹、query、用户、IP、request-id 或 Cookie 放入 label。

耗时从进入边界到安全响应头装配完成，不包含响应体传输或 WebSocket 升级后的连接寿命。有限桶与业务直方图一致，为 `0.001、0.005、0.01、0.05、0.1、0.5、1、5、30` 秒，另含 `+Inf`；直方图不带 status，避免桶随状态码倍增。业务 SLO 应按业务路由筛选，排除健康、指标与 fallback 聚合流量；本次 scrape 的 HTTP 计数在编码完成后更新，下次采集可见。

WebSocket 只在成功进入 `on_upgrade` 回调后增加当前数和累计数；Drop 守卫跟随连接任务，在正常退出、错误、取消与异常展开时减回当前数，升级失败不计连接。累计数随进程重启归零；当前数不替代 Hub 在握手前取得的配额和排空许可。Admin 不注册 WebSocket 指标。

限流指标只在 `session_guard`、`admin_session_guard`、`realtime_connection_guard` 实际返回频率拒绝时累加；`realtime_busy`、`discovery_busy` 等容量拒绝不计入，仍体现在 HTTP 状态码计数中。关闭指标时上述记录器和连接守卫均为 no-op，不注册 collector，不改变配额、认证、缓存或业务结果。

审计 label 全部为编译期闭集：`class` 为 `transient/permanent/unknown`；`sqlstate_class` 为 `08/09/21/22/23/25/27/28/39/3D/3F/40/42/44/53/55/57/58/P0/XX`、`other` 或非 SQL 的 `none`；kind 与映射由 [auction-storage 分类表](../auction-storage/README.md#审计失败与就绪)拥有。失败族在首次观察对应组合后出现；影子三类及缓存/检查各结果预置零值。审计 collector 只读取内存快照，并发 scrape 不重复累计；HTTP 记录器由装配根创建的 `ProcessMetrics` 注册并注入探针。观测和切换见[运维手册](../../ops/README.md#审计就绪影子观察)。

### 迁移就绪指标

`ProcessMetrics::migration_readiness` 返回可克隆记录器，三角色将其注入数据库适配器；heartbeat 与 HTTP 底层数据库检查共同累计，HTTP 缓存命中不发起新检查。`branch` 与 `class` 都是 `current`、`behind`、`ahead`、`compatible_ahead`、`incompatible_ahead`、`invalid_history`、`query_failed` 的闭集，预置零值。`ahead` 只表示 strict 模式未读取兼容登记；`query_failed` 包含本次检查的 SQL/解码失败或内部超时；被外层 HTTP 预算先取消的检查由原 HTTP timeout 指标记录，可能尚未产生迁移分支计数。

只有迁移 shadow 模式出现 strict/compatible 布尔差异时增加 mismatch；预期仅 `compatible_ahead`，其他类别应为零。版本号只在结构化日志，不进入 label；metrics 关闭时记录器为 no-op，日志与失败关闭规则保持。暂沿用 W0.14 命名风格，W0.15 共享框架落地后再统一；不把同版无差异或无样本作为超前分支验收。判定及开关由[数据库契约](../../docs/contracts/database.md#迁移兼容登记与就绪)与[配置参考](../../ops/config/README.md#就绪策略)定义。

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

`audit_failure_count` 在 legacy/shadow 模式为全部失败累计，classified 模式为 permanent + unknown 的阻断累计；发布器、汇总与告警的非零门禁保持一致，不新增 DTO 字段。成功写入不清零，重启归零；全量及细分仍由审计指标保存。心跳使用原始依赖端口，不复用 HTTP 一秒缓存；Worker 心跳另要求 control ready，但 control 探针自身仍仅表示运行/排空状态。

## 源码结构

| 路径 | 内容 |
| --- | --- |
| `crates/auction-observability/src/telemetry/` | subscriber 装配、OTLP 批处理器与 exporter 健康 |
| `crates/auction-observability/src/logging/` | 固定类别错误日志去重与窗口汇总 |
| `crates/auction-observability/src/metrics/` | 注册表、业务/HTTP/WebSocket/Worker/连接池/tracing 指标 |
| `crates/auction-observability/src/health/` | 心跳模型、发布器、Admin 仪表盘快照 |

排空指标为 `auction_shutdown_critical_inflight{kind}`、`auction_shutdown_total{role,reason,outcome}` 与 `auction_shutdown_duration_seconds{role,phase}`。所有 label 从编译期枚举产生，不接收业务标识或错误原文。`kind` 为 background/delivery_send/delivery_take/delivery_return/compose_discard/forum/forum_pm（background 不登记围栏交接或外写关键段）；`role` 为 public/admin/worker，`reason` 为 signal/runtime_failure/server_stopped，`outcome` 为 completed/timed_out/interrupted/failed，`phase` 为 critical/drain。inflight 同时包含围栏调用前登记的交接与外写关键段，表示排空需等待的活动数，不表示已获外写授权。活动 drop 只减少 inflight，未确认结果以结构化日志记录，不能作为已落库证明。Worker 关键段等待期间保留 control 指标读取；最后一次指标可能来不及抓取，停止结论同时记录日志。


## 运行时开关指标

`auction_runtime_switch_refresh_successes_total{role}` 与 `auction_runtime_switch_refresh_failures_total{role}` 为饱和刷新累计；`auction_runtime_switch_snapshot_age_seconds{role}` 表示最后成功快照年龄，未成功为 -1。`auction_runtime_switch_applied_revision{role,switch}` 是已观察事实版本（未设置为 0）；`auction_runtime_switch_effective_value{role,switch}` 中 bool 为 0/1，审计模式 legacy/shadow/classified 为 0/1/2。Worker 不导出不适用的 HTTP 缓存项。标签只来自 Rust 闭集角色和名称；collector 只读内存，scrape 不查数据库，也不重复累计。角色实际值的到期回落会在 scrape 时重新解析。 exporter 关闭不影响刷新或审计累计。操作见[运行时开关](../../ops/README.md#运行时开关)。
