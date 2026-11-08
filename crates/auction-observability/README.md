# auction-observability

[docs/README.md](../../docs/README.md) · [ops/README.md#监控与日志](../../ops/README.md#监控与日志)

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

I-A/I-B 的 jobs/application/integrations/HTTP 共用 `auction::diagnostics` 诊断门。
键由固定 kind/phase/safe_kind 组成，有界保存 512 个窗口，满时放行原日志；窗口不保存业务标识。
主 Worker 在原 select 中刷新，排空后关闭；PM 共享同一长期窗口。原 delivery 循环的去重仍保留。
Public/Admin 在 tracing 初始化后由原服务 owner 安装 `DiagnosticLogOwner`。
`run` 同时轮询原服务 future 与到期 flush；原服务结束、取消或提前返回时 Drop 执行 close。
不新建后台任务；窗口只持有固定类别与计数，不保存路由模板、request-id、UUID 或整个事件。
未安装日志门的 CLI 等调用方直接输出安全日志。
Public/Admin sink 不安装 Worker observer，不增加 W0.3 计数。
不改变轮询节奏、重试、租约、指标、持久化或结果未知不重发的不变量，其他日志仍走原路径。
默认在 WARN 汇总可见时启用；回退可在现有 `log_filter` 追加
`auction_observability::log_dedup=off`，重启后该 helper 放行所有原始日志。
也可移除调用点恢复原日志。汇总 target 被过滤时不执行抑制，避免计数不可见。
配置文件与重启规则见[ops/config/README.md#commontoml](../../ops/config/README.md#commontoml)。

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
| `auction_http_rate_limit_rejections_total` | 实际频率拒绝次数，label 为 `role`、`kind`（`login`、`mutation`、`realtime`、`browser_error`） |
| `auction_browser_errors_total` | 已接受的浏览器错误信标，label 为 `role`、`class`（`wasm_panic`、`dto_decode`、`realtime_protocol`） |
| `auction_browser_error_build_mismatches_total` | 上报构建号与当前进程构建号不一致的信标数，label 为 `role`；上报值不进入标签 |
| `auction_worker_loop_iterations_total`、`auction_worker_loop_iteration_duration_seconds`、`auction_worker_schedule_delay_seconds` | Worker 各循环的轮次、耗时与调度延迟 |
| `auction_worker_batch_items_total`、`auction_worker_batch_capacity_total`、`auction_worker_batch_saturated_total` | 批次利用率与饱和次数 |
| `auction_worker_retry_attempts_total`、`auction_worker_retry_mode_bound` | 重试次数与重试模式 |
| `auction_worker_unpersisted_error_total` | 未确认对应持久化失败、退避或人工处置的错误；见[口径](#未持久化错误) |
| `auction_worker_task_panics_total{loop,owner}` | 工作项 join 收口观察到的 panic；owner 为闭集，取消不计数 |
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
| `auction_shadow_evaluations_total{owner,branch}` | W0.15 共享影子框架：各 owner/分支完成的可比较样本数，预置零值 |
| `auction_shadow_mismatch_total{owner}` | W0.15 共享影子框架：新旧判定不一致样本数；切换前必须为零 |

指标默认关闭；Public/Admin 业务 listener 必须是 loopback TCP 或 Unix socket。
三个装配根可另开默认关闭的 `127.0.0.1` 指标出口，复用本进程同一注册表；
scrape 只编码内存，不读取数据库或外部服务，不新增业务或高基数 label。
绑定/服务失败只降级采集，固定类别 ERROR 不改变业务判定或 readiness。
附加任务随装配根退出取消；Worker control 原授权路径不变。

业务指标路由与附加出口共用转发头拒绝、GET/HEAD、query、404/405 实现。
反向代理仍屏蔽公网 `/metrics`；完整契约见
[docs/contracts/routes.md#健康与指标publicadmin](../../docs/contracts/routes.md#健康与指标publicadmin)。
地址、开关及 Admin 覆盖关系见[ops/config/README.md#commontoml](../../ops/config/README.md#commontoml)，
Prometheus/exporter 安装与采集核对见[ops/README.md#指标](../../ops/README.md#指标)。

### HTTP 与连接口径

HTTP 指标由 `boundary::enforce` 统一记录，覆盖业务 API、健康检查、`/metrics`、静态资源和未知路径，包括 request-id 拒绝、方法错误、限流、panic 与超时生成的响应。`role` 只有 `public`、`admin`；`route` 来自 axum `MatchedPath`，缺少模板时固定为 `__unmatched__`（当前也聚合 SPA 和静态资源 fallback）；`method` 为 GET、HEAD、POST、PUT、PATCH、DELETE、OPTIONS、TRACE、CONNECT，其他方法聚合为 `OTHER`。`status` 为协议三位精确状态码，非协议值聚合为 `OTHER`。只建立实际出现的路由组合，不把原始路径、资源指纹、query、用户、IP、request-id 或 Cookie 放入 label。

耗时从进入边界到安全响应头装配完成，不包含响应体传输或 WebSocket 升级后的连接寿命。有限桶与业务直方图一致，为 `0.001、0.005、0.01、0.05、0.1、0.5、1、5、30` 秒，另含 `+Inf`；直方图不带 status，避免桶随状态码倍增。业务 SLO 应按业务路由筛选，排除健康、指标与 fallback 聚合流量；本次 scrape 的 HTTP 计数在编码完成后更新，下次采集可见。

WebSocket 只在成功进入 `on_upgrade` 回调后增加当前数和累计数；Drop 守卫跟随连接任务，在正常退出、错误、取消与异常展开时减回当前数，升级失败不计连接。累计数随进程重启归零；当前数不替代 Hub 在握手前取得的配额和排空许可。Admin 不注册 WebSocket 指标。

限流指标只在 `session_guard`、`admin_session_guard`、`realtime_connection_guard` 实际返回频率拒绝时累加；`realtime_busy`、`discovery_busy` 等容量拒绝不计入，仍体现在 HTTP 状态码计数中。关闭指标时上述记录器和连接守卫均为 no-op，不注册 collector，不改变配额、认证、缓存或业务结果。

审计 label 全部为编译期闭集：`class` 为 `transient/permanent/unknown`；`sqlstate_class` 为 `08/09/21/22/23/25/27/28/39/3D/3F/40/42/44/53/55/57/58/P0/XX`、`other` 或非 SQL 的 `none`；kind 与映射由 [crates/auction-storage/README.md#审计失败与就绪](../auction-storage/README.md#审计失败与就绪)拥有。失败族在首次观察对应组合后出现；影子三类及缓存/检查各结果预置零值。审计 collector 只读取内存快照，并发 scrape 不重复累计；HTTP 记录器由装配根创建的 `ProcessMetrics` 注册并注入探针。观测和切换见[ops/README.md#审计就绪影子观察](../../ops/README.md#审计就绪影子观察)。

### Final 阶段每次出价的页面重读（W12.2）

实时事件只通知“竞价发生变化”，页面随后重读权威接口；本口径只用 W0.9 的 HTTP 计数
度量这个重读量，不改变读取语义，也不从事件负载推导竞价结果。分母取同一窗口内
`auction_business_events_total{event="bid",outcome="success"}` 的增加量，即被接受的出价事实数。

lot 详情页在一次通知后只重读两份权威数据，均按模板分别计数：
`GET /api/v1/lots/{lot_id}`（详情与当前价）与
`GET /api/v1/auctions/{auction_id}/bid-log`（竞价历史）。同期打开的看板、账号等页面
各有自己的读取（`/api/v1/auctions/{auction_id}/lots`、`/api/v1/me/bids` 等），比较时
需一并计入或单独列出；不要混入 `/metrics`、`/health/*`、`__unmatched__`（SPA 与静态
资源 fallback）与写入路由。

```promql
(
  sum(increase(auction_http_requests_total{role="public",method="GET",status="200",route="/api/v1/lots/{lot_id}"}[15m]))
  +
  sum(increase(auction_http_requests_total{role="public",method="GET",status="200",route="/api/v1/auctions/{auction_id}/bid-log"}[15m]))
)
/
sum(increase(auction_business_events_total{event="bid",outcome="success"}[15m]))
```

客户端对同一条竞价通知使用“从首条待处理通知起 100 毫秒、后续通知不延后截止”的合并
窗口（[crates/auction-web-public/src/app/surface/board/refresh.rs](../auction-web-public/src/app/surface/board/refresh.rs)），窗口内
N 条通知对同一资源只重读一次；资源尚未就绪时不消费修订号，就绪后仍读最新快照。
因此期望重读量约为“打开的详情页数 × 2 × 该窗口内的重读轮次”：出价间隔不短于窗口时
每笔各读一次，密集突发时按 100 毫秒窗口摊薄，但不会完全合并成一次。比值长期高于
该期望说明存在未合并或重复的读取，应调整窗口或减少资源重启，而不是减少权威读取。

单页读数只能作参考：`auction_websocket_connections{role="public"}` 只统计已升级的
WebSocket 连接，未认证页面（手动刷新）不计入。Final 窗口按场次
`final_phase_at` 到 `effective_end_at` 取相同时间范围；进程重启后计数器归零，使用
`increase` 而不是差分配额。W12.2 的线上基线待部署后在真实 Final 窗口采集，当前不预填。

### 迁移就绪指标

`ProcessMetrics::migration_readiness` 返回可克隆记录器，三角色将其注入数据库适配器；heartbeat 与 HTTP 底层数据库检查共同累计，HTTP 缓存命中不发起新检查。`branch` 与 `class` 都是 `current`、`behind`、`ahead`、`compatible_ahead`、`incompatible_ahead`、`invalid_history`、`query_failed` 的闭集，预置零值。`ahead` 只表示 strict 模式未读取兼容登记；`query_failed` 包含本次检查的 SQL/解码失败或内部超时；被外层 HTTP 预算先取消的检查由原 HTTP timeout 指标记录，可能尚未产生迁移分支计数。

只有迁移 shadow 模式出现 strict/compatible 布尔差异时增加 mismatch；预期仅 `compatible_ahead`，其他类别应为零。版本号只在结构化日志，不进入 label；metrics 关闭时记录器为 no-op，日志与失败关闭规则保持。本项暂沿用 W0.14 的局部命名；W0.15 共享框架（下文）已提供，迁移随后续工作项，不把同版无差异或无样本作为超前分支验收。判定及开关由[docs/contracts/database.md#迁移兼容登记与就绪](../../docs/contracts/database.md#迁移兼容登记与就绪)与[ops/config/README.md#就绪策略](../../ops/config/README.md#就绪策略)定义。

### 影子运行共享框架（W0.15）

改变判定口径的工作项先影子运行：旧路径继续裁决，新路径只并行计算待比较的值。共享契约在
[crates/auction-application/src/ports/shadow_run/mod.rs](../auction-application/src/ports/shadow_run/mod.rs)：`ShadowOwner`
是 owner 闭集并声明自己的判定分支，`ShadowOutcome` 为 `match/mismatch/uncomparable`，
`ShadowRecorder` 只累加计数。调用点用 `record_comparison` 比较新旧结果，用
`record_uncomparable` 声明没有可比证据的样本；两个 helper 都不返回可影响控制流的值。

装配根从 `ProcessMetrics::shadow` 取得记录器；三角色都注册同一注册表，未接线 owner、
关闭指标或其他角色使用 no-op，比较与日志路径保持一致。框架本身不读取配置、不执行 I/O、
不创建后台任务，也不改变判定、重试、锁、租约或持久化。当前生产调用点是 W2.5 告警读侧
（owner `disposition_reads`，分支为 `delivery_failed`、`notification_failed`、
`notification_oldest_failed_age`、`notification_postage`、`notification_oldest_postage_age`、
`fulfillment_failed`）：`shadow` 模式下可见值仍是旧口径，分类口径只产生差异样本；
配对口径读不到时六个分支都记为 `uncomparable`。另两个 owner：

- `audit_readiness`（W0.14）：每次审计写入失败按类别（`transient`/`permanent`/`unknown`）记一条
  样本，旧口径为“任一失败阻断”，分类口径为“只有非瞬时阻断”，因此 `transient` 的差异是预期
  差异（决策 72）。记录器由三角色装配根经 `auction_storage::install_audit_shadow_recorder`
  进程级安装一次；原有 `auction_readiness_shadow_mismatch_total{class}` 保留。没有审计失败时
  三个分支都没有样本，这本身就是正常状态，切换依据见决策 72，而不是“每个分支都有样本”。
- `sent_baseline`（W1.4）：Worker 发送前 Sent 基线的每次可比较评估按 `SentBaselineBranch`
  分支记一条样本（MID 集合与候选集合都一致为一致），候选边界无法建立时记在 `full_fallback`
  分支的不可比较原因下；原有分 world 的 `auction_mooglemail_sent_baseline_*` 指标保留。

| 指标 | 口径 |
| --- | --- |
| `auction_shadow_evaluations_total{owner,branch}` | 每个分支完成的可比较样本数，包含一致与不一致；`branch` 必须已在该 owner 的闭集中登记 |
| `auction_shadow_mismatch_total{owner}` | 新旧判定不一致的样本数；正常可比较样本必须为零，切换条件另见交付规范 |

标签全部来自编译期闭集并预置零值；未登记的分支会被丢弃并输出 ERROR，不扩张 series。
`uncomparable` 不计入 evaluations，因此不能作为“差异为零”的证据。

结构化日志 target 为 `auction::shadow`，字段只有 `owner`、`branch`、`outcome` 与
`uncomparable` 的 `reason`；差异与不可比较为 WARN，一致为 DEBUG。不得写入 MID、账号、
正文、URL、错误原文或任何业务主键；样本量大时由调用点复用 W0.11 去重。

接入新的影子 owner：在 `ShadowOwner` 增加变体并登记其分支标签；在旧路径得出裁决后调用
`record_comparison`（不可比较时调用 `record_uncomparable`）并把 `ProcessMetrics::shadow`
注入该 owner；观察期覆盖至少一个含 Final 与结算的完整场次周期、每个分支都有样本且差异
为零后，由用户切换并删除旧路径。新增 owner 或分支时同步本节与
[docs/plans/architecture-improvement.md#5-每个工作项的交付规范](../../docs/plans/architecture-improvement.md#5-每个工作项的交付规范)。回退为移除
调用点：指标与日志是附加输出，无迁移、无持久状态。

### Sent 发送基线局部影子指标

Worker 的 `ProcessMetrics::sent_baseline` 注入应用层窄观测端口；四族均按实际事件累计，scrape 不重复增加。关闭指标或其他角色得到 no-op，不影响证明、日志或失败关闭。本切片保留局部 family；[影子运行共享框架](#影子运行共享框架w015)已提供，迁移时按 `owner/branch` 口径合并，`world` 并入 branch 标签。

| 指标 | 口径 |
| --- | --- |
| `auction_mooglemail_sent_baseline_shadow_evaluations_total{world,branch}` | 完成可比较的新旧新增 MID 集合及列表投影候选集合评估，包含一致与不一致样本 |
| `auction_mooglemail_sent_baseline_shadow_mismatch_total{world,class}` | 实际集合差异；`mid_set` 与 `candidate_set` 可在同一评估中各增加一次，正常可比较样本均须为零 |
| `auction_mooglemail_sent_baseline_shadow_uncomparable_total{world,reason}` | 无法完成等价比较；旧路径超限、超时、窗口变化、拒绝或取消不能计作一致样本 |
| `auction_mooglemail_sent_baseline_fallback_total{world,phase,reason}` | 在原截止点内尝试完整枚举；失败尝试也计数，不代表发送成功 |

标签全部为闭集并预置零值。`world` 为 `main/isekai`；`branch` 为 `empty/new_minute/same_minute/same_minute_cross_page/no_candidate/multiple_candidates/full_fallback`，按无候选、多候选、完整回退、空边界、分钟分支顺序分类。`phase` 为 `before_send/after_send`。`reason` 为 `invalid_mid/order_violation/page_discontinuity/anchor_missing/anchor_changed/page_limit/timeout/read_unavailable/cache_boundary_delta/boundary_unavailable/scope_changed/response_hint_conflict/canceled`；`canceled` 包括 prepared 未消费或写入被拒，表示没有完成比较，不宣称发生了网络取消。跨页相同 MID/时间重复记 `page_discontinuity` 并尝试完整回退，同一 MID 的时间变化仍属前提失效。缓存捕获窗口与 fresh 顶部不一致记 `cache_boundary_delta`，不冒充候选一致。

日志只输出 world、固定分支/原因/阶段、页数、集合一致布尔值及差集数量，不输出 MID、收件人、主题、正文或凭据。旧路径触顶而水位边界可取得是预期可用性差异；shadow 此时不发送，计不可比较而非通过。观察与切换见[ops/README.md#sent-发送基线影子观察](../../ops/README.md#sent-发送基线影子观察)。

### 邮件增量扫描指标

Worker 的 `ProcessMetrics::mail_scan` 注入应用层闭集观测端口；Sent、Inbox 与 Read Mail 的计划、前缀、完整校验和影子比较分别累计。关闭指标或其他角色得到 no-op，扫描窗口、回退与证明路径保持不变。水位只缩短读取范围，不是发送、收取或已读证明。

| 指标 | 口径 |
| --- | --- |
| `auction_mooglemail_scan_plan_total{world,stream,mode,reason}` | 本轮完整或增量选择；`reason` 为闭集，增量固定为 `incremental` |
| `auction_mooglemail_scan_prefix_total{world,stream,outcome,reason}` | 前缀结论；只有到达锚点才是 `complete`，缺失、重复、顺序异常或超限计回退 |
| `auction_mooglemail_scan_verification_total{world,stream,outcome,reason}` | 每日或显式完整校验；`page_limit` 只告警，不阻断发送或轮询 |
| `auction_mooglemail_scan_shadow_total{world,stream,outcome,result}` | 完整枚举与前缀的“新增 MID”集合比较；`result` 为 `match/mismatch` |

`world` 为 `main/isekai`；`stream` 为 `sent/inbox/read_mail`；`mode` 为 `full/incremental`；`outcome` 为 `complete/page_limit/error/canceled`。`reason` 与 `MailScanReason` 的固定标签一致，只预置实际可达组合。日志只输出 world、流、页数与固定原因，不输出 MID、收件人、主题、正文或凭据。

### 邮件循环口径

`mooglemail_poll_isolation_enabled` 为 true 时，`mail-main`、`mail-isekai`
的轮次耗时只覆盖收件，不包含独立维护和纯观察；关闭时仍覆盖原三分支 join，
结果沿用收件分支的口径。切换不改变在途轮次口径。隔离路径增加以下固定 `loop`
label，均只计 Worker 阶段指标，不映射为 `BusinessEvent::MailIntake`，
不增加业务收件事件与耗时：

| label | 计时与结果 |
| --- | --- |
| `mail-maintenance-main`、`mail-maintenance-isekai` | 从 `sweep` 开始到返回，包含数据库等待、凭据排队和全部远端操作；超时或 `Err` 为 `error`，`Ok(report)` 含 `compose_error` 或 `sent_state_error` 也为 `error`，其余为 `success` |
| `mail-read-observation-main`、`mail-read-observation-isekai` | 仅在完整 Read Mail 纯观察实际执行时记录；成功取得完整列表数量为 `success`，超时或错误为 `error` |

在途阶段因排空超时或任务取消而中断时，守卫记录一次 `canceled`；停止后没有开始的阶段不计轮次。维护与观察不绑定内联或持久重试指标。`auction_worker_loop_iteration_duration_seconds` 的有限桶为 `0.01、0.05、0.1、0.25、0.5、1、5、30、60、120、300` 秒，可用于观察维护 p95 是否低于 60 秒；调度延迟从叠加 jitter 后的计划唤醒时刻开始计算。

Read Mail 日志的 `read_observed` 始终是完整列表数量；观察降频只改变采样节奏，失败不输出部分数量。配置与回退见[ops/config/README.md#mooglemail-收件与维护](../../ops/config/README.md#mooglemail-收件与维护)。

### 外部账号客户端指标（W0.4/W0.5）

`ProcessMetrics::external_metrics` 只在 Worker 角色注入真实记录器；
其他角色或关闭 metrics 时使用同一 no-op。记录器同步、无 I/O、无 `.await`，
不改变请求间隔、锁范围与顺序、deadline、重试、取消、写入围栏或“未知不重发”；
指标调用先于日志过滤与去重。客户端物理请求、共享读取、Worker 预算和导入估计
属于不同层，计数不可相加当作“总失败”。
本包只拥有注册与记录；采集与保存仍依赖 W0.10 的 loopback 出口和外部 Prometheus。

#### MoogleMail（W0.4）

| 指标 | 类型 | labels 与口径 |
| --- | --- | --- |
| `auction_mooglemail_requests_total` | Counter | `{world,operation}`；物理执行前加一，不证明对端收到 |
| `auction_mooglemail_request_results_total` | Counter | `{world,operation,category}` |
| `auction_mooglemail_request_signals_total` | Counter | `{world,operation,kind}` |
| `auction_mooglemail_request_gate_wait_seconds` | Histogram | `{world,operation,outcome}` |
| `auction_mooglemail_lock_wait_seconds` | Histogram | `{world,operation,lock,outcome}`；只测获取等待 |
| `auction_mooglemail_budget_exhaustions_total` | Counter | `{world,phase,budget}`；预算 owner 至多一次 |
| `auction_mooglemail_request_interval_seconds` | Gauge | 无 world；共享门的配置间隔，不是实测请求间隔。 |

`world` 只能是 `main/isekai`。`operation` 为 `inbox`、`read_mail`、`detail`、
`sent`、`sent_detail`、`balance`、`equipment`、`identity`、`inventory`、
`inventory_organize`、`equipment_unstore`、`equipment_unlock`、`compose`、
`attach_add`、`attach_cod`、`send`、`return_message`、`attach_remove`、`discard`；
保留既有 `Take → attach_remove` 命名。`category` 为 `ok/canceled` 加
`configuration`、`credential_unavailable`、`session_expired`、`account_busy`、
`rate_limited`、`remote_temporary`、`sent_scan_page_limit`、`sent_scan_timeout`、
`persistence_unavailable`、`response_too_large`、`protocol_drift`、
`identity_mismatch`、`identity_unproven`、`not_observed`、`write_unknown`。
门等待 `outcome` 为 `acquired/error/canceled`；锁等待只用 `acquired/canceled`。

`phase` 为 `poll_fetch`、`poll_batch`、`maintenance`、`read_observation`、
`shared_read`、`sent_scan`、`read_scan`、`sent_baseline_before`、
`sent_baseline_after`；`budget=time/pages`。`poll_fetch`、`poll_batch`、
`maintenance`、`read_observation` 只记录 `time`；`sent_scan`、`read_scan`、
`shared_read`、`sent_baseline_before/after` 记录 `time` 或 `pages`。
共享读取的物理请求在其共享任务内计数；等待者与缓存复用不重复计数。
维护/观察的正常排空取消不冒充预算耗尽；批次即使提交成功前缀，也保留已发生的耗尽。

请求门/锁等待直方图桶为
`0.001、0.01、0.05、0.1、0.5、0.75、1、2、5、15、30、60、120、300` 秒，
另有 `+Inf`。维护/观察耗时继续使用 W1.0 的
`auction_worker_loop_iteration_duration_seconds`，不另建同义耗时族。

#### 外部账号运行时快照（W5.3）

Worker 角色把同一份外部客户端事件广播给 Prometheus 记录器和
`ExternalAccountTracker`；其他角色与关闭 metrics 时仍是 no-op。
tracker 在内存中维护邮件在途请求（最早已开始者决定忙碌时长）、请求门等待次数与累计秒、
最近请求/成功/失败时间、论坛请求边界与 `write_unknown` 次数，以及维护轮次的
`idle/running/succeeded/failed/canceled`。`canceled` 既不记成功也不记失败；
维护轮次只消费新增的 `MailMaintenanceStarted/Finished` 事件，不改变原调度、
完成判定或重试规则。快照不新增 Prometheus 族，不包含凭据、Cookie、URL、
页面正文或游标；锁损坏或字段关联不合法时读取器返回 `None`，
Worker control 返回 `503 capability_unavailable`，Admin 对应分区为 `unknown`。

#### 论坛客户端（W0.5）

| 指标 | 类型 | labels 与口径 |
| --- | --- | --- |
| `auction_forum_requests_total` | Counter | `{world,operation}`；物理请求边界，共享读取的等待者不重复计数。 |
| `auction_forum_request_results_total` | Counter | `{world,operation,category}`；含取消。 |
| `auction_forum_request_signals_total` | Counter | `{world,operation,kind}` |
| `auction_forum_request_gate_wait_seconds` | Histogram | `{world,operation,outcome}`。 |
| `auction_forum_request_interval_seconds` | Gauge | 无 world；共享门的配置间隔。 |
| `auction_forum_topic_scan_pages` | Histogram | `{world,mode,measure,outcome}`；见下节 |
| `auction_forum_write_unknown_total` | Counter | `{world,operation}`；每次逻辑写入返回未知结果一次。 |
| `auction_forum_pm_operations_total` | Counter | `{world,phase,outcome,category}`。 |
| `auction_forum_budget_exhaustions_total` | Counter | `{world,phase}` |
| `auction_forum_reply_delay_estimate_seconds` | Histogram | `{world,phase,method,bound}`；见下节 |
| `auction_forum_reply_delay_samples_total` | Counter | `{world,phase,method,outcome}`；覆盖与缺样 |

`world` 为 `main/isekai/shared`；`shared` 只用于无权威世界上下文的资料读取，
主题与 PM 不使用。`operation` 为 `check_session`、`create_topic`、
`reconcile_topic`、`fetch_topic`、`reply_topic`、`edit_post`、`read_post`、
`profile`、`pm_prepare`、`pm_send`、`pm_sent_list`、`pm_sent_detail`。
`category` 为 `ok/canceled` 加 `configuration`、`credential_unavailable`、
`session_expired`、`rate_limited`、`remote_temporary`、`cloudflare`、
`topic_locked`、`permission_denied`、`response_too_large`、`protocol_drift`、
`write_unknown`。

主题扫描当前只记录 `mode=full`。`discovered` 表示导航声明的页数，
在页数上限拒绝前记录；`processed` 表示已消费并校验的页数，成功为 `complete`，
失败为 `error`，取消为 `canceled`。并发读取任务数不是扫描页数，实际开销仍看请求计数。
页数桶为 `0、1、2、3、5、8、10、15、20、50、100`。

`write_unknown` 只在逻辑写入的远端结果未知时加一，包括写后会话失效、缺少精确证明，
以及越过写入边界后的取消；只读核对失败不算新的未知写入。
PM 合法组合：prepare 为 `prepared/error`，send 为 `proved/unknown`，
reconcile 为 `proved/not_observed/error`，尚未进入写阶段的取消可记 `canceled`；
`category` 为 `none/unavailable/authentication/rate_limited/`
`invalid_recipient/protocol/outcome_unknown`。
`proved` 必须已有完整回执；`not_observed` 不表示未发送。

#### 回帖导入延迟

终点是每个回帖第一条**新**命令账本提交得到确认，不是竞价最终接受。
`pending` 重试与已完成命令重放不计新命令；提交回执丢失可能漏样，不补造零样本。
`ForumCommandStart.is_new` 只表达本次是否首次插入，不进入浏览器 DTO。
领取时从已连接的拍卖状态投影 `phase=normal/final/unknown`；跨阶段样本
不能作为严格 Final SLA 证明。

本批只实现 `method=poll_window`。上次完整扫描开始为 `A`，本次完整扫描完成为 `B`，
首次导入确认为 `I`，记录区间 `[I-B, I-A]` 与中点；中点只是统计估计。
该方法依赖完整扫描与稳定可见性假设，窗口宽度包含扫描间隔和扫描耗时，
不能用配置轮询间隔代替。首次扫描、覆盖不完整、重启、状态淘汰或时钟异常时
不填零：`outcome` 为 `observed/missing_baseline/coverage_unknown/state_evicted/`
`timestamp_unavailable/clock_invalid`。估计器只保存有界内部 ID 与单调时刻，
不含玩家名、MID、正文、URL 或凭据。

`method=posted_minute` 本批不实现；只有后续核实页头时间、时区和 DST 后才扩展，
分钟值应解释为 `[T,T+60s)`。延迟桶为
`1、5、10、30、60、120、300、600、1800、3600、86400` 秒。
Final 分位目标、允许误差与最低覆盖率由 W0.13 定义；本批只提供样本与覆盖率，
不宣称 SLO 达标。

#### 与计划详情页的差异

W0.4 原列 `poll/cycle.rs` 为主要接入点；W1.0 后维护/观察的实际预算 owner
已在 `worker/mail/maintenance/`，预算指标按该 owner 记录。客户端默认间隔、
Worker 默认 fetch 预算与 W0.12 记录的线上值不可混用。W1.4 当前只交付发送基线切片，
不含常规 Sent/Read Mail 持久水位。当前解析模型没有发帖时间；
“没有发帖时间”只表示代码未提取，不表示线上页头没有时间。W0.12 记录指标开关
全部关闭且没有存储，因此本批只完成实现与静态门禁，实采仍依赖 W0.10。

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

I-B 为集成与 HTTP 补充闭集投影，已知远端和运行时变体不折叠为 other，
类别见[crates/auction-integrations/README.md#安全错误记录](../auction-integrations/README.md#安全错误记录)。
闭集扩展不新增指标族或 label，也不把 HTTP 错误计入 Worker 指标。
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

`auction_runtime_switch_refresh_successes_total{role}` 与
`auction_runtime_switch_refresh_failures_total{role}` 为饱和刷新累计。
`auction_runtime_switch_snapshot_age_seconds{role}` 表示最后成功年龄，初始为 -1。
`auction_runtime_switch_applied_revision{role,switch}` 为已观察事实版本，未设置为 0；
`auction_runtime_switch_effective_value{role,switch}` 的 bool 为 0/1，
审计模式 legacy/shadow/classified 为 0/1/2。
Worker 不导出 HTTP 缓存项，增加 metrics_sli_snapshot_enabled 与
mooglemail_poll_isolation_enabled；Public/Admin 不消费这两项。
三角色都导出 metrics_reject_forwarded_requests。
标签来自 Rust 闭集，scrape 只读内存、不查数据库、不重复累计。
期限回落在使用时重新解析；exporter 关闭不影响刷新或审计累计。
操作见[ops/README.md#运行时开关](../../ops/README.md#运行时开关)。

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

场次写队列指标（同一数据库记录器，关闭指标时为 no-op；队列覆盖同一场次的竞价、生命周期迁移
与 Admin 排期/lot 修改，指标名沿用历史的 `bid_queue` 前缀以保持仪表盘兼容）：

- `auction_bid_queue_wait_seconds{stage}`：实际排队等待秒数。stage 为 `in_process`（同实例按
  场次互斥）与 `cross_instance`（PostgreSQL 会话 advisory lock）。只在真正等待过时观测；
  未经过队列的命令不产生样本。
- `auction_bid_queue_requests_total{outcome}`：经过队列的命令结果。outcome 为 `acquired`、
  `timed_out`（排队预算用尽，映射为暂时不可用）、`canceled`（请求被取消或连接断开，
  凭证在 Drop 中解锁）与 `failed`（加锁语句或连接获取失败）。

两个直方图有限桶为 0.001、0.005、0.01、0.05、0.1、0.5、1、5、30、120 秒，
另含 +Inf。loop 严格复用 WorkerLoop 的 17 项闭集：

```text
mail-main mail-isekai mail-maintenance-main mail-maintenance-isekai
mail-read-observation-main mail-read-observation-isekai delivery auction settlement
auction-fulfillment auction-recovery forum realtime operations-alert-scan
operations-alert-dispatch operations-alert-reconcile player-notifications
```

`settlement` 是 W12.5 拆出的独立结算循环；它只处理结算 claim，不再延长拍卖主循环
（生命周期、慢速代理与周计划）的单轮时长。
持有守卫随普通 Executor、显式 acquire 和事务移动，覆盖 commit/rollback await；
不依赖 after_release，不把 SQLx 后台回滚/归池耗时冒充应用持有时间。
已有池 gauge 仍反映真实池大小与借出数，raw 迁移/LISTEN 获取不伪造 acquisition。

`scope_database_worker_loop` 设置无监听 task-local 归属，
`current_database_worker_loop` 供存储读取。jobs 的 `diagnostics::scope` 同时设置
诊断与连接观测的同一个 WorkerLoop；`inherit` 在创建子任务 future 时捕获现有身份，
再在子任务中进入同一个 scope，不另建循环或改变 W0.3 计数。
新任务不会自动继承 Tokio task-local，业务 JoinSet 子任务显式使用 `inherit`。

覆盖邮件收件、独立维护与 Read 观察、投递、拍卖生命周期与慢速代理、独立结算循环、
结算履约、恢复、论坛同步/回帖/PM、实时消费，以及告警扫描/分发/对账。
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


## SLI 与容量基线

W0.13 只增加观测，不参与竞价、资格、截止、履约或重发判定。
指标和 label 不包含玩家名、各类业务 ID、IP、SQL、正文或错误文本。
目标值暂不确定：**基线观察后由 Claude 提出、用户确认**。
记录规则在 `ops/monitoring/auction-recording.rules.yml`，
告警草案在 `ops/monitoring/auction-alerts.rules.yml.template`，后者不加载。

### 已有指标与实时代理

- 出价 p99 复用 `auction_http_request_duration_seconds`，选择 Public 的
  `POST /api/v1/lots/{lot_id}/bids` 模板；从 HTTP 边界进入到响应生成完成。
  包含认证、解码、事务和响应装配，覆盖全部结果，不含网络传输或浏览器处理。
  成功率与失败状态另看 `auction_http_requests_total`；不能从耗时族筛成功状态。
- `auction_realtime_relay_duration_seconds{scope}` 从 `realtime_events.created_at`
  到本地 sink 成功交给 Hub；无订阅者时也可有样本，不代表客户端收到。
- `auction_realtime_push_duration_seconds{scope}` 从同一起点到一个连接的
  `socket.send` 成功完成；样本为事件乘连接，失败、Ping 和 HTTP 补读不计。
  at-least-once 重投仍可能产生新的写出样本，不以无界集合去重。
- `scope` 仅为 `auction/discovery`，不使用原始 stream。
  创建时间仅随进程内帧携带，JSON 编码、补读、游标和线协议均不变。
  数据库创建时间不是精确 COMMIT 时间；WS 写出也不证明浏览器处理完成。
  起终点为墙钟时间，负时差不补成零，计入
  `auction_realtime_observations_skipped_total{scope,stage}`；stage 为 relay/push。
  正常直方图观察仍受桶分辨率与时钟同步影响。

出价冲突全部引用 W0.6，不在 HTTP、Worker 或 SLI 汇总中重新计数：

| 比率或次数 | 分子 / 分母 |
| --- | --- |
| 尝试冲突中止率 | conflicts_total / attempts_total |
| 遭遇冲突的操作比率 | conflicted_total / transactions_total |
| 操作耗尽率 | retry_exhausted_total / transactions_total |
| Final 耗尽 | retry_exhausted_total，限定 phase=final |

上表省略共同前缀 `auction_db_transaction`；操作分母实际名称是
`auction_db_transactions_total`，尝试分母是 `auction_db_transaction_attempts_total`。
owner 仅取 `bid/proxy/slow_proxy`；按 owner、phase 比较，不混淆尝试与操作。
`unknown` 不猜作 Final；领域拒绝、幂等冲突和 HTTP 503 不能代替数据库冲突。
分子分母均取同一窗口的 rate，分母无样本时不补零。
原始 Final 耗尽累计非零即按已有 D5 条件提前评估 W12.1；
同时保留一小时 increase，但不能只靠 increase 识别首次出现即为 1 的序列。
原计数、重试及 phase 定义见[数据库观测](#数据库事务与连接观测)。

### Worker 窗口快照

`SliMetrics` 只由有数据库且启用 metrics 的 Worker 注册。
`[metrics] sli_snapshot_enabled` 默认 false，运行时 bool 开关为
`metrics_sli_snapshot_enabled`。开关刷新、期限、审计与只读卡片复用 W7.7。
每 60 秒唤醒一次，跳过漏掉的 tick，不追赶补跑；禁用时不查询数据库。
生效等待包含现有五秒开关刷新和下一次六十秒唤醒；
关闭开关不取消已开始、总预算两秒的只读轮次。

汇总通过应用端口调用 storage 的唯一 SQL owner，最多借一个现有空闲连接。
池忙立即跳过，不等连接、不扩池，不建立第二个连接池。
只读 Repeatable Read 事务内串行查询，局部 statement timeout 为 1000ms、
lock timeout 为 100ms，包含 BEGIN、查询与确认回滚的总预算为两秒。
成功只读回滚后原子发布完整数值快照；失败或取消不写任何业务事实。
失败连接丢弃，不将未知清理状态归还业务池；任务退出的事务按 SQLx 取消规则回滚。

窗口为数据库快照时间向前七天，容量候选按事件创建时间取闭区间。
通过 `realtime_events_created_at_idx` 最多读取 10001 条候选，
再按主键有界关联 bid、命令、lot 与场次，不扫描全部历史再筛选。
10000 是候选上限，额外一条只检测溢出；kind/replay 分类在 LIMIT 之后。
慢速代理当前积压使用 `lots_due_slow_idx`，同样最多 10001 条。
任一候选集合超限就使整轮 `limited`，不发布截断统计。
这些是索引路径、输入规模与时间预算约束，本批没有连接数据库验证实际执行计划。

| 指标 | 语义 |
| --- | --- |
| `auction_sli_snapshot_enabled` | 当前 Worker 开关，0/1 |
| `auction_sli_snapshot_available` | 是否曾取得完整快照，0/1 |
| `auction_sli_snapshot_age_seconds` | 查询开始后的单调时钟年龄；首次成功前为 -1 |
| `auction_sli_snapshot_last_success_timestamp_seconds` | 最近成功快照的数据库时间；初始为 0 |
| `auction_sli_snapshot_refreshes_total{outcome}` | 实际轮次；禁用不计轮次 |
| `auction_sli_snapshot_refresh_duration_seconds` | 实际汇总耗时直方图 |
| `auction_sli_flow_available{flow}` | 流程是否有完整可发布数据；不代表数据仍新鲜 |
| `auction_sli_flow_blocked{flow,reason}` | 历史流程的固定阻塞原因；当前八项均已实现，无 awaiting_index 系列 |

outcome 为 `success/busy/timeout/limited/storage/invalid_data`。
失败保留全部旧值及原年龄，不刷新成功时间，不把缺测伪装成恢复。
关闭开关也保留最后快照；查询和看板必须同时核对 enabled、available 和 age。
Scrape 只读内存，容量与积压 Gauge 的一次编码不交错混用两轮快照。
历史数据不会反复 observe 到 Histogram；刷新耗时只记录实际执行的一轮。

`auction_final_bid_commands_per_second{world,kind}` 为最近六十秒接受命令数除以六十；
`auction_final_bid_peak_per_second{world,kind}` 为七天内数据库事件创建秒的最大命令数。
world 仅 main/isekai，kind 仅 ordinary/proxy_create/slow_proxy。
按稳定 command_id 去重，忽略运维 replay，不把自动反击多条事实当多个用户命令。
Final 用已持久化的 final_phase_at 与 bid 创建时间关联，不根据 HTTP 路径猜测。
仅覆盖这三类已接受命令，不代表全部请求负载、失败请求或任意生命周期事实。

`auction_sli_pending{flow,world}` 与 `auction_sli_oldest_pending_seconds{flow,world}`
只实现 flow=slow_proxy_execution：当前 live lot 上仍保留的到期调度数量和最老秒数。
包括已过有效截止但尚未清除的条目，不重做执行资格判定；
字段已被推进或清除的历史世代不在当前积压中，不能据此证明没有漏执行。
完整候选集合为空时才输出零；首次成功前不创建容量和积压数值序列。

WebSocket 容量复用 `auction_websocket_connections`，记录同时连接总数的七天采样峰值。
它可能漏掉抓取间隔内的短峰，不能把各实例不同时刻的最大值相加当同时峰值。
容量观测仅为基线，不是压力测试或能力承诺。
目标保留 `<FINAL_BID_COMMANDS_PER_SECOND>`、`<CONCURRENT_WEBSOCKETS>`。

### 历史流程

迁移 `000077` 的全局时间索引支撑八项只读窗口；Worker 在同一只读快照内按终点落窗。
每个流程先以成功完成时间入口 MATERIALIZED LIMIT 10001，再做类型/关联分类；结算的
支付义务扇出另有 10001 哨兵。任一入口或扇出超限整轮不可评估。每个 flow 发布
`auction_sli_window_duration_seconds{flow,world,quantile}`（仅有效分位）、
`auction_sli_window_samples`、`auction_sli_window_unknown`、
`auction_sli_window_incomplete`、`auction_sli_window_oldest_incomplete_seconds` 与
`auction_sli_window_incomplete_available`；
提醒另发布 `auction_sli_outbid_margin_seconds{world,channel,deadline_state,quantile}`
（p01/p05）、`auction_sli_outbid_samples`、`auction_sli_outbid_non_positive` 与
`auction_sli_outbid_unknown`。无样本不生成分位；坏 metadata 使整轮 InvalidData；
已完成但缺强 Send 证明的候选不进入分位，按 unknown 记录；结算首/末封任一付款投递
缺证明时该场不发布分位并计 unknown。
完整快照后 `auction_sli_flow_available` 对八项为 1，不再有 awaiting_index 系列。
索引存在不等于完整覆盖：给全表扫描加 statement timeout 或对历史主键任意截断，
都不能证明完整窗口。

| flow | 窗口与证据 |
| --- | --- |
| register_key | 成功投递完成窗口、入站回查与注册/key 身份证明 |
| outbid_deadline_margin | 已成功发送的 MM/PM 完成窗口，包含失效后的历史通知，以及有效截止 |
| settlement_first_payment_mail | 场次结算窗口、应发付款邮件完整集合与首封证明 |
| settlement_last_payment_mail | 同上，还须证明全部应发邮件已完成 |
| buyer_payment_to_seller_payout | 货款发送完成窗口、前置付款义务与首次有效付款证明 |
| submission_take | 投稿对应 Take 完成窗口、尝试与入站关联 |
| submission_rejection_notice | 拒收通知发送完成窗口与入站关联 |
| slow_proxy_execution | 按 action/时间枚举全部慢速代理历史审计并核验 metadata |

候选入口分别为 `inbound_mail_sli_received_at_idx`、
`deliveries_sli_succeeded_completed_at_idx`、`lot_watch_pm_sli_succeeded_completed_at_idx`、
`auction_settlements_sli_settled_at_idx` 与 `audit_records_sli_slow_proxy_tick_idx`；
它们不替代按场次、按义务或按主键的点查。当前 slow_due 索引只能描述尚未清除的调度，
不能替代历史执行证据。完整索引证据与 owner 见[docs/contracts/database.md#sli-只读查询](../../docs/contracts/database.md#sli-只读查询)。

这些流程的终点和将来指标口径固定如下，均以秒为单位：

- 注册/key：入站 received_at → key 回信首次获得本地成功发送证明，观察 p50/p95/p99。
  Send 证明只认 `delivery_has_frozen_sent_proof` 的当前 attempt/binding（含人工 operator）；
  注册、拒收、结算付款、seller payout 与 MM 提醒使用同一强谓词。`payload.external_mail_id`
  是入站 MID，不能与出站 Sent MID 直接比较。
- 提醒：lot 有效截止减发送证明时间，观察 p01/p05 及非正余量比例。
  deadline_state 仅 provisional/final；Final 仍在竞价阶段，只有 settling/settled/cancelled
  或截止不可再延长时才为 final。D4 只使用 final；不保存或伪造发送瞬间的截止快照。
- 结算首封：settled_at → 第一封应发付款邮件的成功证明；无应发集合不生成零样本。
  末封：settled_at → 全部应发集合成功后的最后时间；未齐全保持未完成。
  首/末封只认 COD 与 carrier 付款邮件模板；isekai 的 equipment_handoff 是付款后的
  装备交付，不是付款邮件，不进入首/末封集合；`bot_internal_v1` 内部满足的义务
  也不需要邮件，不计入应发集合。候选按付款邮件完成终点落窗，settled_at 可以早于七天。
  候选保留全部成功付款投递；任一投递缺强证明（含更早期次）时该场不给首/末封分位，
  统一计 `unknown`，避免把较晚已证明邮件误当首封。
  当前所有 flow 的 incomplete 覆盖尚未实现：`auction_sli_window_incomplete_available=0`，
  不发布 incomplete/oldest；不能把未查询的未完成集合当作零。待各 flow 的完整未完成
  候选查询（含 failed/manual_reconcile 与缺证据义务）落地后再启用。
- 买家至卖方：首次有效付款 proof_observed_at → 卖方货款发送证明。
  main 直接取 seller_payout 的付款前置；isekai 先经 winner_equipment_handoff 的
  prerequisite 找到真正付款义务。缺可靠付款证据或 seller_payout 投递缺强证明
  计 unknown，不以一般 updated_at 替代。
- 投稿收取：入站 received_at → Take 的成功动作证明，复用 conflicts/intake 的
  action_removed_or_success_page / action_recovered_inbox_v1 owner 谓词，不以本地录入时间替代。
  拒收：入站 received_at → 拒收通知发送证明，覆盖 equipment_rejected、no_attachments 与
  assets_rejected；并另看附件 Return 状态。
  成功投稿接收回执邮件由 W14.3 新增（`auction_submission_receipt`），当前仍没有该邮件的专用 SLI 流程；它按通知种类计入 W14.8 的 created/sent/failed 指标。
- 慢速代理：lot.slow_proxy_tick 的 metadata.due_at → 审计 occurred_at，观察 p95/p99。
  幂等重放不增加审计样本，当前积压不能代替这项历史分位。

完成样本按终点落入七天窗口；未完成项另报数量和最老年龄，不只看成功者。
阈值比率使用已成熟起点群组作分母，不把未知或失败结果算成功。
`unknown` 与 `incomplete` 是覆盖结果，不是零延迟；没有分位序列时不能按 0 解释。
提醒的 p01/p05 与非正余量只对成功发送的 MM/PM 样本计算；`mm`/`pm` 与
`provisional`/`final` 是固定标签。迁移 077 的索引只提供候选入口，
实际关联仍按场次、义务或主键点查。

本地发送证明不是玩家收件时间，proof_observed_at 不是 HV 实际付款瞬间。
只读派生不会为证明指标而访问外部邮箱，也不会授权外部重发。

### Final 回帖导入与目标确认

Final 回帖导入延迟已由 W0.5 接入：
`auction_forum_reply_delay_estimate_seconds{world,phase,method,bound}`，
bound 为 lower/midpoint/upper；以及
`auction_forum_reply_delay_samples_total{world,phase,method,outcome}`。
当前 method 只有 `poll_window`，`posted_minute` 留待核实页头时间后扩展。
终点是第一条新命令账本提交得到确认，导入确认不等于竞价最终接受。
目标必须同时约束延迟上界、估计区间宽度和有效估计覆盖率，不能只约束 midpoint。
若源为直方图，分位边界之差仅表示分位估计区间，不等于逐样本宽度的分位。
当前基线尚无 W0.5 序列，缺测不能解释为零延迟或覆盖率达标。

所有 SLO、告警保持占位，告警模板不加入 rule_files。
覆盖至少一个含 Final、结算与邮件处理的完整场次周期后，
由 Claude 提出目标、用户确认，再作为 W1/W12 验收标准。
无样本、unknown/incomplete、超限或证据不足的流程不能报告 SLO 已达标。
