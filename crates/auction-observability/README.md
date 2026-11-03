# auction-observability

[文档导航](../../docs/README.md) · [运维手册](../../ops/README.md#监控与日志)

日志、指标、tracing 与服务心跳；三个角色在启动时由装配根初始化。
本 crate 不读配置文件、不监听端口。`auction-http` 挂载 Public/Admin
业务指标路由和三角色附加出口，`auction-jobs` 挂载 Worker control 指标。

## 日志

`init_with_config` 安装 JSON 格式的 `tracing-subscriber`（关闭 ANSI），
过滤器取公共配置的 `log_filter`，省略时为 `info,<服务名>=debug`。
`init_json_stderr` 供 CLI、server 父进程和 Admin 首次设置使用。
CLI 保留资金证据的两个闭集诊断 target，并开启安全 panic 日志；
server 父进程和首次设置只开启安全 panic 日志，不启用 OTLP。
日志不得包含连接串、Cookie、密钥、登录 key 或邮件正文。

### 原生 panic hook

五个原生装配根在日志初始化后调用 `install_safe_panic_hook`。
同一进程只安装一次，替换默认 hook，不再次调用可能泄漏负载的旧 hook；
不捕获 panic，不改变 unwind、abort 或原有退出码。
结构化 ERROR target 为 `auction::panic`，事件不继承父 span。
字段只有 `thread_name`、源码位置 `file`/`line`/`column` 和安全 `payload`。
仅接受 `&'static str` 类型的 payload；`String` 和其他负载统一写为
`<non-static panic payload redacted>`，不执行其 Display 或 Debug。
subscriber 未就绪时仅将同样字段写到 stderr，写入失败不触发二次 panic。
各装配根输出 join 错误时只保留 `panic`、`cancelled` 或 `other`；
顶层 source 链含 JoinError 时不格式化原错误链，返回签名和退出路径不变。

### 重复错误去重

`RateLimitedLog` 是由调用循环显式持有的 helper：每个固定 `target`/固定 `kind` 建立一个实例，键不含错误正文、账号、业务标识或其他动态值。窗口从首条开始，固定 60 秒；这是运维日志聚合周期，不参与业务调度，暂不增加窗口配置字段。窗口内首条（包括 ERROR）由调用方按原级别与原安全字段正常输出，其余只累加抑制计数。到期时在 `auction_observability::log_dedup` target 输出一条 WARN 汇总，含 `log_target`、`kind`、`suppressed_count`、`window_seconds` 与 `window_complete`；没有重复时不输出汇总。正文和动态字段不保存在 helper 中，汇总不继承 span。

调用方先用原事件的 `tracing::enabled!` 检查过滤，再调用 `should_log()`，避免为不可见日志生成汇总；同时必须在已有 Tokio 循环的 `select!` 中持续轮询 `flush_when_due()`，因此停止报错后仍会到期汇总。等待可取消，不创建独立后台任务或全局锁。实例丢弃时补齐尚未汇总的计数，未到期则 `window_complete = false`；进程被强制终止时无法保证汇总。采用显式 helper 可限定接入范围，避免全局 Layer 干预其他日志或 OTLP span。

I-A 新增 jobs/application 的 `auction::diagnostics` 诊断门，复用 `RateLimitedLog`。
键由固定 kind/phase/safe_kind 组成，有界保存 512 个窗口，满时放行原日志；窗口不保存业务标识。
主 Worker 在原 select 中刷新，排空后关闭；PM 共享同一长期窗口。原 delivery 循环的去重仍保留。
不改变轮询节奏、重试、租约、指标、持久化或结果未知不重发的不变量，其他日志仍走原路径。
默认在 WARN 汇总可见时启用；回退可在现有 `log_filter` 追加
`auction_observability::log_dedup=off`，重启后该 helper 放行所有原始日志。
也可移除调用点恢复原日志。汇总 target 被过滤时不执行抑制，避免计数不可见。
配置文件与重启规则见[配置参考](../../ops/config/README.md#commontoml)。

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
| `auction_worker_unpersisted_error_total` | 未确认对应持久化失败、退避或人工处置的错误；见[口径](#未持久化错误) |
| `auction_worker_task_panics_total{loop}` | 现有工作项 join 收口观察到的 panic |
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

指标默认关闭；Public/Admin 业务 listener 必须是 loopback TCP 或 Unix socket。
三个装配根可另开默认关闭的 `127.0.0.1` 指标出口，复用本进程同一注册表；
scrape 只编码内存，不读取数据库或外部服务，不新增业务或高基数 label。
绑定/服务失败只降级采集，固定类别 ERROR 不改变业务判定或 readiness。
附加任务随装配根退出取消；Worker control 原授权路径不变。

业务指标路由与附加出口共用转发头拒绝、GET/HEAD、query、404/405 实现。
反向代理仍屏蔽公网 `/metrics`；完整契约见
[路由契约](../../docs/contracts/routes.md#健康与指标publicadmin)。
地址、开关及 Admin 覆盖关系见[配置参考](../../ops/config/README.md#commontoml)，
Prometheus/exporter 安装与采集核对见[运维手册](../../ops/README.md#指标)。

### HTTP 与连接口径

HTTP 指标由 `boundary::enforce` 统一记录，覆盖业务 API、健康检查、`/metrics`、静态资源和未知路径，包括 request-id 拒绝、方法错误、限流、panic 与超时生成的响应。`role` 只有 `public`、`admin`；`route` 来自 axum `MatchedPath`，缺少模板时固定为 `__unmatched__`（当前也聚合 SPA 和静态资源 fallback）；`method` 为 GET、HEAD、POST、PUT、PATCH、DELETE、OPTIONS、TRACE、CONNECT，其他方法聚合为 `OTHER`。`status` 为协议三位精确状态码，非协议值聚合为 `OTHER`。只建立实际出现的路由组合，不把原始路径、资源指纹、query、用户、IP、request-id 或 Cookie 放入 label。

耗时从进入边界到安全响应头装配完成，不包含响应体传输或 WebSocket 升级后的连接寿命。有限桶与业务直方图一致，为 `0.001、0.005、0.01、0.05、0.1、0.5、1、5、30` 秒，另含 `+Inf`；直方图不带 status，避免桶随状态码倍增。业务 SLO 应按业务路由筛选，排除健康、指标与 fallback 聚合流量；本次 scrape 的 HTTP 计数在编码完成后更新，下次采集可见。

WebSocket 只在成功进入 `on_upgrade` 回调后增加当前数和累计数；Drop 守卫跟随连接任务，在正常退出、错误、取消与异常展开时减回当前数，升级失败不计连接。累计数随进程重启归零；当前数不替代 Hub 在握手前取得的配额和排空许可。Admin 不注册 WebSocket 指标。

限流指标只在 `session_guard`、`admin_session_guard`、`realtime_connection_guard` 实际返回频率拒绝时累加；`realtime_busy`、`discovery_busy` 等容量拒绝不计入，仍体现在 HTTP 状态码计数中。关闭指标时上述记录器和连接守卫均为 no-op，不注册 collector，不改变配额、认证、缓存或业务结果。

审计 label 全部为编译期闭集：`class` 为 `transient/permanent/unknown`；`sqlstate_class` 为 `08/09/21/22/23/25/27/28/39/3D/3F/40/42/44/53/55/57/58/P0/XX`、`other` 或非 SQL 的 `none`；kind 与映射由 [auction-storage 分类表](../auction-storage/README.md#审计失败与就绪)拥有。失败族在首次观察对应组合后出现；影子三类及缓存/检查各结果预置零值。审计 collector 只读取内存快照，并发 scrape 不重复累计；HTTP 记录器由装配根创建的 `ProcessMetrics` 注册并注入探针。观测和切换见[运维手册](../../ops/README.md#审计就绪影子观察)。

### 迁移就绪指标

`ProcessMetrics::migration_readiness` 返回可克隆记录器，三角色将其注入数据库适配器；heartbeat 与 HTTP 底层数据库检查共同累计，HTTP 缓存命中不发起新检查。`branch` 与 `class` 都是 `current`、`behind`、`ahead`、`compatible_ahead`、`incompatible_ahead`、`invalid_history`、`query_failed` 的闭集，预置零值。`ahead` 只表示 strict 模式未读取兼容登记；`query_failed` 包含本次检查的 SQL/解码失败或内部超时；被外层 HTTP 预算先取消的检查由原 HTTP timeout 指标记录，可能尚未产生迁移分支计数。

只有迁移 shadow 模式出现 strict/compatible 布尔差异时增加 mismatch；预期仅 `compatible_ahead`，其他类别应为零。版本号只在结构化日志，不进入 label；metrics 关闭时记录器为 no-op，日志与失败关闭规则保持。暂沿用 W0.14 命名风格，W0.15 共享框架落地后再统一；不把同版无差异或无样本作为超前分支验收。判定及开关由[数据库契约](../../docs/contracts/database.md#迁移兼容登记与就绪)与[配置参考](../../ops/config/README.md#就绪策略)定义。

### Sent 发送基线局部影子指标

Worker 的 `ProcessMetrics::sent_baseline` 注入应用层窄观测端口；四族均按实际事件累计，scrape 不重复增加。关闭指标或其他角色得到 no-op，不影响证明、日志或失败关闭。W0.15 共享框架尚未落地，暂沿用 W0.14 的局部影子风格。

| 指标 | 口径 |
| --- | --- |
| `auction_mooglemail_sent_baseline_shadow_evaluations_total{world,branch}` | 完成可比较的新旧新增 MID 集合及列表投影候选集合评估，包含一致与不一致样本 |
| `auction_mooglemail_sent_baseline_shadow_mismatch_total{world,class}` | 实际集合差异；`mid_set` 与 `candidate_set` 可在同一评估中各增加一次，正常可比较样本均须为零 |
| `auction_mooglemail_sent_baseline_shadow_uncomparable_total{world,reason}` | 无法完成等价比较；旧路径超限、超时、窗口变化、拒绝或取消不能计作一致样本 |
| `auction_mooglemail_sent_baseline_fallback_total{world,phase,reason}` | 在原截止点内尝试完整枚举；失败尝试也计数，不代表发送成功 |

标签全部为闭集并预置零值。`world` 为 `main/isekai`；`branch` 为 `empty/new_minute/same_minute/same_minute_cross_page/no_candidate/multiple_candidates/full_fallback`，按无候选、多候选、完整回退、空边界、分钟分支顺序分类。`phase` 为 `before_send/after_send`。`reason` 为 `invalid_mid/order_violation/page_discontinuity/anchor_missing/anchor_changed/page_limit/timeout/read_unavailable/cache_boundary_delta/boundary_unavailable/scope_changed/response_hint_conflict/canceled`；`canceled` 包括 prepared 未消费或写入被拒，表示没有完成比较，不宣称发生了网络取消。跨页相同 MID/时间重复记 `page_discontinuity` 并尝试完整回退，同一 MID 的时间变化仍属前提失效。缓存捕获窗口与 fresh 顶部不一致记 `cache_boundary_delta`，不冒充候选一致。

日志只输出 world、固定分支/原因/阶段、页数、集合一致布尔值及差集数量，不输出 MID、收件人、主题、正文或凭据。旧路径触顶而水位边界可取得是预期可用性差异；shadow 此时不发送，计不可比较而非通过。观察与切换见[运维手册](../../ops/README.md#sent-发送基线影子观察)。

### 邮件循环口径

`poll_isolation_enabled = true` 时，`mail-main`、`mail-isekai` 的轮次耗时只覆盖收件，不包含独立维护和纯观察；关闭开关时仍覆盖原三分支 join，结果沿用收件分支的口径。新路径增加以下固定 `loop` label，均只计 Worker 阶段指标，不映射为 `BusinessEvent::MailIntake`，不增加业务收件事件与耗时：

| label | 计时与结果 |
| --- | --- |
| `mail-maintenance-main`、`mail-maintenance-isekai` | 从 `sweep` 开始到返回，包含数据库等待、凭据排队和全部远端操作；超时或 `Err` 为 `error`，`Ok(report)` 含 `compose_error` 或 `sent_state_error` 也为 `error`，其余为 `success` |
| `mail-read-observation-main`、`mail-read-observation-isekai` | 仅在完整 Read Mail 纯观察实际执行时记录；成功取得完整列表数量为 `success`，超时或错误为 `error` |

在途阶段因排空超时或任务取消而中断时，守卫记录一次 `canceled`；停止后没有开始的阶段不计轮次。维护与观察不绑定内联或持久重试指标。`auction_worker_loop_iteration_duration_seconds` 的有限桶为 `0.01、0.05、0.1、0.25、0.5、1、5、30、60、120、300` 秒，可用于观察维护 p95 是否低于 60 秒；调度延迟从叠加 jitter 后的计划唤醒时刻开始计算。

Read Mail 日志的 `read_observed` 始终是完整列表数量；观察降频只改变采样节奏，失败不输出部分数量。配置与回退见[配置参考](../../ops/config/README.md#mooglemail-收件与维护)。

### 工作项子任务 panic

`auction_worker_task_panics_total{loop}` 使用全部 15 个 `WorkerLoop` label，
启用指标时预置零值；关闭指标时使用 no-op recorder。
仅生命周期、结算、慢速代理、履约、恢复 action 和 delivery 叶子 join 增加。
生命周期、结算、慢速代理归 `auction`；其余归各自循环。
同一 panic 在日志过滤、去重前增加一次，并产生固定 `item_panic` 诊断，
安全类别为 `SafeErrorKind::TASK_PANIC`（`task_panic`）。
正常取消不增加该指标，循环本身 panic 也不冒充工作项子任务 panic。

邮件、论坛、PM、告警叶子没有新增 catch_unwind；
它们直接执行时的 panic 仍退出常驻任务并触发 Worker 排空。
这是进程内观测计数，不是持久化工作项尝试次数；
计次、封顶与人工阻断尚未实现，W13.3b 待用户决定。
原合并结果、领取、重试和循环退出均不变。

### 未持久化错误

`auction_worker_unpersisted_error_total{loop,kind}` 统计
“本次失败没有确认形成对应的持久化失败、退避或人工处置事实”。
提交结果未知也计入；不因计数补造失败事实、改变租约或授权重发。
该计数随进程重启归零，不能作为持久积压或资金事实。

loop 沿用 `WorkerLoop` 闭集；PM 先归入 forum。
邮件独立维护/观察采用对应阶段 label，旧 join 路径采用原邮件 owner。
kind 来自 ApplicationError、InternalEvidenceError、FundingError 的 safe_kind 闭集，
加 timeout、task_join、lease_expired、configuration_invalid、other；未知类别只生成 other。
子任务 panic 的原 `task_join` 未持久化计数保留，
新增 `task_panic` 诊断不带 Unpersisted 标记，避免重复累计或改分类。
固定类别使用 `SafeErrorKind` 关联常量，既有 `safe_kind()` 字符串才经 `from_label` 映射。
UUID、失败码、日志 kind/phase、正文和远端返回值不得进入 label。
关闭 metrics 时不注册 collector，no-op recorder 不改变处理结果。

记录点由 owner 的最终处置决定：claim/扫描失败、未确认的阶段/围栏/续租/完成、
保存失败退避或人工处置失败；维护把错误装入 Ok(report) 时在原因丢失前记录。
`DiagnosticDisposition::Unpersisted` 以枚举匹配触发计数，不比较日志字符串。
原轮次、批次与重试指标不改口径：error 轮次可能已经保存失败事实，也可能没有确认保存，
不能据此推导本计数。

不计正常排空和取消、正常 stale 竞争、唤醒提示回退，
以及已确认保存的 Deferred/ManualReconcile。
实时 acknowledge 失败后若 record_failure 成功，不重复统计外层 Err。
PM 的 prepare 失败仅 release_unsent 而未保存原因仍计入，
但 send/reconcile 后 finish(None) 已保存人工处置时不计入。
失败回退成功时不统计原错误；回退写入本身失败时记录该失败，避免内外层重复计数。

Worker 观测上下文通过 task-local 窄包装传递，JoinSet 子任务显式继承。
包装不重建 deadline，不 detach，不改变返回值或取消。
指标通知发生在 enabled! 与 RateLimitedLog 之前；关闭或抑制日志不能降低计数。
部署后通过现有 /metrics 采集，以 rate 或 increase 观察，
不能把本次静态门禁当作真实故障样本验收。

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

## 数据库事务与连接观测

数据库 recorder 由 `ProcessMetrics::database` 注入存储适配器，
两种连接模式均使用同一口径；关闭指标时为 no-op，不执行 SQL 或建立监听。
保留已有 `auction_postgres_pool_*` 名称与含义。

新增计数器如下，未注明时标签为 `{owner,phase}`：

- `auction_db_transactions_total{owner,outcome,phase}`：完整 owner 操作。
- `auction_db_transaction_attempts_total`：实际进入的事务尝试。
- `auction_db_transaction_conflicts_total{owner,kind,phase,site}`：冲突失败尝试，
  每次尝试只计一次，含最后一次；重复错误映射不增加计数。
- `auction_db_transaction_conflicted_total`：遇过冲突的完整操作，只计一次。
- `auction_db_transaction_retries_total`：冲突后真正进入下一次事务尝试。
- `auction_db_transaction_retry_exhausted_total`：owner 已确认用尽原尝试预算。
  Deferred、RolledBack 的原返回值保持不变，计数不依赖它是否表现为 Err。

`owner` 初始闭集共 21 项：

```text
bid proxy slow_proxy lifecycle settlement fulfillment funding
delivery_claim delivery_fence delivery_outcome delivery_admin recovery
mail_intake mailbox_sweep scheduler public_identity public_watch admin
runtime_switch equipment_loss sent_confirmation
```

`outcome` 为 success、error、exhausted、canceled；
`kind` 为 serialization_failure（40001）、deadlock（40P01）；
`phase` 为 final、non_final、unknown、not_applicable；
`site` 为 realtime_sequence、other、unknown。

owner 由存储命令边界或原事务 runner 选择，不取 HTTP 路由、用户或业务 ID。
Final 来自事务内权威快照，取不到就记 unknown；不为标签额外查库。
尝试结束时才按已取得的 phase 累计，取消也通过守卫收口。
序号分配 SQL 明确失败记 realtime_sequence；COMMIT 冲突不能由 SQLSTATE 推断热点。
比率分别计算“冲突尝试/全部尝试”和“遇冲突操作/全部操作”，不能混用分母。
Final 耗尽需单独看绝对次数，unknown 未解释前不宣称 Final 零耗尽。

连接生命周期指标：

- `auction_worker_db_acquire_duration_seconds{loop,outcome}`：获取等待秒数。
  outcome 为 success、timeout、error、canceled，失败等待也计入。
- `auction_worker_db_connection_hold_duration_seconds{loop}`：应用持有秒数。
- `auction_worker_db_connections{loop}`：当前应用借出数，取消与异常自动扣回。
- `auction_db_reserved_connections{purpose}`：长期所有权/订阅预留数。
  purpose 为 mail_ownership、forum_ownership、realtime_ownership、delivery_listener。

两个直方图有限桶为 0.001、0.005、0.01、0.05、0.1、0.5、1、5、30、120 秒，
另含 +Inf。loop 严格复用 WorkerLoop 的 15 项闭集：

```text
mail-main mail-isekai mail-maintenance-main mail-maintenance-isekai
mail-read-observation-main mail-read-observation-isekai delivery auction
auction-fulfillment auction-recovery forum realtime operations-alert-scan
operations-alert-dispatch operations-alert-reconcile
```
持有守卫随普通 Executor、显式 acquire 和事务移动，覆盖 commit/rollback await；
不依赖 after_release，不把 SQLx 后台回滚/归池耗时冒充应用持有时间。
已有池 gauge 仍反映真实池大小与借出数，raw 迁移/LISTEN 获取不伪造 acquisition。

`scope_database_worker_loop` 设置无监听 task-local 归属，
`current_database_worker_loop` 供存储读取。jobs 的 `diagnostics::scope` 同时设置
诊断与连接观测的同一个 WorkerLoop；`inherit` 在创建子任务 future 时捕获现有身份，
再在子任务中进入同一个 scope，不另建循环或改变 W0.3 计数。
新任务不会自动继承 Tokio task-local，业务 JoinSet 子任务显式使用 `inherit`。

覆盖邮件收件、独立维护与 Read 观察、投递、拍卖生命周期与慢速代理、结算履约、
恢复、论坛同步/回帖/PM、实时消费，以及告警扫描/分发/对账。
生命周期、结算、慢速代理、履约、恢复 action 和投递工作项的子任务继承父循环身份。
常驻根任务在各自已有入口建立 scope；邮件独立维护与 Read 观察阶段使用各自的嵌套
scope，结束或取消后恢复外层归属。旧版邮件三分支 join 仍沿用收件循环身份，
不改变原有诊断与轮次口径。这里没有新增或移动循环入口。

启动装配、角色心跳、运行时开关刷新、凭据所有权 sticky 状态监视、control 服务、
loopback 指标服务和 tracing 刷新/关闭不属于业务循环，不设置 WorkerLoop。
这些任务的数据库活动仍反映在已有总池指标中，不伪装成某个业务循环。
连接生命周期只统计获取/持有/释放；`auction_worker_task_panics_total` 仍只由既有
join 收口记录，两个 recorder 分别注册于数据库与 Worker owner，不互相增加计数。
ownership/LISTEN 只进 reserved gauge，不混入普通循环持有时长。
LISTEN 的预留生命周期为成功订阅到订阅对象销毁，重连瞬间不作为物理连接精确快照。

所有标签均为代码闭集，不包含 SQL、连接串、Cookie、正文、主键或错误原文。
计数、scope 与守卫不能决定重试、锁、证据期限、结果或缓存命中行为。
