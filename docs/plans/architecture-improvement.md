# 架构改进计划

[文档导航](../README.md) · [架构](../architecture.md) · [开发指南](../development.md) · [运维手册](../../ops/README.md)

版本 v4（2026-11-02）。本计划只处理代码与工程体系，不包含线上数据处置；线上问题在各阶段完成并经用户批准部署后自然受益。每完成一个工作项，在[进度](#8-进度)中更新状态。正文中的仓库路径均以仓库根为起点。

| 版本 | 变更 |
| --- | --- |
| v1 | W0–W8 初版 |
| v2 | 依最新源码复审：修正请求间隔基线；新增已知缺陷 D1/D2；W0 覆盖论坛与连接池；W1 增加 Read Mail 快速限界、论坛对齐与通知预算；W3 增加时区无关的行快照证明；W7 改用 CI 候选制品；新增 W9 安全加固、W10 数据生命周期与备份、W11 资产与资金对账 |
| v3 | 记录用户决策：提醒保持现状；批准领域测试与 PG18 集成测试并提前到 M2；告警只建通道入口；Admin 不限制暴露、不加第二因素；异机备份与数据清理按建议；main 作为发布基线；现线上即正式环境，不建预发 |
| v4 | 全仓审计后修正：更正连接池指标、请求间隔门、`maximum_pages` 默认值、通知出队顺序与错误丢弃范围等基线；新增 D3（Sent 页数悬崖）与 D4（出队优先级待定）；W0 扩展到存储/应用层错误、HTTP 指标、指标采集路径与待核实事实；W1 改为“三层争用 + 跨进程”调度并增加主动暂停与访问收口；W5 补告警来源与外部存活检查；W6 扩大测试范围与入口；W7 增加时段门禁、回滚边界与制品来源；W8 拆分部署形态决策与代码结构评估；W9/W10 补密钥、备份范围与保留政策前置；新增 W11.4 内部一致性巡检与 W12 竞价热路径；修正依赖倒置并重排里程碑 |

## 1. 目标与原则

目标：在不削弱现有资金安全与审计保证的前提下，降低外部依赖导致的故障面、减少“例外谓词”式维护、让人工处置以证据为中心，并建立可重复的发布、备份与安全基线。

原则：

1. [协作规范](../../AGENTS.md)的领域不变量全部保持：命令幂等、Serializable 关键事务、追加式事实、结果未知不重发、缓存不决定正确性、冻结策略不改写。
2. 每个工作项一个主题提交，按“领域 → 用例/端口 → 存储/外部适配 → HTTP/Web/Worker”落地，同步契约与运维文档。
3. 默认门禁仍为 `just verify`；行为测试、数据删除与线上部署都需要用户逐项批准（见[决策记录](#7-决策记录)）。
4. 先可观测、再改行为：每个改变运行行为的阶段，都先有能证明改进效果的指标，并且指标有地方存储和查看（W0.10）。
5. 无预发环境（决策 9），因此改变运行行为的工作项必须带配置开关与旧路径回退；涉及判定或计数口径的改动先影子运行（新旧并行、只记录差异），差异归零后再切换。
6. 每个工作项写明可度量的验收标准与回退方式；含迁移的版本不能靠换回旧程序回退（见 W7.2）。

## 2. 现状基线

| 项 | 基线 | 证据 |
| --- | --- | --- |
| 规模 | 约 21.5 万行 Rust；`auction-storage` 5.4 万行、`auction-application` 3.5 万行、`auction-domain` 0.5 万行 | 源码统计 |
| 迁移 | 74 个，三周内累积 | `migrations/postgres` |
| 测试 | 17 个测试函数，集中在论坛解析与少量领域状态；竞价/代理、排期日历、邮件解析器无测试；CI 有静态、审计、MSRV 与发布构建作业，无测试作业，也没有 `just test` 入口 | `.github/workflows/ci.yml`、`Justfile` |
| 外部账号：请求来源 | 同一游戏账号 Cookie 至少被四个独立限速器使用：Worker MoogleMail 客户端（代码默认 750 ms，线上 1000 ms，main/isekai 共用一个门）、Worker 装备来源读取（自带 250 ms 门）、Admin 进程手工录入的装备来源读取、CLI 的 `mail inspect-sent` 与 `funds` 证据读取 | `crates/auction-integrations/src/clients/mooglemail/client/security.rs`（`IntervalGate`）、`crates/auction-integrations/src/equipment/reader.rs:57`、`apps/auction-admin-api/src/bootstrap/state/database.rs:190`、`apps/auction-cli/src/operations/mail.rs:113`、`apps/auction-cli/src/operations/funds/evidence.rs:112` |
| 外部账号：进程内争用 | 三层串行点：发送/资产/内部证据的独占操作锁覆盖“Sent 基线扫描 → compose → 发送 → 发送后核对”全程；Read Mail/Inbox 列表扫描持 `incoming` 读锁，期间阻塞改变成员关系的 Inbox 详情读取；最后是 FIFO 请求门。每轮邮件轮询以 `tokio::join!` 并行执行观察、维护与收件，下一轮须等最慢分支结束；两个世界共用同一 cron，同时触发 | `crates/auction-integrations/src/clients/mooglemail/gateway/credentials.rs:244`、`crates/auction-integrations/src/clients/mooglemail/client/reads/lists.rs:195`、`crates/auction-jobs/src/worker/mail/poll/cycle.rs:95`、`apps/auction-worker/src/bootstrap/mail/mod.rs:51` |
| 外部账号：出队与节奏 | 投递出队故意让 `lot_watch`（出价被超提醒）排第一，迁移 059 为此建索引；已有按七类持久化的发送节奏 `[worker.mooglemail.send_intervals]`；`[worker.delivery] concurrency` 默认 8，但 MoogleMail transport 实际上限为 1 | `crates/auction-storage/src/database/delivery_scheduler/claims.rs:89`、`migrations/postgres/000059_delivery_send_cadence.up.sql`、`ops/config/examples/worker.toml` |
| 全量扫描 | 维护每 5 分钟完整扫描 Sent（约 37 页），Read Mail 观察完整扫描已读（约 34 页），仅 main 两类扫描每 5 分钟约 71 次请求，约占 1 req/s 门的 24%；Read Mail 扫描即使目标已知也不提前停止。`maximum_pages` 代码默认 20、硬上限 100，线上值待核实 | 线上日志：部署 `68c1b8df` 后维护每轮 120 秒超时；`crates/auction-config/src/config/settings/worker/mooglemail.rs:167` |
| Sent 基线 | 每次发送前要求把 Sent 完整扫到末页才能建立排除集（有缓存复用，但过期或换凭据即重扫）；客户端没有删除能力。若 HV 不自动清理 Sent，列表超过 `maximum_pages` 后所有发送失败关闭 | `crates/auction-integrations/src/clients/mooglemail/sender/proof.rs:40` |
| 账号忙 | 战斗页按请求逐个失败，每次保存诊断；诊断目录硬上限 1024 文件；人也会用该账号战斗或手工发信，没有主动暂停手段 | 2026-11-02 约 2000 次/小时 `account_busy`，旧诊断被全部挤出 |
| 通知量 | 出价被超提醒按 lot、用户与轮次各发一封 MoogleMail，无按收件人合并或限额；失去资格的提醒成批预检退役 | `crates/auction-application/src/services/public/watch.rs`；线上 110 条预检退役 |
| 错误可观测 | jobs 层 43 处、应用层 25 处、集成层 22 处丢弃错误详情（`Err(_error)`/`Err(_) =>`）；存储层 1430 处 `map_err(\|_\| …)` 把 SQLSTATE 与约束名压成固定中文串；`ApplicationError` 没有安全分类；D1 所在的实收资金化把三种不同原因记成同一个 defer reason | `crates/auction-jobs/src/worker`、`crates/auction-application/src/services/settlement/fulfillment/collected.rs:49` |
| 指标 | 已有业务事件、Worker 循环、重试、调度延迟、连接池（占用/空闲/等待/取消）与 tracing 指标；缺 HTTP 按路由的请求量/状态/延迟、WebSocket 连接数、限流拒绝、序列化冲突与外部账号指标。Worker `/metrics` 只在 Unix socket 上提供，Prometheus 不能直接抓取；仓库未体现线上指标存储 | `crates/auction-observability/src/metrics`、`apps/auction-worker/README.md` |
| 数据库连接 | 每进程固定 16 个连接，不可配置；服务连接没有 `statement_timeout`、`idle_in_transaction_session_timeout` 与 `application_name`；竞价 Serializable 事务冲突后立即重试 3 次，无退避抖动、无冲突指标 | `crates/auction-storage/src/database/postgres_store.rs:17`、`crates/auction-storage/src/database/competition/postgres.rs:89`、`crates/auction-storage/src/database/backend.rs:64` |
| 规则位置 | 资格与状态机散落在存储层：JSON `Value` 字段比较、SQL 推进、数据库函数（迁移 073/074）；早鸟期与排期日历的纯规则位于应用层 ports | `crates/auction-storage/src/database/sent_confirmation/snapshot.rs`、`crates/auction-storage/src/database/recovery_scheduler`、`crates/auction-storage/src/database/settlement_funding/receipt.rs`、`crates/auction-application/src/ports/scheduling/early_bird.rs` |
| 时区耦合 | legacy 自寄与装备损失证明用 `to_jsonb(行)` 与快照整行比较（含时间戳），只因 sqlx 连接固定 `TimeZone=UTC` 才成立；其他会话时区下结论翻转 | `crates/auction-storage/src/database/legacy_self_proof/mod.rs`、`crates/auction-storage/src/database/equipment_loss/proof.rs` |
| 处置语义 | legacy 自寄、装备损失、Admin 判失败、预检退役、人工 Sent 证明各有专用表或谓词，仪表盘/告警/列表分别拼排除条件 | `crates/auction-storage/src/database/operations_alert/snapshot/reads.rs` |
| 资产与资金 | 没有定期核对 bot 应持装备与 Armory 实际库存；仅记账模式下没有余额漂移检测；没有投影与事实的内部一致性巡检 | 无对应端口；`FundingPolicy::observes_cash` |
| 告警 | 告警来源只有 Process、Delivery、Fulfillment、Recovery 四类，缺收件停滞、凭据失效、生命周期逾期、论坛、磁盘与备份；告警由 Worker 经同一游戏账号的 MoogleMail 发送，账号忙或 Worker 崩溃循环时静默；无外部存活检查 | `crates/auction-application/src/ports/operations/alert/signals.rs:8`、`[worker.operations_alert]` |
| 调度依赖 | 慢速代理到期与进入 Final 由 Worker 执行；Worker 在截止前停机时代理可能来不及反超 | `crates/auction-jobs/src/worker/auction/slow_proxy.rs` |
| 安全暴露 | Caddy 模板把 Admin 作为独立公网站点代理（决策 4 保持）；Admin 为单一共享密钥登录，无登录失败指标与告警；应用与 Caddy 都未设 HSTS，CSP 缺 `form-action`；限流依赖 `trusted_proxy_cidrs` 正确配置，`config check` 不提示缺失 | `ops/deploy/caddy/Caddyfile`、`crates/auction-http/src/http/security/headers.rs`、`ops/README.md` |
| 密钥 | 每类数据密钥只有单个 key ID，无密钥环与重加密流程；邮件诊断归档复用 `credential_data_key`（缺失时回退装备密钥），与“三组密钥不得复用”冲突 | `auction-config` secrets 模型；`apps/auction-worker/src/bootstrap/mail/diagnostics.rs:25` |
| 备份与保留 | 只有手动 `backup create`，范围仅数据库，不含 runtime 配置、诊断证据与密钥托管；未定义 RPO/RTO；仓库无定时任务模板；[保留政策](../../ops/deploy/retention-policy.v1.json)对 realtime、会话等数据族为 `retain_forever`，另有 12 项 `required_operator_decisions` 未决 | `ops/deploy/systemd`、`ops/deploy/RECOVERY.md`、`apps/auction-cli/README.md` |
| 发布 | CI 只在默认分支推送与手动触发时构建 `release-candidate-<sha>`，正式制品保留 14 天，无 tag 触发与来源证明；部署仍在本地重建并手工多步执行；部署状态写在服务器 README；readiness 要求迁移历史与二进制完全一致，旧程序无法在新 schema 上就绪；仓库无 `main`，远端默认分支为 `refactor/dioxus-frontend`，本地 `master` 已过时 | `.github/workflows/ci.yml`、`crates/auction-storage/src/database/store.rs:204` |
| 模块结构 | 401 处 `#[path]` 让模块树与目录脱钩，333 处 `use super::*`，`auction-application` 平铺导出 838 个符号；存在 947 行的 `crates/auction-storage/src/database/mailbox_sweep/mod.rs` 等大文件 | 源码统计 |

## 3. 工作流总览

| 编号 | 工作流 | 优先级 | 依赖 | 迁移 | 估算 |
| --- | --- | --- | --- | --- | --- |
| W0 | 错误分类、指标与采集路径、已知缺陷诊断、待核实事实 | P0 | — | 否 | 6–8 天 |
| W1 | 外部账号调度：快速限界、熔断、三层争用统一调度、访问收口、主动暂停、增量扫描、证据退避、论坛对齐 | P0 | W0 | 是（暂停开关、扫描水位） | 12–16 天 |
| W2 | 统一处置事实模型 | P1 | W0、W3.7 | 是 | 6–8 天 |
| W3 | 规则上移：类型化快照、纯函数判定、时区无关证明、排期规则下沉 | P1 | W2 的领域类型；W6.2 | 否 | 10–14 天，分批 |
| W4 | 证据优先的 Admin 运维台 | P1 | W2、W1.4 | 视实现 | 6–8 天 |
| W5 | 告警通道入口、告警来源补齐、外部存活检查、仪表盘口径统一 | P1 | W0、W2 | 否 | 4–6 天 |
| W6 | 验证体系（W6.1、W6.2 已批准） | P1 | — | 否 | 8–12 天 |
| W7 | 发布流水线与环境 | P2（W7.5 为 P0） | W7.2 依赖 W8.1 | 否 | 5–7 天 |
| W8 | 规模收敛评估（只出决策记录） | P2 | — | 否 | 2–3 天 |
| W9 | 安全加固：密钥环、已知取舍复核、Admin 补偿控制、主机加固 | P2 | — | 是（密钥环） | 4–5 天 |
| W10 | 数据生命周期与备份：RPO/RTO、定时备份、异机副本、备份范围、恢复演练、按政策清理 | P1 | — | 视政策 | 6–8 天 |
| W11 | 资产、资金对账与内部一致性巡检 | P1 | W1.3、W5 | 是（报告事实） | 5–6 天 |
| W12 | 竞价热路径与读放大 | P2 | W0.6、W0.9 | 否 | 3–5 天 |

估算为单人连续工作日，含文档同步与静态门禁，不含线上部署与观察；合计约 77–106 天。

### 已知缺陷

| 编号 | 缺陷 | 处理 |
| --- | --- | --- |
| D1 | Admin 失败更正后的 COD 实收资金化每轮返回未持久化错误，付款义务停在 `enqueued`，卖方货款保持阻塞；数据库无报错，属于 Rust 侧拒绝，具体原因被应用层 `Err(_)` 丢弃 | W0.7：先让错误可分类，再定位修复；修复前保持人工证明事实不变 |
| D2 | `68c1b8df` 让 Read Mail 观察真实翻页后，与 Sent 维护共享预算并持有 `incoming` 读锁，维护每轮超时，Sent 找回、草稿清理与目击停摆 | W1.0 快速限界；W1.3、W1.4 根治 |
| D3 | 潜在：发送前 Sent 完整基线受 `maximum_pages ≤ 100` 限制，Sent 若只增不减，超过上限后所有发送失败关闭 | W0.12 核实增速与 HV 清理行为；W1.4 改为 MID 水位前缀扫描；若核实结果显示临近上限则提前单独交付 |
| D4 | 待定：出队让出价被超提醒优先，与 W1.7“通知最低优先级”的方向相反 | 等待用户决定（见[决策记录](#7-决策记录)），决定前不改出队顺序 |

## 4. 分阶段方案

### W0 错误分类、指标与已知缺陷诊断

| 项 | 内容 | 涉及位置 |
| --- | --- | --- |
| W0.1 | 为 `ApplicationError` 增加 `safe_kind() -> &'static str`，闭集分类（如 `funding_unproven`、`evidence_expired`、`repository_conflict`、`remote_busy`），不含秘密与正文 | `crates/auction-application/src/errors` |
| W0.2 | jobs 层 43 处、应用层 25 处、集成层 22 处丢弃点改为记录 `safe_kind` 与业务标识（obligation/delivery/world），保持原控制流不变；同一函数内不同原因必须映射到不同 defer reason | `crates/auction-jobs/src/worker`、`crates/auction-application/src/services`、`crates/auction-integrations/src` |
| W0.3 | Worker 指标：`worker_unpersisted_error_total{loop,kind}` | `crates/auction-observability` |
| W0.4 | MoogleMail 指标：按 world/operation/类别统计请求数、请求门等待、独占操作锁与 `incoming` 锁等待、`account_busy`/限流/超时次数、维护与观察耗时及预算耗尽次数 | MoogleMail 客户端、`crates/auction-jobs/src/worker/mail/poll/cycle.rs` |
| W0.5 | 论坛客户端同口径指标（主题扫描页数、写入未知、PM） | `crates/auction-integrations/src/clients/ehentai` |
| W0.6 | 数据库：连接池已有指标保留；新增连接数配置、服务连接的 `statement_timeout`/`idle_in_transaction_session_timeout` 与按角色的 `application_name`；按事务 owner 统计序列化冲突与重试耗尽 | `crates/auction-storage/src/database/postgres_store.rs`、`crates/auction-storage/src/database/backend.rs` |
| W0.7 | 用 W0.1 的分类定位 D1：资金化校验各拒绝分支映射到独立 `safe_kind` | `crates/auction-application/src/services/settlement/fulfillment/collected.rs`、`crates/auction-storage/src/database/settlement_funding/receipt.rs` |
| W0.8 | 存储层 SQL 错误安全上下文：保留 SQLSTATE 类别、约束名与表名（schema 标识，非秘密），不保留 `detail`/`message`（可能含值）；先覆盖资金、投递与生命周期 owner | `crates/auction-storage/src/database/**/mapping.rs` |
| W0.9 | HTTP 指标：按路由模板的请求量、状态码与延迟，WebSocket 连接数，限流拒绝次数（按 login/mutation/realtime） | `crates/auction-http/src/http/shared/boundary.rs` |
| W0.10 | 指标采集路径：Worker 指标提供可被抓取的出口（loopback TCP 或由 Admin 只读代理），并确定指标存储与查看方式；W1/W12 的验收以此为准 | Worker control、`ops/README.md` |
| W0.11 | 重复错误的日志去重：同类错误在窗口内只记首条与计数，避免账号忙期间日志淹没其他信号 | `crates/auction-observability` |
| W0.12 | 只读核实计划依赖的事实：Sent 实际增速与 HV 是否自动清理、Read Mail 排序键、线上 `maximum_pages`/`fetch_timeout_seconds`、`trusted_proxy_cidrs` 是否配置为反向代理地址、线上是否有指标存储 | 线上只读查询与远端只读页面 |

验收：`just verify` 通过；新指标可被采集并查看；错误日志不含秘密；D1 的拒绝分支与 SQL 失败约束名可从日志直接读出；W0.12 结论写入本计划基线。

### W1 外部账号调度

| 项 | 内容 | 关键约束 |
| --- | --- | --- |
| W1.0 快速限界 | Read Mail 观察改为有界页数或降频；维护从每轮 `tokio::join!` 中拆出为独立循环与独立节奏，不再拖慢下一轮收件；main 与 isekai 错峰触发；先确认共享 reader 对完整已读列表的依赖 | 只减少读取；不改变任何证明或业务结果 |
| W1.1 账号忙熔断 | 识别战斗页/冻结后按账号记录 `busy_until`，退避 1→2→5→10 分钟，期间只放行一次探测，其余请求不发网络直接返回 `AccountBusy` | 熔断只在内存限流，不决定业务状态（不变量 5）；写入类在 sending 围栏前被拒 |
| W1.2 诊断去重 | 同一忙碌期只保存首份战斗页；诊断淘汰按类别优先保留写入未知证据 | 不降低 write_unknown 证据保全 |
| W1.3 统一账号调度 | 把三层串行点作为一个调度问题设计：出队优先级（与已有 `send_intervals` 七类节奏合并为一套）、独占操作锁（长扫描不得在持锁期间进行，或按优先级排队）、`incoming` 锁（长列表扫描分段释放）、请求门（分级令牌桶替换 FIFO `IntervalGate`，低优先级保底份额） | 总速率不超过配置间隔；同账号远端写入串行；优先级只影响先后，不影响结果 |
| W1.4 增量扫描 | Sent 与 Read Mail 持久化“已完整扫描到的最高 MID + 时间”水位，常规轮按 MID 前缀扫描新增部分而不是按页号；发送前基线同样改为“发送前最高 MID + 前缀扫描”，消除 100 页悬崖（D3）；Read Mail 水位以 W0.12 确认的排序键为前提，目标已知时找齐即停；每日一次完整校验扫描，超过上限时只告警不阻断发送 | 水位不作为发送或收取证明；缺失或不连续回退完整扫描；影子运行期间新旧结果并行比对 |
| W1.5 证据退避 | 履约证据按年龄指数退避（每分钟逐步放宽到 30 分钟），付款截止前后提高频率 | 不改变付款期限与恢复策略 |
| W1.6 论坛对齐 | 论坛客户端接入同一调度抽象：优先级、写入未知、熔断与指标 | 论坛写入仍按既有未知结果流程，不自动重发 |
| W1.7 通知优先级 | 出价被超提醒保持现有发送规则（不合并、不限额，决策 1）；出队位置待 D4 决定后在 W1.3 中实现 | 不改变玩家可见内容 |
| W1.8 配置 | 预算、退避、暂停与通知参数进入 `worker.toml`，带中文“可选/必填”注释并同步[配置参考](../../ops/config/README.md)；清理无实际效果的配置（如 `[worker.delivery] concurrency` 对 MoogleMail 的有效上限为 1） | 默认值不超过当前上限；删除字段按配置兼容规则处理 |
| W1.9 主动暂停 | 运维可经 Admin 或 CLI 设置带到期时间的“暂停外部访问”标记（持久化），Worker、Admin 装备读取与 CLI 远端读取都遵守；用于人工使用账号、事故处置与恢复后的安全启动（[备份与恢复](../../ops/deploy/RECOVERY.md#6-外部历史核对与启动)） | 暂停只阻止新请求，不中断已进入 sending 围栏的写入；到期自动恢复并留审计 |
| W1.10 访问收口 | Admin 手工录入的装备读取与 CLI 远端读取改为经 Worker 代为执行（control 或持久工作项），或改用基于 PostgreSQL 的跨进程限速；Worker 自身的装备来源读取并入 W1.3 的请求门 | 不在 Admin/CLI 进程持有第二个限速器；Worker 不可用时明确返回能力不可用 |

验收（以 W0.10 采集的指标度量）：稳态外部请求不超过请求门容量（配置间隔对应的每秒请求数）的 30%；维护 p95 低于 60 秒且无预算耗尽；收件轮询间隔不受维护耗时影响；模拟战斗期每个退避间隔不超过 1 次探测、1 份诊断；Sent 页数超过 `maximum_pages` 时发送仍可完成。

### W2 统一处置事实模型

| 项 | 内容 |
| --- | --- |
| W2.1 领域 | `DeliveryDisposition` 闭集：`auto_sent_proof`、`operator_sent_confirmation`、`admin_failed`、`equipment_loss`、`legacy_self`、`preflight_retired`、`operator_acknowledged`；每类定义准入事实，并明确闭集覆盖“全部终态”还是“仅异常处置”，避免与正常成功路径重复计数 |
| W2.2 迁移 | 只追加表 `delivery_dispositions`（delivery、fence、kind、来源引用、操作者、原因、时间），追加触发器；来源引用按 kind 使用各自的可空外键并以 CHECK 约束“恰好一个非空”；表由单一应用端口拥有，其他用例经该端口写入；同步保留政策与[数据库契约](../contracts/database.md) |
| W2.3 回填 | 迁移内只回填能由既有事实精确证明的行，其余保持未处置；回填比较使用 W3.7 的时区无关形式 |
| W2.4 写入 | 各写者在原事务内经 W2.2 的端口追加处置：Admin 对账、装备损失、`confirm-sent`、提醒预检退役、legacy 修复 |
| W2.5 读侧 | 仪表盘、告警、Admin 列表改用“未处置”视图，先影子比对再删除 `reads.rs` 中的专用排除；新增“确认已处置”命令处理无业务影响的历史失败，领域层限定只适用于没有资金或资产影响的条目，必须填写原因 |
| W2.6 履约 | 履约终态同理（如装备损失后的线下补偿） |

验收：迁移前后各面板计数与当前排除口径逐项一致（影子期差异为零）；读侧不再含专用排除谓词。

### W3 规则上移

统一模式：

```text
storage：按锁序加载 → 类型化快照（结构体，而非 JSON Value）
application/domain：evaluate(snapshot) → 允许 / 阻断项闭集
storage：以快照版本做 CAS 写入；数据库函数保留为最后一道约束
```

权威划分：业务判定以 Rust 为准，数据库函数只保留不可绕过的最低约束；两边规则变化须在同一提交中同步，并以同一组样例证明结论一致。

| 批次 | 目标 | 现位置 |
| --- | --- | --- |
| W3.1 | 人工 Sent 证明准入（两种准入、时间窗、资金占用） | `crates/auction-storage/src/database/sent_confirmation/snapshot.rs` |
| W3.2 | 装备损失处置准入 | `crates/auction-storage/src/database/equipment_loss/snapshot.rs` |
| W3.3 | 恢复阶段推进收敛为领域状态机 | `crates/auction-storage/src/database/recovery_scheduler/scheduler/advance.rs` |
| W3.4 | Admin 投递动作守卫（目击、退役、迁移表） | `crates/auction-storage/src/database/delivery_admin/commands.rs` |
| W3.5 | COD 实收资金化校验（结合 D1 结论） | `crates/auction-storage/src/database/settlement_funding/receipt.rs` |
| W3.6 | 履约 claim 一致性校验 | `crates/auction-storage/src/database/fulfillment_scheduler/claims.rs` |
| W3.7 | 行快照证明改为时区无关：显式列比较或 UTC 规范化时间，不依赖会话 `TimeZone`（提前到 M3，先于 W2） | `crates/auction-storage/src/database/legacy_self_proof/mod.rs`、`crates/auction-storage/src/database/equipment_loss/proof.rs` |
| W3.8 | 排期日历纯规则（早鸟期、赛季前场次、夏令时换算）从应用层 ports 下沉到领域 | `crates/auction-application/src/ports/scheduling/early_bird.rs`、`crates/auction-application/src/ports/scheduling/calendar.rs` |

每批验收：业务规则不再以 JSON 下标表达；Rust 判定与数据库函数在同一组样例上结论一致（以 W6.2 集成测试固化，切换前影子比对）；W3.7 在非 UTC 会话下结论不变；W3.8 的已冻结场次排期结果逐场不变。

### W4 证据优先的 Admin 运维台

| 项 | 内容 |
| --- | --- |
| W4.1 | 投递详情 DTO：尝试历史、冻结期望摘要、Sent 观察与目击、领取状态、处置、关联义务与恢复 |
| W4.2 | 复用维护已写入的 `mailbox_sweep` 观察与 `sent_reference_sighting`，把 Sent 详情观察升级为可引用证据（摘要与回执，不含登录 key 正文） |
| W4.3 | Admin 发起人工 Sent 证明、Admin 失败更正、确认已处置：选择已采集证据 + 原因 + 稳定命令 ID，复用同一应用端口 |
| W4.4 | 对账对话框展示目击与候选；判失败保持闭集原因 |

同步[路由契约](../contracts/routes.md)、五语言译文与前端 `Route`。验收：已领取货款、误判失败、结果未知通知、退件重试四类处置均可在 Admin 内闭环。

### W5 告警通道、来源与仪表盘口径

| 项 | 内容 |
| --- | --- |
| W5.1 | 运维告警抽象出通道端口与配置入口，与 MoogleMail 通道并存、可单独启用；具体外部通道（Webhook 等）待用户另定后再接入（决策 3） |
| W5.2 | 仪表盘状态复用告警策略的阈值评估，消除“>0 即需处理”与告警阈值两套口径 |
| W5.3 | 新增“外部账号”分区：忙碌与暂停状态及持续时间、请求门使用率、维护健康、论坛状态 |
| W5.4 | 告警正文附运维手册锚点；W10、W11 落地时各自注册备份与对账告警来源 |
| W5.5 | 补齐告警来源：收件轮询停滞、凭据失效、生命周期逾期（应进入 Final/结算而未进入）、论坛同步与私信失败、磁盘与备份目录容量 |
| W5.6 | 外部存活检查：提供只读心跳出口，由服务器外部的检查方在 Worker 心跳超时或进程崩溃循环时报警；检查方与 W5.1 的外部通道一并选定 |

验收：模拟账号忙、Cookie 失效、Worker 停止三种场景，各在阈值时间内经至少一条不依赖该故障的路径产生告警（外部通道选定前，以仪表盘与心跳出口状态为准）。

### W6 验证体系（W6.1、W6.2 已批准）

| 项 | 内容 |
| --- | --- |
| W6.1 | 纯逻辑测试：结算守恒、持有曲线插值、比例分配、冲正；竞价增量 v1/v2、代理竞争与上限出价、软截止；排期日历（夏令时、早鸟期、赛季前场次）；MoogleMail 与论坛解析器（合成样例覆盖收件人 15 字符截断、标题 7 位 hex、分钟精度时间、战斗页、登录页）；前端纯逻辑（实时事件合并与缺口检测、URL 查询状态）；W1.3 调度算法（tokio 暂停时间） |
| W6.2 | PostgreSQL 18 集成测试骨架：测试夹具用本机 PG18 二进制建临时集群，每个测试从模板库克隆独立库；以独立 feature 控制，`just verify` 只编译不运行；覆盖竞价（含并发序列化冲突）、结算、人工 Sent 证明、恢复推进、仪表盘计数、非 UTC 会话下的证明 |
| W6.3（待定） | 脱敏的真实页面回放语料（Inbox/Read Mail/Sent/详情/战斗页/截断列表/论坛页） |
| W6.4（待定） | CI 增加独立测试作业，不并入默认 `just verify` |
| W6.5 | 测试入口与规范：新增 `just test`（纯逻辑）与 `just test-db`（PG18 集成）；同步更新 `AGENTS.md` 验证范围、`CONTRIBUTING.md` 与[开发指南](../development.md#测试)，写明已批准的测试类别与维护要求 |

验收：W6.1 覆盖上述每类至少一组正反例并全部通过；W6.2 在本机可一条命令运行并清理临时集群；规范文档与 Justfile 入口一致。

### W7 发布流水线与环境

| 项 | 内容 |
| --- | --- |
| W7.1 | 分支与版本：以当前开发线建立 `main` 并设为远端默认分支，开启分支保护（必须通过 CI）；此后只从 `main` 上的 tag 发布（决策 8）：CI 增加 tag 触发，制品附到 GitHub Release 长期保留并生成来源证明；CI 增加“已发布迁移文件未被修改”检查；部署只使用 CI 制品并核对其提交可从发布 tag 到达 |
| W7.2 | `auction-cli deploy <制品目录>`：在现有 `service restart`（严格配置检查、停机、迁移、按序启动、readiness）之上增加在途发送门禁、W7.5 时段门禁、备份并新旧 CLI 校验、保留旧程序、静态资源核对与回执；部署形态范围以 W8.1 的决定为准。配套 `auction-cli rollback` 只用于不含迁移的版本；含迁移的版本回退只能按[备份与恢复](../../ops/deploy/RECOVERY.md)恢复并做外部历史核对 |
| W7.3 | 部署回执写入状态目录并可查询，取代手改服务器 README |
| W7.4（取消） | 现线上即正式环境，不建预发、不更名（决策 9）；行为验证使用本地 PostgreSQL 18 与 W6.2 |
| W7.5 | 时段门禁（提前到 M1）：`service stop/restart` 在任一场次处于 Final、存在即将到期的慢速代理或 lot 有效截止临近时拒绝执行，需显式 `--force` 并写明原因；同步[运维手册](../../ops/README.md) |

验收：从 tag 到上线一条命令完成并产生回执；含迁移版本的 `rollback` 被拒绝并给出恢复指引；Final 阶段的停机请求被拒绝。

### W8 规模收敛评估

输出决策记录，不直接实施。

| 项 | 内容 |
| --- | --- |
| W8.1 | 部署形态：是否只保留合并部署、是否移除可选 Redis、首次设置服务与停用世界（isekai）代码路径的去留；须在 W7.2 实现前完成 |
| W8.2 | 代码结构：`#[path]` 与目录对齐、`use super::*` 收敛、`auction-application` 平铺导出改为按上下文分组、大文件（如 `crates/auction-storage/src/database/mailbox_sweep/mod.rs`）拆分、按限界上下文拆 crate 的编译收益；旧兼容层清单（`crates/auction-application/src/services/auction/competition/legacy_ledger.rs`、legacy 自寄、v1 竞价策略、迁移执行视图）及各自的移除条件 |

### W9 安全加固

| 项 | 内容 |
| --- | --- |
| W9.1（取消） | Admin 暴露面保持现状，不加来源限制（决策 4） |
| W9.2（取消） | 不加第二因素（决策 5） |
| W9.3 | 数据密钥环：多 key ID 并存解密、新写入用当前 key、离线重加密命令与回执；邮件诊断归档改用独立密钥域；旧密钥保留期不短于最长备份保留期（W10.2 为 12 个月）；凭据 Cookie 轮换流程文档化 |
| W9.4 | 复核 CSP `unsafe-eval` 等已知取舍并记录接受理由；增加 HSTS 与 `form-action 'self'` |
| W9.5 | Admin 补偿控制（不改变决策 4、5）：登录失败计数指标与告警、`admin_secret` 轮换流程；`config check --strict` 在生产形态（`cookie_secure = true`、loopback 监听）而未配置可信代理时报警 |
| W9.6 | 主机加固：systemd 模板补资源上限（MemoryMax、TasksMax）与可用的沙箱选项，用 `systemd-analyze security` 记录评分 |

### W10 数据生命周期与备份

| 项 | 内容 |
| --- | --- |
| W10.0 | 确定 RPO/RTO 与演练频率；评估 WAL 归档（日备份意味着最多丢失一天的竞价与资金事实）；结论写入保留政策的对应决策项 |
| W10.1 | 定时备份：systemd timer 模板 + `auction-cli backup create/verify`，回执与失败告警；本地保留份数与磁盘水位 |
| W10.2 | 异机副本：加密后上传 Cloudflare R2，保留 30 份日备份与 12 份月备份，副本校验（决策 6）；备份加密密钥离机托管；服务器上的 R2 凭据只能写入不能删除，或启用 bucket lock；R2 凭据进入敏感 TOML 并纳入 `config check` |
| W10.3 | 定期恢复演练：在隔离实例执行 `db rebuild --ephemeral`/恢复预检并出回执；演练前确认磁盘余量 |
| W10.4 | 按政策只清理运行态数据族（会话、心跳、realtime 事件、诊断等）；审计与资金事实永不删除；每次删除先只读演练并经用户批准（决策 7）。前置：发布保留政策 v2（现行 v1 对 realtime、会话为 `retain_forever`，并有未决的 `required_operator_decisions`）；在[实时事件契约](../contracts/realtime.md)中定义回放保留期限与过期游标行为 |
| W10.5 | 备份范围：除数据库外，纳入 runtime 配置、加密邮件诊断证据与部署回执；数据密钥与备份密钥的离机托管流程写入[备份与恢复](../../ops/deploy/RECOVERY.md) |

### W11 资产、资金对账与内部一致性巡检

| 项 | 内容 |
| --- | --- |
| W11.1 | 装备持有核对：由数据库推导 bot 应持装备（待退回、草稿、未寄出等），与 Armory 实际库存比对，输出差异报告 |
| W11.2 | 余额漂移报告：仅记账模式下按冻结账目推导预期变化，与只读余额观察比对，只报告不改账 |
| W11.3 | 报告以追加事实保存，差异进入仪表盘与告警（W5） |
| W11.4 | 内部一致性巡检：只读 SQL 不变量（lot 投影与出价事实、补偿池余额与流水、义务链完整性、投递与义务状态对应），结果同样以追加事实保存 |

约束：只读外部观察，按 W1 的最低优先级执行；报告不授权任何资金或资产动作。

### W12 竞价热路径与读放大

| 项 | 内容 |
| --- | --- |
| W12.1 | 以 W0.6 指标确认序列化冲突率后，竞价与独立 Serializable 写事务的重试增加带抖动的退避，重试预算不变 |
| W12.2 | 以 W0.9 指标度量 Final 阶段每次出价触发的页面重读量；必要时在前端增加短合并窗口，仍以权威接口为准，不从事件负载推导结果 |
| W12.3（待定） | 本地 PG18 负载演练：模拟 Final 阶段并发出价，需用户批准 |

## 5. 推进顺序与里程碑

| 里程碑 | 内容 | 完成标志 |
| --- | --- | --- |
| M1 | W0、W1.0、W7.5 | 错误、外部请求与 HTTP 可度量且可采集；D1 原因可读；维护不再超时；待核实事实写入基线；Final 阶段无法误停机 |
| M2 | W1.1–W1.3、W1.6、W1.9、W1.10、W6.1、W6.2、W6.5 | 熔断、统一调度与主动暂停生效，指标达标；Admin/CLI 不再直连账号；测试入口与规范可用 |
| M3 | W3.7、W1.4–W1.8、W2 | 时区无关证明先行；增量扫描消除 Sent 悬崖；处置模型落地，仪表盘无专用排除 |
| M4 | W5、W3.1–W3.3、W3.8、W4 | 告警来源齐全、外部存活检查入口就绪；运维台闭环，核心准入上移 |
| M5 | W3 其余、W10、W11、W9.3–W9.6、W8.1 | 备份与对账自动化，密钥环完成，部署形态决定 |
| M6 | W6.3–W6.4（如确认）、W7.1–W7.3、W8.2、W12 | 发布流水线可用，结构评估完成，热路径按指标调整 |

每个里程碑结束时停下来：汇报变更、门禁结果与部署建议，由用户决定是否部署。D1 的修复一经定位即单独交付；D3 若经 W0.12 核实临近上限，W1.4 的发送基线部分提前单独交付。两者都不等待里程碑。

## 6. 每个工作项的交付规范

1. 开始前 `git status --short`，确认基线分支，保留与任务无关的改动。
2. 先写清本项保护的不变量与契约变化（路由、JSON、事件、配置、数据库），以及验收标准与回退方式。
3. 改变运行行为的项带配置开关并保留旧路径；判定或计数口径变化先影子运行，记录差异，差异归零后再切换并在后续版本删除旧路径。
4. 实现后同步契约文档、模块 README、运维文档与配置示例；文档中的仓库路径以仓库根为起点。
5. `cargo fmt --all` 与 `just verify` 通过；涉及数据库的判断在本地 PostgreSQL 18 只读预览核对；已批准的测试（W6）随功能一起更新。
6. 提交标题 `type(scope): summary`，正文写不变量与验证范围；一个主题一个提交。
7. 在[进度](#8-进度)更新状态与提交号。

## 7. 决策记录

2026-11-02 用户决策：

| # | 事项 | 决定 |
| --- | --- | --- |
| 1 | 出价被超提醒合并与限额 | 保持现状，只调整调度优先级（W1.7） |
| 2 | 行为测试 | 批准领域测试与 PG18 集成测试（W6.1、W6.2）；回放语料与 CI 测试作业待定 |
| 3 | 告警外部通道 | 先保留通道入口，具体通知方式后定（W5.1） |
| 4 | Admin 暴露方式 | 不限制（取消 W9.1） |
| 5 | Admin 第二因素 | 不加（取消 W9.2） |
| 6 | 异机备份 | 加密上传 R2，30 份日备份 + 12 份月备份（W10.2） |
| 7 | 数据清理 | 只清理运行态数据；审计与资金事实永不删除；每次先演练并经批准（W10.4） |
| 8 | 分支与发布基线 | 建立 `main` 作为发布基线，只从 `main` 的 tag 发布（W7.1） |
| 9 | 预发与更名 | 现线上即正式环境，不建预发、不更名（取消 W7.4） |
| 10 | 规模收敛 | 只出决策记录，评估后再定（W8） |

仍待决定：

| 事项 | 说明 |
| --- | --- |
| D4 出队优先级 | 现有出队让出价被超提醒优先（截止后才送达的提醒没有价值）；决策 1 的“只调整调度优先级”是否意味着把它降为最低，或仅在请求门层面降级、保留出队优先 |
| W0.10 指标存储 | 本机 Prometheus 等单机方案，或只在 Admin 仪表盘展示关键统计 |
| W5.1/W5.6 外部通道与存活检查 | 通知方式与服务器外部的检查方 |
| W6.3、W6.4 | 回放语料与 CI 测试作业 |
| W8 | 评估后的实施与否 |
| W10.0 | RPO/RTO、演练频率与是否启用 WAL 归档 |
| W10.2 | 备份加密工具与密钥托管位置 |
| W10.4 | 保留政策 v2 各数据族期限 |
| W12.3 | 是否做负载演练 |

线上运维事项（不属于本计划）：`fetch_timeout_seconds` 维持现状；xilixilixi 邮件为日常交流、与拍卖无关，不重试，待 W2.5 的“确认已处置”收口。

## 8. 进度

| 编号 | 状态 | 提交 | 备注 |
| --- | --- | --- | --- |
| D1 | 待诊断 | — | 依赖 W0.7 |
| D2 | 待处理 | — | W1.0；线上已部署 `92259475`，未包含修复 |
| D3 | 待核实 | — | W0.12 核实后决定是否提前交付 W1.4 发送基线 |
| D4 | 待决定 | — | 见决策记录 |
| W0 | 未开始 | — | — |
| W1 | 未开始 | — | — |
| W2 | 未开始 | — | — |
| W3 | 未开始 | — | W3.7 随 M3 先行 |
| W4 | 未开始 | — | — |
| W5 | 未开始 | — | — |
| W6 | W6.1、W6.2 已批准 | — | M2 执行，含 W6.5 |
| W7 | 未开始 | — | W7.5 随 M1 先行 |
| W8 | 未开始 | — | W8.1 在 M5，先于 W7.2 |
| W9 | 未开始 | — | — |
| W10 | 未开始 | — | — |
| W11 | 未开始 | — | — |
| W12 | 未开始 | — | — |

## 9. 术语

| 术语 | 含义 |
| --- | --- |
| sending 围栏 | 外部写入前持久化的发送状态与 fencing 版本；越过后结果未知只能人工核对，不自动重发 |
| 目击（sighting） | 维护在 Sent 列表中观察到的投递引用，用于找回或核对发送结果 |
| legacy 自寄 | 旧实例遗留的发给 bot 自身的投递，需要专门证明后处置 |
| 预检退役 | 发送前预检发现提醒已失去资格，直接退役不发送 |
| 水位 | 已完整扫描到的最高远端邮件 ID；只用于缩小扫描范围，不作为发送或收取证明 |
| 独占操作锁 | 同一凭据上 compose、资产动作与内部证据共用的进程内互斥锁 |
| 影子运行 | 新旧路径并行计算、只记录差异，不改变实际结果 |

业务流程详见[结算与履约](../guide/settlement.md)与[邮件流程](../guide/mail-flows.md)。
