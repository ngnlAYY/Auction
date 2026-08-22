# Auction-BF → Rust 迁移实施计划

## Context

本仓库当前是**启动骨架**：工作区、四个进程装配、健康检查、TOML 配置、双数据库适配器边界、
Dioxus 双 surface 与 5 语言已建立；当前工作重点是保持已有静态门禁，同时迁移尚未实现的业务功能。

旧系统 `../Auction-BF` 的实际规模（本次盘点所得，非估算）：

| 维度 | 数量 |
| --- | --- |
| Go 后端代码（非测试） | **105,228 行** |
| feature 模块 | 16 个 |
| PostgreSQL 迁移 | 28 个，共 61 张表 |
| 公开端 API 路由 | 20 条 |
| 管理端 API 路由 | 37 条 |
| 浏览器路由 | public 14 条 / admin 21 条 |
| 前端 TS/TSX | 21,516 行（+ 4,632 行 CSS，已迁移） |
| 前端服务端状态调用点 | 122 个 `useQuery`/`useMutation` |

各 feature 的代码规模决定迁移顺序和风险：

```
auction        41,296   ← 占 39%，含竞价/代理/结算/论坛/恢复
mail            7,323
users           7,184
delivery        7,107
realtime        3,926
keymanagement   3,876
adminconfig     2,995
operationsalert 2,654
servicehealth   1,807
sessions        1,514
mooglemail      1,373
audit           1,350
rules           1,260
workercontrol   1,192
equipment         956
world             411
```

本计划的目标是把这 105k 行 Go 按领域所有权重建为 Rust，而不是逐文件翻译。

## 必须先纠正的两处既有文档错误

盘点中发现本仓库文档与旧系统实际实现不符，阶段 0 已完成核对并修正，否则会按错误前提实现：

1. **竞价事务隔离级别写错了。** `docs/architecture.md:121-122` 与 `AGENTS.md:185` 写
   「PostgreSQL 使用行锁」。实际旧系统用的是
   `platformpostgres.RunSerializable(ctx, starter, 3, ...)`
   （`internal/platform/postgres/transaction.go:29-36`）——**`pgx.Serializable` 隔离级别
   加最多 3 次重试**，行锁（`FOR UPDATE OF l`）是在此之上的额外保护，不是替代。
   旧系统另有 `RunReadCommitted` 用于「通过显式行锁保护状态变更」的队列消费路径。
   两种隔离级别的适用边界必须在文档中区分，Rust 实现也要保留这一区分。

2. **`auction_commands` 幂等账本的结构比文档描述的更严格。** 实际为：

   ```sql
   command_id uuid PRIMARY KEY,
   command_kind text NOT NULL CHECK (command_kind <> ''),
   aggregate_id uuid NOT NULL,
   request_hash bytea NOT NULL CHECK (octet_length(request_hash) = 32),
   result_code text NOT NULL CHECK (result_code <> ''),
   result_payload jsonb NOT NULL DEFAULT '{}'::jsonb
       CHECK (jsonb_typeof(result_payload) = 'object'),
   created_at timestamptz NOT NULL DEFAULT transaction_timestamp()
   ```

   关键点：`request_hash` 是**固定 32 字节**（SHA-256），重放检测靠它而非比较负载；
   `bids.command_id` 是 `UNIQUE REFERENCES auction_commands(command_id) ON DELETE RESTRICT`
   ——幂等账本与事实表之间有外键，账本记录不可先于事实删除。
   `bid_id` 由 `auctioncommand.DeterministicID(command_id, "auction-bid-fact", 1)` 派生，
   即**同一命令重放必然得到同一 bid_id**，这是幂等的实现基础而非附带效果。

## 阶段划分

一个前置阶段和八个实现阶段。每阶段有明确的完成判据和 owner 边界；**不允许跨阶段并行推进领域与持久化**。

---

### 阶段 0：契约冻结（前置，不写业务代码）

盘点结果落地为可校验的契约清单，这是后续所有阶段的验收基准。

**产出物：**

- `docs/contracts/routes.md` — 20 条公开端 + 37 条管理端 API 路由全表，含方法、路径参数、
  状态码。路径逐条从旧 `internal/features/*/publichttp/`、`*/adminhttp/` 提取，**不允许
  在迁移中变更**。
- `docs/contracts/schema.md` — 61 张表的所有权归属表：每张表标注属于哪个 Rust crate、
  属于「完整档」「Lite 档」还是「两者」。
- `docs/contracts/events.md` — realtime 事件名、sequence/version 语义、WebSocket 消息结构
  （来源：`internal/features/realtime/public_contract.go`、`hub.go`）。
- `docs/contracts/invariants.md` — 每个可变命令的 `command_id` 语义、`request_hash` 算法、
  确定性 ID 派生规则、隔离级别选择依据。

**完成判据：** 上述四份文档以同一旧仓库提交为基线，且能回答「某个 URL/字段/事件改动
是否破坏旧契约」。当前已完成初版盘点；后续每次契约变化必须更新来源和 owner。

**不做：** 任何 SQL 或 Rust 业务代码。

---

### 阶段 1：领域层（`auction-domain`）

在不依赖 HTTP、数据库、外部服务的前提下重建状态机与不变量。

**范围（按依赖顺序）：**

1. 值对象与标识符：扩展现有 `ids.rs`，补 `LotId`、`BidId`、`UserId`、`CommandId`、
   `WorldId`、`SettlementId` 等带类型包装。
2. **金额类型**：旧 schema 全部是 `bigint NOT NULL CHECK (amount > 0)`
   （`000001_baseline.up.sql:274,287-288,325,337,352-353,359-362,374-375`）。
   用 `i64` newtype（如 `BidAmount`）配合领域校验，**不引入 `rust_decimal`**——
   会带来无谓的精度语义分歧。
3. 拍卖生命周期：现有 `auction.rs` 的 7 状态机已是正确骨架，补齐软延时、
   final phase、取消路径的边界条件。
4. 竞价与代理竞价规则：`BidPolicy`、`CompetitionPlan`、Fast/Slow 代理授权状态机
   （对应旧 `bidding_state.go`、`bid_proxy_authorizations` 与
   `bid_proxy_authorization_transitions` 两张表）。
5. 结算规则：`settlement_pools`、`settlement_obligations`、补偿池账本语义。

**边界：** 只允许依赖 serde、uuid、chrono、thiserror。已修正的 `chrono` 现含 `now` feature，
但**领域层不应自己取当前时间**——时间由 `Clock` 端口注入，保持可测试性。

**完成判据：** 每条不变量在代码注释中说明它保护什么、违反的后果是什么。

---

### 阶段 2：应用层端口（`auction-application`）

声明用例与窄端口，不含任何实现。

**范围：**

- 命令用例：`PlaceBid`、`SetProxy`、`LowerProxy`、`ClaimSettlement`、
  `CompleteSettlement`、`OpenAuction`、`CancelAuction` 等。每个命令携带 `command_id`。
- 幂等策略端口：`CommandLedger`，负责 `request_hash` 比对与结果重放。
- 事务端口：需要表达「隔离级别 + 重试次数」，因为旧系统区分 `RunSerializable`（竞价写入）
  与 `RunReadCommitted`（队列消费）。**不能用单一 `Transaction` trait 抹平这个区分。**
- 仓储端口：按 feature 拆分为窄 trait，不做上帝接口。
- `Clock` 端口已存在，现在可以实现（`chrono` 的 `now` feature 已补齐）。

**边界：** 不读配置、不开监听、不执行 SQL、不调外部服务。

---

### 阶段 3：PostgreSQL 完整档持久化（`auction-storage` + `migrations/postgres/`）

先建完整生产 schema，**再**考虑 Lite 子集。顺序不可颠倒。

**迁移策略：** 不照搬旧系统 28 个迁移的编号与切分。按领域所有权重新组织为追加式迁移，
但**共有对象保持同名同义**（表名、列名、索引逻辑名）。每个迁移文件在
`docs/contracts/schema.md` 中有对应条目。

当前已提交 PostgreSQL `000001` 至 `000007`：覆盖基础身份、拍卖与 lot、命令账本、
竞价事实、代理授权及历史、审计/realtime、结算/履约、生命周期、身份/外部命令、恢复/
可观测性和论坛同步对象。旧名 `bid_proxies` 由只读兼容视图提供，不再维护第二份权威
状态。`auction-storage` 已接入 PostgreSQL 竞价与结算事务适配器：命令账本重放/冲突、
权威状态锁、竞价/结算事实、履约初始状态、审计和 realtime 事件在对应事务内提交。
SQLite `000001_lite_core.up.sql` 与 `000002_worker_scheduler.up.sql` 及迁移入口已追加，
Lite 的 `BEGIN IMMEDIATE` 竞价和 Worker 调度写适配器已接入。

**优先级顺序**（依赖决定，非工作量决定）：

1. `worlds`、`users`、`equipment` — 被其他表引用的基础实体
2. `auction_commands` — 幂等账本，所有可变命令的前置
3. `auctions`、`lots`、`auction_preview_items` — 拍卖主体
4. `bids`、`bid_proxy_authorizations`、`bid_proxy_authorization_transitions`、
   `bid_proxy_attempts`、`bid_proxy_outbid_uses` — 竞价核心
5. `audit_records` — 追加式审计
6. `auction_settlements`、`lot_settlements`、`settlement_pools`、
   `settlement_obligations`、`compensation_ledger` — 结算
7. 其余按 feature 迁移顺序补齐

**竞价写入事务必须复刻的步骤**（源自
`internal/features/auction/postgres/adapter/repository.go:898-960`）：

```
RunSerializable(max_attempts = 3):
  1. 派生确定性 bid_id = DeterministicID(command_id, "auction-bid-fact", 1)
  2. 计算 request_hash，查 auction_commands：命中则校验存储结果一致性后重放
  3. lock_bid_state(lot_id)        → FOR UPDATE OF l
  4. 反序列化 policy_snapshot（快照存于 lot，非实时读取策略表）
  5. lock_active_authorization(lot_id)
  6. lock_participant_ids(...)     → 资格校验，含代理授权持有者
  7. 资格不符 → ErrBidderIneligible；活动授权持有者失格 → FreezeProxy 计划
  8. PlanCompetition(policy, state, authorization, trigger)
  9. 同一事务写入：bid 事实 + 状态版本 + audit event + outbox/event 记录
```

**注意第 4 步**：策略来自 `lot.policy_snapshot`（`000012_auction_policy_snapshot`），
不是实时查询策略表——这保证同一 lot 的竞价规则在轮次内不因管理端改配置而漂移。

**SQLx 查询校验：** 采用离线模式（`migrations/.sqlx/` 已在 `.gitignore`）。
共有查询才要求两个后端都校验。

---

### 阶段 4：SQLite Lite 档子集

**仅在阶段 3 对应能力完成后**才做，且只做 `docs/contracts/schema.md` 标为
「Lite 档」或「两者」的能力。

Lite 档的连接选项已在 `b803cb5` 修复并实证：文件库 WAL + 5 连接 + 5 秒 `busy_timeout` +
`create_if_missing` + `foreign_keys`，内存库单连接跳过 WAL。

**关键约束：** SQLite 没有 `FOR UPDATE`。竞价写入必须通过明确的串行写事务
（`BEGIN IMMEDIATE`）入口，并在 `auction-storage` 注释中写明这一差异。
「轻量」只缩减系统能力，**不放宽已声明支持功能的数据正确性**：`command_id` 幂等、
事务原子性、审计追加、凭据安全在 Lite 档同样必须成立。

Worker 并发在 Lite 档必须为 1，且该约束需在配置校验层拒绝而非仅写在文档里。

---

### 阶段 5：应用端口接入与 HTTP 表面

**顺序：先 Public，再 Admin。** 公开端契约面窄（20 条）且是主要流量路径。

**HTTP 层职责**（`auction-http`）：只做输入映射与契约输出。处理器不含竞价、结算、
状态机规则。DTO 全部放 `auction-contracts`，浏览器收不到 SQL 行。

**需要新增的横切能力：**

- 会话边界：Public 使用 HttpOnly + Secure + SameSite Cookie；Admin 使用 Basic Auth 一次性
  登录交换、独立 CSRF Cookie 和 `X-CSRF-Token`，数据库只保存 session/CSRF 摘要；会话存储在
  PostgreSQL/SQLite 对应的 `public_sessions`、`admin_sessions`，Redis 不得成为会话权威。
- `command_id` 提取与冲突响应：同一 ID 不同负载必须报冲突而非静默接受。
- 关联 ID 透传，接入 `auction-observability`。

**当前健康检查逻辑在 `auction-http`，与架构表声明的 `auction-observability` 所有权矛盾。**
本阶段一并迁移，并把 `HealthResponse` 的 `version` 传参约定（已在 `b803cb5` 建立）保留。

---

### 阶段 6：Worker 与外部集成

**范围：**

- `auction-jobs`：持久化游标（`realtime_consumer_offsets`、
  `auction_forum_reply_cursors`、`mail_poll_state`）、重试策略、due task 调度。
  **不得创建第二套拍卖状态机**——所有状态迁移调用 `auction-application` 的命令端口。
- `auction-integrations`：E-Hentai 论坛（发帖、回帖扫描）、MoogleMail。
  每个外部写入有 timeout、重试上限、幂等键、审计日志。
- 外部投递发生在**本地事务提交之后**；外部失败不能回滚已接受的竞价事实
  （`outbox` 模式，对应旧 `auction_forum_bid_commands`、`delivery_admin_commands`）。

当前已接入 `scheduled_rounds` 的持久化 due claim、租约、尝试次数和失败退避；Worker 的
preview/open 任务调用共享生命周期端口。PostgreSQL 完整档又接入了基于拍卖 settlement
workflow 字段的结算 due claim、稳定结算命令、完成 fence 和指数退避，结算执行仍复用
`SettlementCommandPort` 的 Serializable 事务；SQLite Lite 明确不启用该完整档 workflow。
结算后的履约在 PostgreSQL 已接入持久化 claim/fencing、稳定的 delivery/prepare/activation
身份，以及 `pending → prepared → enqueued` 的履约状态推进；`deliveries` outbox 也已接入
持久化领取、发送中 fencing、成功、失败和 `manual_reconcile` 终态边界。外部传输端口已经
定义，但 Worker 当前尚未装配 MoogleMail 传输实现，因此不会发出外部请求。实时消费的
PostgreSQL/SQLite 游标、失败记录、连续序号确认和显式 replay 边界已经接入应用端口、存储
适配器及现有 Worker 循环；由于外部广播协议尚未冻结，装配根暂不注入 sink，不会伪造远程
转发成功。论坛回帖游标、邮件轮询游标、远程协议映射、回执/对账与 recovery 履约仍未接入，
不能把当前 Worker 首片视为完整后台行为。A05 已先落地为纯本地论坛预览解析：HTML/文本输入
在 2 MiB 边界内解析为 catalog、posts、warnings，不打开远程发布或数据库写入；论坛回帖、
同步游标和远程回执仍需后续 Worker/外部协议阶段。

**凭据通道：** 论坛与邮件 Cookie 只能来自 `auction-config` 的
`ehentai_cookie()` / `mooglemail_cookie()` getter。**不新增任何其他凭据加载路径**
（包括 `.keyenv` 一类文件）——`SecretString` 的脱敏与校验是唯一入口。

---

### 阶段 7：浏览器端真实数据接入

骨架已就位（14 + 21 条路由、5 语言、BEM 视觉契约）。公开端首页当前拍卖看板、lot 详情、规则文档、公开黑名单、
竞价日志、成交价格历史和 `/status` 持久化实时事件游标页已经通过同源 HTTP 读取真实 Public API；管理端拍卖列表、delivery
页面与拍卖 lot 页面也已接入安全投影和只读数据读取。本阶段继续把其余占位页换成真实实现。

**顺序：** 只读优先——拍卖列表 → 拍品详情 → 竞价记录 → 登录 → 出价 → 代理 → 管理命令。

**实时更新用 WebSocket**（`/api/v1/ws`），与旧 `realtime/hub.go` 契约一致；事件带
sequence/version，跳号或重连后重新读取权威快照，**不在浏览器端累积状态**。当前已先把
`/api/v1/events` 的持久化 Catch-up 已接到 PostgreSQL/SQLite 事件表，支持拍卖流、after 游标、
有界 page_size 和 `refetch_required`；`/api/v1/ws` 已接入 Public session、只读服务端推送、
128 个并发连接上限和 ping/pong，仍以后续的持久化 Catch-up 作为恢复权威。
公开端 `/status` 已接入 `/api/v1/events` 的安全事件信封、sequence 游标、继续读取、刷新和
`refetch_required` 停止边界；已认证会话还会自动订阅 `/api/v1/ws` 的只读事件页，客户端只替换
服务端安全快照，跳号或 `refetch_required` 后停止并要求重新读取权威数据。公开端拍卖看板现在
会对已注册的拍卖生命周期、目录变化和竞价事件重新读取 `/api/v1/auctions/current` 及其 lot
快照，不在浏览器端累积事件状态；Admin 拍卖列表也已提供带 CSRF、全新 `command_id` 和成功/失败
反馈的 preview/open/cancel/final-phase 操作入口。

**改状态的动作必须携带 `command_id`**，并显式展示冲突与幂等结果。

**i18n 约束：** 新增文案必须同步更新全部 5 个 `*.ftl`——`t!` 键缺失会 panic。
Fluent 消息 ID 不含点号（旧 `nav.home` → `nav-home`）。建议在 CI 加键集比对脚本，
把当前手工校验固化。

---

### 阶段 8：切换

- 双跑只读流量，比较事件、余额、竞价结果。
- 确认数据一致性与运维 runbook 后再切写。
- 行为验证（对照测试、双写、切换前验证）在此阶段才纳入范围，**且需用户明确要求**。

---

## 跨阶段的工程约束

**可观测性缺口（部分解决）：** `auction-observability` 已拥有 service heartbeat 的安全模型、
`HealthService` 和 dashboard health/process 聚合；`auction-storage` 已接入 PostgreSQL/SQLite
读取与 sequence-fenced 发布适配器，A02 Admin route 与 Public/Admin/Worker 的周期心跳发布已接入。
旧系统用
`prometheus/client_golang` + 4 个 OpenTelemetry crate（`otel`、`otel/sdk`、`otel/trace`、
`otlptracehttp`）。Rust 侧 metrics/tracing exporter、Worker control 和 mail/realtime/delivery
等 dashboard owner 仍需后续决定与迁移。旧系统还有 `operationsalert`
（2,654 行）依赖告警信号表——它属于哪个 Rust crate 需在阶段 0 定明。

**测试边界：** 按 `AGENTS.md`，在用户明确要求前不新增或运行单元测试、功能测试、集成测试、
端到端测试或浏览器测试。每阶段的默认验收只有 `just verify`
（fmt / check --locked / feature-check / clippy -D warnings / 两个 wasm 目标）加 MSRV job。
若用户要求恢复测试：PostgreSQL 验证完整契约、行锁、隔离与并发竞价语义；SQLite 只验证
Lite 能力契约，**不能替代** PostgreSQL 的隔离验证。

**防腐层：** 兼容旧数据时建立明确的映射适配器，不把旧表结构或旧命名未经审计地传播进
新领域模型。

**源码目录：** 每个 apps/* 和 crates/* 的 src 根只保留薄的 main.rs 或 lib.rs；实现文件
必须放入按职责划分的子目录，并在对应 README 与模块声明中保持 owner 一致。目录重组只允许
保持公开契约和依赖方向不变的机械迁移，不借机把业务规则塞进入口文件。

**单一 owner：** 每张表、每个迁移、每条写入查询、每个事件、每条路由只有一个 owner
crate；新增对象时在 `docs/contracts/schema.md` 记录归属。

## 验证

每阶段结束时运行：

```bash
just verify
cargo +1.94.1 check --locked --workspace --all-targets --all-features
```

阶段 3 之后追加（需要本地 PostgreSQL）：

```bash
cargo sqlx prepare --workspace -- --all-targets   # 离线查询元数据
just db-migrate
AUCTION_CONFIG_FILE=ops/config/examples/public.toml cargo run -p auctionctl -- check-config public
```

阶段 7 之后追加（需要 dx CLI）：

```bash
just web-build
# 启动进程并访问 /health 与静态资源属行为验证，需用户明确要求
```

## 当前实施状态

阶段 0 的四份契约文档已经完成初版盘点，且本文开头指出的两处文档错误（竞价隔离级别、
幂等账本结构）已纳入架构、协作规范和契约文档。后续阶段必须按修正后的语义实现。

阶段 1 已完成首片：带类型标识、金额值对象、拍卖状态版本、软截止/取消边界、竞价事实、
代理授权、纯领域 `CompetitionPlan` 和结算值对象已落地。阶段 2 已完成首片：事务策略、
命令账本、竞价/结算事务端口和 `place_bid`/`settle_auction` 用例已落地。阶段 3 的
PostgreSQL `000001` 至 `000007` schema 与竞价/结算/生命周期事务适配器已接入；阶段 4 的
SQLite Lite schema、迁移入口和 `BEGIN IMMEDIATE` 竞价写适配器已接入，且 Lite Worker
并发已在配置层限制为 1。阶段 5 已注册 Public 20 条与 Admin 37 条冻结路径，并完成稳定
响应封装、世界/拍卖/lot 目录 DTO、应用只读端口、PostgreSQL/SQLite 查询适配器、Public
session 与 Admin session 端口、两套启动时状态装配；Admin session 已接入 Basic Auth 登录交换、
双 Cookie、CSRF 校验、会话轮换/刷新/撤销及两套数据库适配器。`/api/v1/worlds`、`/api/v1/rules`、`/api/v1/blacklist`、`/api/v1/auctions`、
`/api/v1/auctions/current`、`/api/v1/auctions/{auction_id}/lots`、
`/api/v1/auctions/{auction_id}/bid-log` 与 `/api/v1/lots/{lot_id}` 在配置数据库后返回
权威数据，拍卖列表保留旧的页码与 keyset cursor 语义，lot 列表保留 preview/live 互斥投影、
分类统计和旧分页形状，竞价日志保留 world/lot 筛选、编号页与完整 keyset cursor 分页，
价格历史保留旧筛选、成交条目、编号页 stats 与 cursor 页未知 total 语义；P13/P14 补偿池
与单场账本已接入 PostgreSQL 完整档的 Serializable 快照、守恒/闭集校验、opaque cursor 和
旧分页响应，SQLite Lite 明确返回 `501 feature_not_supported`。Public session 已
使用 HttpOnly 会话 Cookie 与持久化会话表；P20 普通竞价已接入会话身份、稳定
`command_id`/`request_hash`、应用竞价事务和旧版结果投影。P16/P17 账户读取已接入 Public session、
应用端口及 PostgreSQL/SQLite 快照投影，覆盖普通竞价、代理 quota、转换历史、最近 proxy bid
和 owner capability；其余未迁移 handler 仍明确返回 501。P18/P19 代理创建/降低命令已返回旧版
`{ authorization: ... }` 完整命令投影，quota、转换历史和终止原因由提交后的安全账户快照补齐，
并在授权状态版本不一致时失败关闭；代理重入的 `usage_number` 在 lot 锁内按用户历史最大值递增，
避免重入和命令重放混淆；Admin A06 lot 列表已接入
PostgreSQL Serializable 同快照的拍卖/lot/装备/投稿投影、Admin 会话保护和管理端表格，SQLite
Lite 因缺少完整生命周期字段明确返回 `501 feature_not_supported`。Admin 的 settlement-report
与 recovery-report 已接入 PostgreSQL 完整档一致只读/安全快照，SQLite 明确返回 `501 feature_not_supported`。Admin 的 preview/open/cancel/
final-phase 已接入统一生命周期端口，并在同一数据库事务内写入命令账本、状态版本、追加审计和
注册表内的 lifecycle realtime 事件；A03 GET/POST 拍卖已接入 Admin 会话、旧分页/筛选、PostgreSQL Serializable 生命周期/lot 数量快照、管理端表格和创建表单；其创建命令已接入零版本 fence、命令账本、world/intake target 锁、策略快照、审计和 `auction.created` 事件，使用当前冻结的显式默认策略；SQLite Lite 创建与列表均返回 `501 feature_not_supported`。Public `/api/v1/events` 已接入 PostgreSQL/SQLite
持久化事件回放、sequence 游标和 schema 安全边界；Worker 已接入 `scheduled_rounds` 的持久化 due claim、
租约、尝试次数、preview/open 生命周期调用和失败退避，并在 PostgreSQL 完整档接入结算
workflow 的 due claim、稳定 `settle_auction` 命令、完成 fence 和失败退避；SQLite Lite 明确
不启用结算 workflow。公开端首页看板、lot 详情、规则文档、公开黑名单、补偿池/账本、论坛镜像、竞价日志、成交价格历史和 `/status` 持久化实时事件游标页已接入同源 Public API；
管理端 delivery 页面已接入列表、release/retry/reconcile-retry 命令，操作携带 `command_id`、
CSRF 和 fencing version。PostgreSQL 完整档还
接入结算后的履约 claim/fencing、delivery outbox 的持久化状态推进和重试边界；公开端拍卖看板
已按实时事件重新读取权威快照，管理端拍卖列表已提供带 CSRF、全新 `command_id` 和成功/失败反馈的
生命周期操作，但外部传输、远程回执/对账、论坛同步、实时外部广播和其余 Web 页面仍待后续阶段。
Admin A34-A37 delivery 列表、release、retry 和 reconcile 已接入 PostgreSQL 完整档的安全投影、
fencing version、独立命令账本以及结算履约状态联动；SQLite 明确返回 `501 feature_not_supported`。
这些接口只处理本地 outbox 的人工状态边界，不把未冻结的远程 MoogleMail 结果伪造成成功。Admin A14 recovery 确认已接入 PostgreSQL 完整档的 Serializable 版本 fence、
证据重校验、命令账本、退款激活、guarded relist、审计和预览目录 realtime 事件；SQLite 明确
返回 `501 feature_not_supported`。当前只覆盖已有 recovery 记录的确认，recovery 生成、外部
履约传输和回执对账仍待后续阶段；Worker 实时消费游标已接入，但外部广播转发和其余 Web 页面仍未接入。
Admin A03 GET/POST 拍卖、A06 lot 列表、A15 代理历史和 A25 规则 GET/POST/PUT 已接入 PostgreSQL 完整档的一致安全投影、有限页码/opaque cursor、Admin 会话保护和管理端真实页面；A03 POST 的策略字段仍由应用层当前冻结的默认快照生成；A04 装备录入已接入 URL/canonical JSON 来源、短生命周期明文 key、AES-256-GCM 密文信封、PostgreSQL Serializable world/intake target/玩家/装备锁、同一事务的装备/密文/投稿/lot/版本/审计/realtime 写入，以及草稿 lot 管理端表单；密钥标识与材料只允许来自敏感 TOML，SQLite Lite 明确返回 `501 feature_not_supported`。A25 PUT 使用规则 revision CAS、命令账本和审计，SQLite 规则只读，发布明确
返回 `501 feature_not_supported`。A15 从命令账本恢复代理计划后只输出安全事件字段，不把原始结果负载或代理密钥传给浏览器。Admin A21-A24 realtime 运维接口已接入 PostgreSQL/SQLite 的 lag、failure 安全投影和带命令账本/审计的 replay
事务；外部广播 sink 仍未冻结。源码目录重组已经完成，仍不引入外部服务行为或测试用例。
A05 论坛预览已接入 Admin session、origin/CSRF、2 MiB 请求体上限、应用端口、本地 HTML/文本
解析器和稳定 DTO；恰好一个输入字段会被解析为 catalog、posts、warnings，解析失败只返回
400，当前实现不抓取远程页面、不写数据库、不宣称远程发布成功。
A33 Admin 审计查询已接入应用端口、PostgreSQL/SQLite 追加式表读取、Admin session 和稳定 DTO；
保留严格精确筛选、时间范围、opaque keyset cursor 与数字分页，数字页的 total 与记录在同一
读取事务中取得，metadata 只允许安全 JSON object，不新增任何写入或测试用例。
A02 Admin dashboard 第一片已接入 `auction-observability` 的心跳安全模型、PostgreSQL/SQLite
service_heartbeats 适配器、Admin session 保护，以及三个进程装配根的启动/就绪/周期/停止心跳
发布，真实返回 health/process 聚合；未迁移的 owner section 显式返回 `unknown`，Worker control
仍未接入，不能把该首片视为完整 dashboard。
A13 Admin draft lot 修改已接入 `PATCH /api/v1/admin/lots/{lot_id}`：请求携带
`command_id`、`expected_state_version`、`action` 和可选 `start_price`，由 Admin session/CSRF
保护后进入 PostgreSQL Serializable 应用事务；事务按 world → auction → lot 顺序锁定并完成
版本 fence、命令账本重放、lot 状态/价格更新、预览 revision/state version、追加审计和
`auction.preview_updated` realtime 事件。withdrawn 动作同时创建 `withdrawn_return` 履约义务
及 pending fulfillment，SQLite Lite 明确返回 `501 feature_not_supported`；管理端 lot 页面已
提供 draft pending lot 的改价与撤回入口。该切片仍不等于完整 Admin lot/外部履约行为。

A16-A19 Admin 邮件首片已接入：`GET /api/v1/admin/mail` 返回指定邮箱的轮询安全状态，
`GET /api/v1/admin/mail/intake` 与 `/intake/{intake_id}` 返回严格筛选下的入站邮件安全摘要，
`PUT /api/v1/admin/mail/polling` 使用 `command_id`、`expected_state_version`、Admin session
和 CSRF 完成暂停/恢复。`auction-application` 持有邮件查询端口、版本 fence 和命令用例，
`auction-storage` 的 PostgreSQL 适配器在读取快照或 Serializable 写事务中访问
`mail_poll_state`、`inbound_mail` 和 `mail_admin_commands`，成功变更同时追加审计并支持命令
结果重放；原始邮件内容、外部 Cookie、轮询游标和任意 metadata 不进入 HTTP/Web DTO。管理端
邮件页面已接入轮询状态和入站审查表格，五种语言文案均已同步。SQLite Lite 没有该完整邮件
能力所需的表和外部协议，相关接口明确返回 `501 feature_not_supported`。

A20 `/api/v1/admin/mail/polls:trigger` 仍保持 `feature_not_migrated`：Worker 调度 owner、
MoogleMail 远程触发协议、幂等回执和失败重试边界尚未冻结，本次没有发送外部请求，也没有把
本地状态读取误报成远程轮询成功。邮件状态/审查首片因此不等于完整邮件生命周期迁移。

A26/A27 Admin runtime 配置首片已接入：`auction-application` 的严格非敏感文档模型、稳定
fingerprint、版本化 request hash、pending/applied/unknown 恢复服务和窄端口已落地；
`auction-config/src/runtime/` 负责 regular-file/权限/锁校验以及同目录 fsync + rename 原子
替换，`auction-storage` PostgreSQL 适配器负责 `runtime_config_admin_commands`、审计外键和
命令重放。`auction-http` 已接入 GET/validate/PUT，`auction-web-admin` 的 Config 页面已
使用 JSON 编辑器、候选校验和重启提示，五种语言文案已同步。SQLite Lite 明确返回
`501 feature_not_supported`；runtime 更新不热改当前进程，文件写入成功后必须重启 Admin。
这只是 A26/A27 的持久化与页面首片，用户、凭据管理、Worker control 聚合和其余 Admin
配置行为仍未迁移。

A29 Admin 改名历史只读首片已接入：`auction-application` 声明 `user_id`/用户名筛选、有限
数字分页和安全改名事实投影，`auction-storage` 在 PostgreSQL/SQLite 的同一读取事务中读取
`user_renames`，`auction-http` 接入 Admin session 下的 `GET /api/v1/admin/renames`，管理端
`/renames` 页面已接入筛选表格并同步五种语言。A30 回滚仍保持 `feature_not_migrated`；待后续
切片补齐用户状态版本 fence、命令账本、最新改名校验和用户名占用冲突后，才开放写入命令。
