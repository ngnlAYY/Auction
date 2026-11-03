# W0 错误分类、指标与已知缺陷诊断

[计划入口](../architecture-improvement.md) · [执行须知](../architecture-improvement.md#2-执行须知) · [交付规范](../architecture-improvement.md#5-每个工作项的交付规范) · [进度](../architecture-improvement.md#7-进度) · [现状基线](baseline.md) · [决策记录](decisions.md)

| 项 | 内容 |
| --- | --- |
| 范围 | 错误分类、指标与采集路径、用户可见 SLO、已知缺陷诊断、待核实事实、就绪探针、影子运行框架、浏览器错误信标 |
| 优先级 | P0（W0.15、W0.16 为 P1） |
| 里程碑 | W0.1–W0.14 随 M1（W0.14 先于 W0.6 的超时）；W0.15 随 M2a；W0.16 随 M3 |
| 依赖 | W0.14 先于 W0.6 |
| 迁移 | 否 |
| 估算 | 10–13 天 |
| 关联缺陷 | D1、D3、D4、D5、D6、D7、D10（详见[已知缺陷](../architecture-improvement.md#已知缺陷)） |
| 关联决策 | 12、32、42、43、45、58（详见[决策记录](decisions.md)） |

| 项 | 内容 | 涉及位置 |
| --- | --- | --- |
| W0.1 | 为 `ApplicationError` 增加 `safe_kind() -> &'static str`，闭集分类（如 `funding_unproven`、`evidence_expired`、`repository_conflict`、`remote_busy`），不含秘密与正文 | `crates/auction-application/src/errors` |
| W0.2 | 规则：凡把错误映射为“暂不可用/内部错误”或丢弃后继续的分支，必须记录 `safe_kind` 与业务标识（obligation/delivery/world/路由），保持原控制流不变；同一函数内不同原因必须映射到不同 defer reason。范围覆盖 jobs、应用、集成层的 `Err(_)` 分支与 `map_err(\|_\| …)`（解析闭集按类甄别），以及 HTTP 层 5xx 路径（`internal_error()` 与各 `*_unavailable()`） | `crates/auction-jobs/src/worker`、`crates/auction-application/src/services`、`crates/auction-integrations/src`、`crates/auction-http/src/http/routes` |
| W0.3 | Worker 指标：`worker_unpersisted_error_total{loop,kind}` | `crates/auction-observability` |
| W0.4 | MoogleMail 指标：按 world/operation/类别统计请求数、请求门等待、独占操作锁与 `incoming` 锁等待、`account_busy`/限流/超时次数、维护与观察耗时及预算耗尽次数 | MoogleMail 客户端、`crates/auction-jobs/src/worker/mail/poll/cycle.rs` |
| W0.5 | 论坛客户端同口径指标（主题扫描页数、写入未知、PM、回帖从发帖到导入的延迟估计） | `crates/auction-integrations/src/clients/ehentai` |
| W0.6 | 数据库：连接池已有指标保留；连接按用途分档：服务档（Public/Admin/Worker）设置连接数（扣除所有权长期占用的连接）、`statement_timeout`/`lock_timeout`/`idle_in_transaction_session_timeout`、按角色的 `application_name`，以及短于 HTTP `request_timeout` 的获取连接超时；维护档（CLI 迁移、回填、备份核对、重建）不受 `statement_timeout` 限制、单独命名；各档连接数之和按 W10.8 的 `max_connections` 预算核对。按事务 owner 统计序列化冲突与重试耗尽（量化 D5）；按 Worker 循环统计连接占用。须在 W0.14 上线后启用超时 | `crates/auction-storage/src/database/postgres_store.rs`、`crates/auction-storage/src/database/backend.rs`、`apps/auction-cli/src/database/mod.rs` |
| W0.7 | 用 W0.1 的分类定位 D1：资金化校验各拒绝分支映射到独立 `safe_kind` | `crates/auction-application/src/services/settlement/fulfillment/collected.rs`、`crates/auction-storage/src/database/settlement_funding/receipt.rs` |
| W0.8 | 存储层 SQL 错误安全上下文：保留 SQLSTATE 类别、约束名与表名（schema 标识，非秘密），不保留 `detail`/`message`（可能含值）；新增接收原错误的 helper，以 codemod 把 `storage_error` 的 1432 处调用一次改完，不按 owner 分批 | `crates/auction-storage/src/database/backend.rs`（`storage_error`）、`crates/auction-storage/src/database/**/mapping.rs` |
| W0.9 | HTTP 指标：按路由模板的请求量、状态码与延迟，WebSocket 连接数，限流拒绝次数（按 login/mutation/realtime，W12.4 后加 read）；`/metrics` 改为拒绝经可信代理转发的请求（带转发头即 404），不再只依赖反向代理规则 | `crates/auction-http/src/http/shared/boundary.rs`、`crates/auction-http/src/http/metrics/mod.rs` |
| W0.10 | 指标采集路径：Worker 指标增加只监听 127.0.0.1 的 TCP 出口（默认关闭，独立于 control socket）；Public/Admin 若以 Unix socket 监听，同样提供 loopback 出口；指标存储采用本机 Prometheus（只监听 127.0.0.1，保留 15–30 天，经 SSH 隧道查看，决策 12）；同时采集 node_exporter 与 postgres_exporter（只监听 127.0.0.1，决策 42），覆盖磁盘、内存、`pg_stat_archiver`、`pg_wal` 大小、autovacuum、长事务与锁等待；W1/W12 的验收以此为准 | Worker control、`ops/README.md` |
| W0.11 | 重复错误的日志去重：同类错误在窗口内只记首条与计数，避免账号忙期间日志淹没其他信号 | `crates/auction-observability` |
| W0.12 | 只读核实计划依赖的事实：Sent 实际增速与 HV 是否自动清理、Read Mail 排序键、线上 `maximum_pages`/`fetch_timeout_seconds`、`trusted_proxy_cidrs` 是否配置为反向代理地址、站点在 Caddy 之前是否还有 CDN 代理（决策 43）、Public/Admin 的监听方式（TCP 或 Unix socket）、线上是否有指标存储、线上 systemd 版本对 `Requires=` 依赖在自动重启时的实际传播、已有论坛主题的最大页数、PostgreSQL 现行配置（`max_connections`、autovacuum、已装扩展）、主机内存与磁盘余量（容纳 Prometheus、exporter 与 WAL 暂存）；通知与告警相关（运维告警已确认未启用，决策 58）：每封邮件的邮资与近 30 天发信量，HV 邮件与 COD 的过期退回期限，HV 是否原样保存非 ASCII 正文（启用运维告警前核实中文摘要能否通过 Sent 精确证明），选择 PM 渠道而未绑定论坛账号的用户数与被抑制提醒数，论坛 `notify` 使用量，2026-11-01 换季时 isekai 仍未完成的义务数 | 线上只读查询与远端只读页面 |
| W0.13 | 用户可见 SLI/SLO 与容量目标：出价 p99 延迟、出价冲突中止率与耗尽次数、实时推送延迟（事件写入到推送）、注册/登录 key 邮件送达延迟、出价被超提醒送达时距 lot 有效截止的余量（D4 验收依据）、结算到首封与末封付款邮件寄出的延迟、买家付款到卖方货款寄出的延迟、投稿到收取或拒收回执的延迟、慢速代理从到期到执行的滞后；容量目标（Final 阶段峰值出价/秒、并发 WebSocket 数）供 W12 验收 | `crates/auction-observability`、`ops/README.md` |
| W0.14 | 就绪探针（D7，决策 32）：审计写入失败按 SQLSTATE 分类，可重试或瞬时的错误（40001、40P01、57014、55P03、08 类连接错误、取连接超时）只计入指标与日志，不再翻转 readiness；不可重试的失败（约束或触发器拒绝、42501 权限、42P01/42703 schema 漂移）仍使进程未就绪并进入告警。`/health/ready` 结果在进程内缓存 1–2 秒并合并并发探测，匿名请求不再逐次查库。须先于 W0.6 的超时上线 | `crates/auction-storage/src/database/audit_health/mod.rs`、`crates/auction-storage/src/database/store.rs`、`crates/auction-http/src/http/routes/health.rs` |
| W0.15 | 影子运行框架（M2a）：共享的新旧判定比对 helper、[交付规范](../architecture-improvement.md#5-每个工作项的交付规范)规定的 `auction_shadow_*` 指标与结构化差异日志、按分支统计样本；W1.4、W2.5、W3 复用，不各自实现 | `crates/auction-observability`、`crates/auction-application` |
| W0.16 | 浏览器错误信标（M3，决策 45，先于 W7.6）：Public/Admin 前端把 WASM panic、DTO 解码失败与实时协议错误按闭集类别和构建号上报到匿名、限流、无正文的信标接口，服务端只累加计数、不入库；同步[路由契约](../../contracts/routes.md) | `crates/auction-web-shared/src/bootstrap`、`crates/auction-http/src/http/routes` |

验收：`just verify` 通过；新指标可被采集并查看；错误日志不含秘密；D1 的拒绝分支、SQL 失败约束名与 HTTP 5xx 的原因可从日志直接读出；D5 的冲突率有基线数据；SLO 有当前值；W0.12 结论写入本计划基线；人为制造一次可重试的审计失败后进程仍就绪，不可重试的失败使进程未就绪；`/health/ready` 突发请求不随请求数线性增加数据库查询。回退：指标与日志均为附加输出，可经配置关闭；W0.14 的分类可回到“任一失败即未就绪”。
