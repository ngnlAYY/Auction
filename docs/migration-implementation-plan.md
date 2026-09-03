# Auction-BF → Rust 迁移实施计划

## Context

本仓库当前处于**功能迁移后期**：工作区、四个进程装配、健康检查、TOML 配置、双数据库适配器
边界、Dioxus 双 surface 与 5 语言已建立；旧版源码能力的静态盘点已收敛，发布制品和同宿主机隔离
systemd/Caddy 生命周期也已取得 R1。当前工作重点是保持已有静态门禁，并完成真实外部服务、浏览器
产物和最终生产切换的行为对照。

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

当前已提交 PostgreSQL `000001` 至 `000032`：覆盖基础身份、拍卖与 lot、命令账本、
竞价事实、代理授权及历史、审计/realtime、结算/履约、生命周期、身份/外部命令、恢复/
可观测性、论坛同步、论坛未知写入人工授权、Final phase 调度、领先时间修正、异世界 carrier
dispatch、生命周期/operations alert/lot 结束时间数据库不变量、旧 v28 bid 缺失结束时间的
严格兼容 trigger、已证明 MoogleMail 退信的独立告警 identity，以及 operations alert 的运行状态、
扫描水位和独立时钟语义、HV 战斗页占用时的 recovery `busy` 余额状态、lot 的分类内展示编号，
以及论坛固定 #1/#2 的远端身份和内容版本证明、同一 world/provider 的单用户论坛账号归属、
Users 级稳定论坛账号 ID、论坛命令真实楼层来源、逐主题同步健康/调度事实、论坛身份变更历史和
追加式审计证据，邮件/论坛两类外部 Cookie 各自唯一 active 的数据库防线，以及论坛主题由完整读取
证明的连续处理楼层水位，以及 Preview 持续录入、Open 原子接管后继 Draft 的数据库防线。
`lots.lot_number` 继续作为拍卖内全局顺序，`category_key`/`category_number` 固化每个装备分类从 1 开始且
不复用的用户可见编号；论坛同步记录同时固化编号方案，避免重写已经发布的旧主题。旧名 `bid_proxies` 由只读兼容
视图提供，不再维护第二份权威
状态。`auction-storage` 已接入 PostgreSQL 竞价与结算事务适配器：命令账本重放/冲突、
权威状态锁、竞价/结算事实、履约初始状态、审计和 realtime 事件在对应事务内提交。
SQLite `000001` 至 `000010` 及迁移入口已追加，
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
  4. 反序列化 policy_snapshot（快照存于 auctions，非实时读取策略表）
  5. lock_active_authorization(lot_id)
  6. lock_participant_ids(...)     → 资格校验，含代理授权持有者
  7. 资格不符 → ErrBidderIneligible；活动授权持有者失格 → FreezeProxy 计划
  8. PlanCompetition(policy, state, authorization, trigger)
  9. 同一事务写入：bid 事实 + 状态版本 + audit event + outbox/event 记录
```

**注意第 4 步**：当前迁移树把策略快照存于 `auctions.policy_snapshot`（由 PostgreSQL
`000002_settlement_core` 建立），不是实时查询策略表——这保证同一拍卖轮次的结算规则在管理
端改配置后仍保持不漂移。竞价锁内已经读取 lot 所属拍卖的完整规则快照；结算继续由同一
快照提供结算/恢复策略。

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

- `auction-jobs`：持久化游标（`realtime_consumer_offsets`、`mail_poll_state`）、重试策略、due task 调度。
  论坛扫描以每个主题的 `auction_forum_sync.content_next_scan_at` 作为权威调度事实；
  `auction_forum_reply_cursors` 仅保留为旧迁移兼容表，当前运行路径不再写入。
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
身份，以及 `pending → prepared → enqueued` 的履约状态推进；recovery action 也已使用同一
两阶段本地 delivery 身份和 action fence；`deliveries` outbox 也已接入
持久化领取、发送中 fencing、成功、失败和 `manual_reconcile` 终态边界。MoogleMail 入站
读取适配器已经完成固定 endpoint、无重定向 GET、响应边界、分页 cursor、allowlist-first
详情读取和严格 HTML/脚本/附件解析；PostgreSQL `000008_mooglemail_polling` 迁移与
`MailPollStore` 又接入了 due claim、lease/fencing、连续失败退避，以及
`inbound_mail`、`mail_commands` 和 cursor 的同事务幂等提交。Worker 通过
`[worker.mooglemail]` 显式启用后，会按顶层 world 与 `additional_worlds` 为每个已配置世界装配
独立的持久化邮箱状态、定向 claim、读取 gateway、业务 processor、拍卖入站事务和 delivery
outbox；配置独立 `credential_data_key` 且使用 PostgreSQL 时，Worker 优先从 A32 唯一 active Cookie
读取并在内存解密，未配置时继续兼容固定 `.keyenv` 或敏感配置，数据库只保存安全摘要与无密文
payload。当前首条
业务链已覆盖拍卖邮件的 Credits/COD 拒绝、无附件通知、装备来源读取、起拍价指令解析、拍卖
草稿/lot 入站以及 take/return/静态通知 delivery；MoogleMail 写入协议层已补齐
compose/detail typed target、表单闭集、单次 POST、`write_unknown` 不确定性分类、发送成功页证明，
以及 return/take 的动作前身份/资产 fence 和动作后结构化证明。`MoogleMailSender` 仍只接受已解析的
远端装备 ID；当前 delivery
transport 已执行版本化静态通知、用户注册/重绑结果通知、return 和 take，并新增版本化结算
send/return：主世界装备 COD、异世界 Credits+COD、装备交接、卖家收益和买家退款都由同一
数据库快照生成无密文 intent，邮件来源的未售/撤回义务执行原邮件 return，后台来源执行装备
交接通知。send 现已按旧版从可见文字精确分类收件人不存在和邮资拒绝；前者保留退信 `mid`/
`take_required` 安全证据，后者在已暂存资产时进入人工对账。结算 `send` 的 compose 成功页只记录
delivery 成功，履约仍保持 `enqueued`；新增
Sent 分页/exact ID 查找、详情资产矩阵解析、Balance 可见值/脚本值一致性解析和 Worker
自动对账，满足时追加 proof kind/reference/digest/observed_at，发送成功页没有 exact remote ID
时转入 `manual_reconcile`。用户注册 key 只以密文信封落库，delivery outbox 只保存用户 UUID
引用，发送前才由 Worker 解密。论坛主题同步和回帖导入已经接入 PostgreSQL Worker：主题发布使用
持久化 claim/fencing，结果未知时先按精确标题只读对账；未命中必须经 PostgreSQL `000016` 的
幂等人工授权和追加审计才允许下一次 POST。论坛 `showuser` 数字 ID 是稳定唯一的外部账号键，保存到
`users.forum_account_id`；论坛用户名是可变的 MM 收件人和页面显示名称，权威字段为
`users.username`。解析器只接受同一个论坛生成作者链接里的数字键与名称。Users owner 在
Serializable 事务内同时核对两者，仅让 active-key 同名用户建立首个映射，已绑定主体可随论坛名称
变化更新 `username`；revoked 用户必须沿用另一世界已经证明的相同数字键，否则建立 revoked-key
`forum_only` 主体供后续 MM 注册恢复，且不自动签发 active Key。PostgreSQL `000026` 限制同一
world/provider 的单用户账号归属，`000027` 增加 Users 级唯一论坛账号 ID，并以延迟约束保证 Users
与该主体全部论坛映射在事务提交时一致；`000029` 为每次论坛身份变化追加历史，并在同一
Serializable 事务中写入 `user.forum_identity_synced` 审计事实；Admin 修正数字 ID 时同步更新全部映射。旧
`game_player_id` 只作 `username` 的数据库兼容镜像。系统不生成 `forum-*` 影子用户名，也不把数字键交给 MM。论坛与公开
拍卖投影统一展示当前名称。回帖再通过 world/lot 映射和命令账本调用现有竞价端口。每个主题新增固定 #1 拍卖目录和 #2 编辑/错误记录：`000025` 保存两个 exact
post ID 与内容版本，用户命令从 #3 开始；确定竞价结果提交后刷新 #1，解析/编辑/拒绝结果稳定更新
#2，pending 命令同样显示，并以独立内容 lease/fencing 串行远端编辑；`000028` 保存每条命令可证明的
真实楼层号，`000031` 单独保存主题连续已处理水位，待重试命令阻止水位越过对应楼层。固定帖编辑会直接读取编辑页
textarea 中的原始 BBCode，以写入前后逐字一致作为成功证明；上一轮为 `write_unknown` 时，远端内容
不等于预期值会失败关闭，不能凭页面渲染结果推断成功。`000029` 还把最近成功时间、连续失败数、
错误摘要和下一次扫描时间固化到每个主题；成功扫描默认推进五分钟 due，失败按主题退避，不再依赖
world 级扫描游标。主题读取按同 origin/thread 的规范 `st` 分页补齐、默认最多 10 页，既有 #2 不是
系统标记时不覆盖。
MM `notify` 已进入 MoogleMail delivery outbox；结算事务现在会为每个获胜
lot 同事务建立 `settlement_recoveries`，冻结付款超时和提醒策略。Recovery Worker 的 PostgreSQL
完整档已接入 due case 推进：付款满足时推进主世界完成或异世界装备交接，付款超时推进
`payment_overdue` 并创建稳定的 `payment_reminder` action；action 具备持久化 claim/fencing、
MoogleMail delivery outbox 关联、发送结果证明、退避和人工复核边界。当前 recovery 首条履约链还会
严格解析带 obligation UUID 的退回邮件证据，在 Serializable 事务内绑定 return mail、比对
Credits/COD/附件摘要并在不匹配时阻断；匹配后按 ordinal 创建装备领取/载具取消/装备恢复 action，
Take 必须携带来源邮件和预期资产摘要，写入前精确核对 sender、装备 ID、Credits、COD 和附件数，
写入后以同一邮件身份、资产归零和唯一回执及目标世界证明收口。recovery
delivery 先写入 `prepared`，再以独立 activation identity 激活为 `pending`，action 只有在
activation fence 成功后才进入 `enqueued`。COD 相关
action 完成后还会通过 Users owner 的幂等命令写入买家黑名单并记录独立 proof。Recovery Worker
现已补齐 `relist_pending` 的 guarded relist、`refunding` 的 buyer refund fulfillment 收口、
resolution 状态版本和 worker 审计；论坛
`start/cancel` 和 PM-only `notify` 对照旧版仍保持 warning-only 边界，MM `notify` 已接入。
实时消费的 PostgreSQL/SQLite 游标、失败记录、连续序号确认和显式 replay 边界已经接入应用
端口、存储适配器及 Worker 循环。每个 namespaced consumer 启动前还会取得唯一 active owner：
PostgreSQL 使用会话级 advisory lock，SQLite 在同主机使用数据库标识与 consumer key 摘要命名的
0600 非阻塞 sidecar 文件锁；重复 owner 会直接拒绝启动，非 Unix 平台不能证明互斥时失败关闭。
再次核对旧版 `realtime/relay.go` 后确认，其 relay 只把持久化事件广播到当前 Public 进程内 Hub，
并不存在外部广播协议；当前 Rust 持久化 relay + Hub 已达到
该旧版边界。真正的外部广播 sink 属于新增系统能力，不再列作旧版功能等价缺口；如后续需要，仍须
另行冻结协议、幂等和回执 owner，不能伪造远程转发成功。Public readiness 同时检查数据库迁移、
relay 任务存活和最近成功消费时间，不能只因 HTTP listener 存活就报告 ready；relay task panic、
意外退出或 abort 会立即撤销健康证明。
A05 已先落地为纯本地论坛预览解析：HTML/文本输入
在 2 MiB 边界内解析为 catalog、posts、warnings；E-Hentai 远程主题读写和 PostgreSQL 回帖
命令导入已在独立 Worker/集成端口接入，解析器仍不直接写数据库或拍卖状态。

**凭据通道：** A32 已将生产外部 Cookie 的管理入口拆为独立的
`credential_data_key_id`/`credential_data_key` 密钥域。Admin 只在 PostgreSQL 完整档提供凭据
导入、改名、启停、删除和安全投影读取；Worker 按 world/shared 读取唯一 active 密文 Cookie，在内存
解密后使用，存在同类重复 active 行、密钥不匹配或密文损坏时失败关闭；暂时没有
active 行时保留 degraded 运行态，实际操作返回 credential unavailable。该密钥不能复用装备或用户
登录 key，也不能进入日志、DTO、快照或提交内容。凭据 owner 内部已补齐 scope 最近更新时间、
最近 Admin 变更时间和 Worker 启动心跳重启判断；旧 v1 dashboard 只投影聚合后的 availability、
观测时间和最近 Admin 变更时间。Worker 未被可靠观测时保持 unknown，不把未验证状态误报为成功。

兼容回退仍允许 `auction-config` 的 `ehentai_cookie()` / `mooglemail_cookie()` getter，以及
`auction-config::load_mooglemail_keyenv` 读取 `.keyenv` 的两个固定键：`hv-ck-key1` 是 Worker
运行 Cookie，`hv-ck-key2` 只供显式隔离验证。`.keyenv` 入口只允许 owner-only 文件权限和固定键
闭集，不构成通用环境变量覆盖；值仍由 `SecretString` 脱敏，Worker 不把测试值加入候选或自动切换。

---

### 阶段 7：浏览器端真实数据接入

14 + 21 条浏览器路由、5 语言和 BEM 视觉契约均已建立。Public/Admin 正式页面都已接入真实
API；通用 panel/placeholder 只用于加载、错误或空结果，不再代表未实现页面。剩余 Web 工作是
在真实浏览器产物中逐页核对 Admin 视觉、交互和响应式细节，以及外部服务返回的错误状态。

**顺序：** 只读优先——拍卖列表 → 拍品详情 → 竞价记录 → 登录 → 出价 → 代理 → 管理命令。

**实时更新用 WebSocket**（`/api/v1/ws`），与旧 `realtime/hub.go` 契约一致；事件带
sequence/version，跳号或重连后重新读取权威快照，**不在浏览器端累积状态**。当前已先把
`/api/v1/events` 的持久化 Catch-up 已接到 PostgreSQL/SQLite 事件表，支持拍卖流、after 游标、
有界 page_size 和 `refetch_required`；旧版单拍卖读取要求规范 `auction_id`，发现读取使用显式
`scope=auctions` 扩展。`/api/v1/ws` 已接入 Public session、只读逐事件推送、有界连接和 ping/pong，
并始终以持久化 Catch-up 和权威快照作为恢复依据。
公开端 `/status` 按旧版保持静态服务说明，不建立业务请求；持久事件 catch-up 与 WebSocket owner 位于
公开拍卖看板。已认证会话会先完成 `/api/v1/ws` 握手、再以 HTTP 补齐游标，并消费旧版单事件帧；
跳号或 `refetch_required` 后停止并要求重新读取权威数据；权威重读后的首条已接受事件只建立新的
sequence 基线，不会再次触发重复刷新。公开端拍卖看板现在会对已注册的拍卖生命周期、目录变化和
竞价事件重新读取 `/api/v1/auctions/current` 及其 lot 快照，并以 readiness 10 秒、当前/共享目录、
lot 列表和可变详情 60 秒的周期回读兜底，不在浏览器端累积事件状态；Admin 拍卖列表也已提供带 CSRF、全新 `command_id` 和成功/失败
反馈的 preview/open/cancel/final-phase 操作入口。

**改状态的动作必须携带 `command_id`**，并显式展示冲突与幂等结果。

**i18n 约束：** 新增文案必须同步更新全部 5 个 `*.ftl`——`t!` 键缺失会 panic。
Fluent 消息 ID 不含点号（旧 `nav.home` → `nav-home`）。CI 已通过
`ci/check-policy.sh i18n` 固化五语言键集、重复 ID、点号 ID 和字面量引用检查。

---

### 阶段 8：切换

- 双跑只读流量，比较事件、余额、竞价结果。
- 确认数据一致性与运维 runbook 后再切写。
- 行为验证（人工对照、双写、切换前验证）在此阶段才纳入范围，**且需用户明确要求**；当前授权仅覆盖
  隔离人工对照和 Auction-Test 中最小化的 MoogleMail main Credits/装备真实写入，不包含新增/运行
  测试用例、论坛写入、双写或生产流量切换。

---

## 跨阶段的工程约束

**可观测性状态（旧版代码边界已接入）：** `auction-observability` 已拥有 service heartbeat 的安全模型、
`HealthService` 和 dashboard health/process 聚合；`auction-storage` 已接入 PostgreSQL/SQLite
读取与 sequence-fenced 发布适配器，A02 Admin route 与 Public/Admin/Worker 的周期心跳发布已接入。
旧系统用
`prometheus/client_golang` + 4 个 OpenTelemetry crate（`otel`、`otel/sdk`、`otel/trace`、
`otlptracehttp`）。Rust 侧已接入默认关闭、进程隔离的 Prometheus `/metrics` scrape、基础进程
身份指标、Worker control scrape、旧版 Worker loop 指标，以及三类进程的 OTLP/HTTP tracing
exporter。Public/Worker 从主配置 `[tracing]` 读取，Admin 从 runtime `[tracing]` 读取，角色专属
headers 只从敏感 TOML 注入；三类 exporter 均使用 Tokio 异步批处理、有界队列、丢弃计数，并在
服务退出路径显式关闭。三进程已通过 `auction-process` 统一订阅 SIGINT/SIGTERM：首次信号触发
10 秒有界 HTTP/Worker drain，排空期间第二次信号强制收口，停止心跳后再按配置上限刷新
 exporter。基于真实 readiness/audit/tracing 的周期心跳安全状态现已收口。`auction-storage` 已在
SQLx 池的普通查询、事务和显式 acquisition 边界
接入旧版 pgx 对应的十项连接池指标；固定低基数的竞价、代理和主要 Worker 业务结果/耗时指标
也已接入。绕过该边界的外部原生池不纳入累计统计。
operations alert 已由
`auction-application` 定义信号/evaluator 与窄端口，`auction-storage` 负责 PostgreSQL 完整档
信号、批次、lease/fencing、三次 Create 尝试、成功入队后的 cooldown、delivery 状态和稳定终态
对账，`auction-jobs` 负责扫描/通知入队，
`auction-http`/`auction-web-admin` 已接入 alerting dashboard section。SQLite 明确返回能力缺失。
Worker control 协议本身已按 UDS/peer credential 边界接入。

**测试边界：** 按 `AGENTS.md` 和本轮用户要求，不新增或运行单元测试、功能测试、集成测试、
端到端测试或浏览器测试。每阶段的默认验收只有 `just verify`
（fmt / 源码布局与命名 / 配置注释 / Fluent 键集合 / check --locked / feature-check /
clippy -D warnings / 两个 wasm 目标）加 MSRV job。
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
PostgreSQL `000001` 至 `000032` schema 与竞价/结算/生命周期事务适配器已接入；阶段 4 的
SQLite Lite schema、迁移入口和 `BEGIN IMMEDIATE` 竞价写适配器已接入，且 Lite Worker
并发已在配置层限制为 1。阶段 5 已注册 Public 20 条与 Admin 37 条冻结路径，并完成稳定
响应封装、世界/拍卖/lot 目录 DTO、应用只读端口、PostgreSQL/SQLite 查询适配器、Public
session 与 Admin session 端口、两套启动时状态装配；Admin session 已接入 Basic Auth 登录交换、
双 Cookie、CSRF 校验、会话轮换/刷新/撤销及两套数据库适配器。`/api/v1/worlds`、`/api/v1/rules`、`/api/v1/blacklist`、`/api/v1/auctions`、
`/api/v1/auctions/current`、`/api/v1/auctions/{auction_id}/lots`、
`/api/v1/auctions/{auction_id}/bid-log` 与 `/api/v1/lots/{lot_id}` 在配置数据库后返回
权威数据，拍卖列表保留旧的页码与 keyset cursor 语义，lot 列表保留 preview/live 互斥投影、
分类统计和旧分页形状，竞价日志保留 world/lot 筛选、编号页与完整 keyset cursor 分页，并以可选
来源字段区分论坛真实楼层和不暴露 fast/slow 子类型的 Web 代理事实，
PostgreSQL 完整档的价格历史保留旧筛选、成交条目、编号页 stats 与 cursor 页未知 total 语义，
SQLite Lite 因没有 settlement 事实链返回 `501 feature_not_supported`；P13/P14 补偿池
与单场账本已接入 PostgreSQL 完整档的 Serializable 快照、守恒/闭集校验、opaque cursor 和
旧分页响应，SQLite Lite 明确返回 `501 feature_not_supported`。Public session 已
使用 HttpOnly 会话 Cookie 与持久化会话表；P20 普通竞价已接入会话身份、稳定
`command_id`/`request_hash`、应用竞价事务和旧版结果投影。P16/P17 账户读取已接入 Public session、
应用端口及 PostgreSQL/SQLite 快照投影，覆盖普通竞价、代理 quota、转换历史、最近 proxy bid
和 owner capability；当前 `501 feature_not_supported` 只表达 SQLite Lite 或未装配依赖的明确
能力边界，不再代表 PostgreSQL 完整档的占位 handler。P18/P19 代理创建/降低命令已返回旧版
`{ authorization: ... }` 完整命令投影，quota、转换历史和终止原因由提交后的安全账户快照补齐，
并在授权状态版本不一致时失败关闭；代理重入的 `usage_number` 在 lot 锁内按用户历史最大值递增，
避免重入和命令重放混淆；Admin A06 lot 列表已接入
PostgreSQL Serializable 同快照的拍卖/lot/装备/投稿投影、Admin 会话保护和管理端表格，SQLite
Lite 因缺少完整生命周期字段明确返回 `501 feature_not_supported`。Admin 的 settlement-report
与 recovery-report 已接入 PostgreSQL 完整档一致只读/安全快照；前者保持旧版严格 `auction` 字段闭集，
并对序列化冲突/死锁最多重试三次，SQLite 明确返回 `501 feature_not_supported`。Admin 的 preview/open/cancel/
final-phase 已接入统一生命周期端口；preview 保持当前录入场次并物化目录，open 校验时间与前序状态后
原子建立后继 Draft、下一轮调度并切换录入目标，
cancel 只允许开放前执行，final-phase 在锁内转换可继续的 slow proxy 并冻结失格授权。Admin 请求使用状态版本 fence，取消要求原因，并在同一数据库事务内写入命令账本、状态版本、取消元数据、追加审计和
注册表内的 lifecycle realtime 事件；A03 GET/POST 拍卖已接入 Admin 会话、旧分页/筛选、PostgreSQL Serializable 生命周期/lot 数量快照、管理端表格和创建表单；其创建命令已接入零版本 fence、命令账本、world/intake target 锁、策略快照、审计和 `auction.created` 事件，主配置 `[auction]` 模板会同时写入 schema v4 快照与遗留规则列；SQLite Lite 创建与列表均返回 `501 feature_not_supported`。Public `/api/v1/events` 已接入 PostgreSQL/SQLite
持久化事件回放、sequence 游标和 schema 安全边界；Worker 已接入 `scheduled_rounds` 的持久化 due claim、
租约、尝试次数、preview/open/final 生命周期调用和失败退避；Auction 主循环还接入两后端数据库
时钟驱动的慢速代理 due 扫描、旧版稳定命令身份、事务内 generation/due/authorization/截止时间/资格
fence、stale 成功跳过和 `lot.slow_proxy_tick` 审计，并在 PostgreSQL 完整档接入结算
workflow 的 due claim、稳定 `settle_auction` 命令、精确拍卖/lot/代理锁、代理关闭、sold/unsold
结果、完成 fence 和失败退避；SQLite Lite 明确
不启用结算 workflow。公开端首页看板、lot 详情、规则文档、公开黑名单、补偿池/账本、论坛镜像、竞价日志和成交价格历史已接入同源 Public API；`/status` 按旧版仅显示静态服务说明。成交价格历史在 SQLite Lite 下显示明确的永久能力缺失状态，不把 501 伪装成可重试存储故障；
管理端 delivery 页面已接入状态筛选、数字分页、release/retry/reconcile 命令和旧版确认/对账交互，
操作携带 `command_id`、CSRF 和 fencing version；manual reconcile 的 succeeded/failed 决策分别
携带外部引用/失败代码。PostgreSQL 完整档还
接入结算后的履约 claim/fencing、delivery outbox 的持久化状态推进和重试边界；公开端拍卖看板
已按实时事件重新读取权威快照，并在权威重读后重建 sequence 基线、以周期回读兜底；管理端拍卖列表已提供带 CSRF、全新 `command_id` 和成功/失败反馈的
生命周期操作；基础通知与结算 MoogleMail transport、Sent/Balance 远程回执对账、论坛主题同步和
回帖竞价导入已接入；Public WebSocket 已补齐客户端事件信封/schema/连续游标校验，以及断档后
`refetch_required` 权威快照恢复；服务端 v1 连接边界已按 RT-002 静态恢复 4 KiB、50/60/5 秒时限、
稳定 close reason、discovery 专属容量、settled 预检和独立连接尝试限流；Public readiness 已纳入
relay owner 存活和消费新鲜度，真实帧 R1 仍待专用进程补录。
Public 登录错误码/封禁公开原因、页头会话菜单、登录 key 清理、
`/me` 竞价账户页映射、匿名登录 CTA、普通竞价/代理独立分页及登录/退出后的会话刷新已按旧版补齐；超出旧版 warning-only 边界的论坛命令扩展不属于等价范围。
Public 只读历史、黑名单、规则、补偿和论坛镜像页面已补齐请求失败重试入口；Web 剩余项收敛为
Admin 真实浏览器产物的逐页视觉/交互核对。
Admin A34-A37 delivery 列表已接入状态筛选和数字分页，release、retry 和 reconcile 已接入
PostgreSQL 完整档的安全投影、fencing version、独立命令账本以及结算履约状态联动；管理端页面
提供 release/retry 确认对话框和 manual_reconcile 的 retry/succeeded/failed 决策表单，成功决策
要求外部引用，失败决策要求失败代码；SQLite 明确返回 `501 feature_not_supported`。
这些接口只处理本地 outbox 的人工状态边界，不把未冻结的远程 MoogleMail 结果伪造成成功。Admin A14 recovery 确认已接入 PostgreSQL 完整档的 Serializable 版本 fence、
证据重校验、命令账本、退款激活、guarded relist、审计和预览目录 realtime 事件；SQLite 明确
返回 `501 feature_not_supported`。结算事务已经生成 recovery case；Recovery Worker 已接入逾期
case 推进、稳定 payment reminder action、delivery outbox 关联、结果证明、退避和人工复核边界，
并已补齐带 obligation UUID 的退回邮件证据绑定、evidence mismatch 阻断、ordinal recovery Take、
目标世界/来源邮件/资产摘要 fence，以及 Users owner 的幂等买家黑名单 action proof；relist/refund
和 resolution 收口已经由 Worker 持久化调度接入。旧版五类 recovery action 的静态闭集和分支均
已对齐，剩余是用真实外部回执验证未知结果与人工复核边界。Worker 实时消费游标和 Public
进程内 Hub 已对齐旧版；Web 页面主体已接入。
Admin A03 GET/POST 拍卖、A06 lot 列表、A15 代理历史和 A25 规则 GET/POST/PUT 已接入 PostgreSQL 完整档的一致安全投影、有限页码/opaque cursor、Admin 会话保护和管理端真实页面；A03 生命周期按钮现在按后端 capability 安全投影过滤，列表补齐预览/开放/结束时间线，A06 lot 页面接入真实页码分页、返回拍卖列表、加载失败重试、draft pending lot 改价/撤回确认、软截止信息、设备属性详情、金额/时间格式化以及代理历史关闭/重试交互；A03 POST 的策略字段现由主配置 `[auction]` 经应用层模板冻结，并同时写入快照与遗留规则列；A04 装备录入已接入 URL/canonical JSON 来源、短生命周期明文 key、AES-256-GCM 密文信封、PostgreSQL Serializable world/intake target/玩家/装备锁、同一事务的装备/密文/投稿/lot/版本/审计/realtime 写入，以及 Draft/Preview 当前录入场次的管理端表单；Preview 期间还会在同一事务追加新 revision 的物化展示项。密钥标识与材料只允许来自敏感 TOML，SQLite Lite 明确返回 `501 feature_not_supported`。A25 PUT 使用规则 revision CAS、命令账本和审计，管理端规则页已补齐旧版的语言标签、动态新增 locale、默认语言提示、版本事实、服务端规范化校验、校验门槛、轻量 Markdown 预览、稳定 command ID、发布冲突和重试反馈；SQLite 规则只读，发布明确
返回 `501 feature_not_supported`。A15 从命令账本恢复代理计划后只输出安全事件字段，不把原始结果负载或代理密钥传给浏览器。Admin A21-A24 realtime 运维接口已接入 PostgreSQL/SQLite 的 lag、failure 安全投影和带命令账本/审计的 replay
事务；外部广播 sink 作为超出旧版的扩展仍未冻结。源码目录重组已经完成，仍不引入外部服务行为或测试用例。
A05 论坛预览已接入 Admin session、origin/CSRF、2 MiB 请求体上限、应用端口、本地 HTML/文本
解析器和稳定 DTO；恰好一个输入字段会被解析为 catalog、posts、warnings，解析失败只返回
400，当前实现不抓取远程页面、不写数据库、不宣称远程发布成功。
A33 Admin 审计查询已接入应用端口、PostgreSQL/SQLite 追加式表读取、Admin session 和稳定 DTO；
保留严格精确筛选、时间范围、opaque keyset cursor 与数字分页，数字页的 total 与记录在同一
读取事务中取得，metadata 只允许安全 JSON object，不新增任何写入或测试用例。
A02 Admin dashboard 已接入 `auction-observability` 的心跳安全模型、PostgreSQL/SQLite
service_heartbeats 适配器、Admin session 保护，以及三个进程装配根的启动/就绪/周期和
SIGINT/SIGTERM 停止心跳路径，返回数据库中的 health/process 聚合；周期值现在每轮读取真实
数据库/schema readiness、进程内审计写入失败计数和各进程 tracing 安全状态；Public 还纳入 relay
任务与消费新鲜度，Worker 纳入逐主题论坛同步周期健康。Worker control
通过安全 runtime 目录、实例锁、固定
group/0660、inode 清理和显式 UID/GID peer allowlist 保护的 Unix socket 接入 live/ready/status，
无数据库启动骨架启用同一 control 时也只报告进程存活和零
poller，不伪造持久化就绪；Admin 现在只请求一次完整安全 dashboard snapshot，并按旧版顺序展示
health/process、Worker、tracing、realtime、mail、delivery、fulfillment、recovery、credential availability
和 alerting。credential scope、重启判断和其它增强诊断仍由内部 owner 计算，但不原地扩张旧 v1 dashboard
响应；外部广播仍不属于本地 dashboard 汇总。
Admin 结算页面已接入旧版 `/settlement` 的主要交互：已结算拍卖列表与分页、结算 lot 金额安全投影、
资金池摘要、recovery 记录和带 state-version fence/CSRF/command ID 的确认对话框。页面仍只消费
既有 settlement/recovery HTTP 契约，不把浏览器按钮状态当作写入授权；SQLite Lite 继续明确返回
`501 feature_not_supported`。
A13 Admin draft lot 修改已接入 `PATCH /api/v1/admin/lots/{lot_id}`：请求携带
`command_id`、`expected_state_version`、`action` 和可选 `start_price`，由 Admin session/CSRF
保护后进入 PostgreSQL Serializable 应用事务；事务按 world → auction → lot 顺序锁定并完成
版本 fence、命令账本重放、lot 状态/价格更新、预览 revision/state version、追加审计和
`auction.preview_updated` realtime 事件。withdrawn 动作同时创建 `withdrawn_return` 履约义务
及 pending fulfillment，SQLite Lite 明确返回 `501 feature_not_supported`；管理端 lot 页面已
提供 draft pending lot 的改价与撤回确认、失败重试、返回列表、软截止/设备属性详情和代理历史关闭入口。Admin lot 本地命令链已完成；外部履约仍需真实回执对照。

A16-A19 Admin 邮件首片已接入：`GET /api/v1/admin/mail` 返回指定邮箱的轮询安全状态，
`GET /api/v1/admin/mail/intake` 与 `/intake/{intake_id}` 返回严格筛选下的入站邮件安全摘要，
`PUT /api/v1/admin/mail/polling` 使用 `command_id`、`expected_state_version`、Admin session
和 CSRF 完成暂停/恢复。`auction-application` 持有邮件查询端口、版本 fence 和命令用例，
`auction-storage` 的 PostgreSQL 适配器在读取快照或 Serializable 写事务中访问
`mail_poll_state`、`inbound_mail` 和 `mail_admin_commands`，成功变更同时追加审计并支持命令
结果重放；原始邮件内容、外部 Cookie、轮询游标和任意 metadata 不进入 HTTP/Web DTO。管理端
邮件页面首次打开聚合全部已配置邮箱的入站记录，输入具体 mailbox 后再读取轮询状态，五种语言文案均已同步。SQLite Lite 没有该完整邮件
能力所需的表和外部协议，相关接口明确返回 `501 feature_not_supported`。

A20 `/api/v1/admin/mail/polls:trigger` 已接入 Admin session、CSRF 和单个 `command_id`；它
通过 Worker control 全局唤醒已装配的 MoogleMail poller，返回 `202 accepted`、命令 ID 和
poller 数量，不会把唤醒动作误报成已完成远程读取。Worker 随后让本轮各邮箱跳过下一次调度
时间，再通过持久化 lease、失败次数和退避执行实际 MoogleMail GET，外部读取结果再以安全
入站事实提交；SQLite Lite 仍明确返回 `501 feature_not_supported`。当前 MoogleMail 旧版代码
边界已覆盖邮件状态/审查、拍卖/用户业务处理、起拍价计划、拍卖入站事务、注册/重绑 Users
事务、版本化 delivery payload 和 Worker DeliveryTransport，协议层的 return/take/通知写入要求
结构化后置证明：return 优先把当前只读 `[RTS]` 回执与动作前身份、正文和完整资产逐项绑定，旧协议
则证明目标 `mid` 从收件箱完全消失；take 证明身份不变、资产归零与回执精确匹配。结算退信、
Sent/Balance 回执对账和 recovery 后续 action 已接入
PostgreSQL Worker；recovery 付款超时提醒由 recovery owner 接管，MoogleMail 协议层不拥有
recovery 业务 owner。旧版 `worlds`、发件人 allowlist、`equipment_filter` 与 14 个
`mail_templates` 现由 Admin 管理的共享 runtime TOML 持有，并在 Worker 启动时投影为业务快照。
Worker 主配置继续拥有 transport endpoint、凭据、调度、mailbox durable identity 和全局
`enabled`；旧 runtime 缺少三个业务区块时继续使用 Worker 主配置，避免兼容文件被默认值静默改写。
论坛扩展通知仍保留 Rust 固定文案，因为它不属于旧版模板闭集，后续若要配置化需先冻结跨模块契约。
MoogleMail 调度现已按 world 拆为独立循环，补齐旧版读取/可重试业务处理各三次的超时重试、指数
退避、周期抖动和全邮箱手动唤醒；Rust 的持久化 claim 在批次执行期间额外续租，失去 fence 后取消
陈旧处理，不能使用过期 token 推进 cursor。Worker 启动还会把配置缩短前遗留的过晚 `next_poll_at`
向当前 cron 收敛，同时保留暂停、活动 claim、失败事实和最大退避边界。应用错误已冻结
`Retryable`/`Permanent` 分类：确定性
业务、输入和配置错误不再 inline retry，超时及可重试依赖错误才进入有界重试。

A26/A27 Admin runtime 配置已接入：`auction-application` 的严格非敏感文档模型、稳定
fingerprint、版本化 request hash、pending/applied/unknown 恢复服务和窄端口已落地；Admin
对外就绪前会先执行同一恢复服务，可证明的命令自动收口，`unknown`/文件冲突保留人工核对
告警，其他恢复错误阻断启动；
`auction-config/src/runtime/` 负责 regular-file/权限/锁校验以及同目录 fsync + rename 原子
替换，`auction-storage` PostgreSQL 适配器负责 `runtime_config_admin_commands`、审计外键和
命令重放。`auction-http` 已接入 GET/validate/PUT，`auction-web-admin` 的 Config 页面已使用
JSON 编辑器、候选校验、旧版启用 world/装备过滤事实卡和重启提示，五种语言文案已同步。文档逐字段
覆盖旧版 main/isekai、发件人 allowlist、装备过滤和 14 个通知模板；三个业务区块使用可选顶层字段
兼容既有 Rust runtime 与 durable request hash，旧文件在浏览器读取时只物化投影，第一次保存才
显式写入。SQLite Lite 明确返回 `501 feature_not_supported`；runtime 更新不热改当前进程，文件
写入成功后必须重启受影响的 Admin 与 Worker。配置页
已补齐旧版的 JSON 对象解析提示、服务端候选规范化校验、校验签名门槛、稳定 command ID、
fingerprint 冲突反馈、重新加载和加载失败重试。
重启后的运行链也已收口：`allow_manual_early_open` 同时驱动 HTTP capability 和生命周期服务，
计划时间之前的人工开放在锁内按同一启动快照失败关闭，计划时间已到的开放与 Worker due 命令不受
影响；人工进入 Final 会把较晚的 live lot 有效截止时间收紧到冻结的最终窗口，默认 900 秒，且不
关闭后续竞价或软截止延期。新增的 `allow_manual_early_settlement` 默认关闭，只在 PostgreSQL 完整档
同时驱动 capability 与锁内授权；启用后 Admin 命令要求原因并把 live lot 截止时间收紧到事务时间，
再由既有 settlement scheduler/Worker 写入成交、义务与履约事实，不在 HTTP 生命周期事务复制结算
状态机。`[equipment_source]` 则映射为 host/path allowlist、响应上限、共享请求间隔和全请求超时，
适配器在固定 HTTPS origin、无重定向和精确 world 路径之上实施流式有界读取。Worker 只消费上述
业务区块，不把 Admin listener/security/session/metrics/tracing 投影进自身运行边界。
Admin runtime listener 已接入 TCP 与 Linux Unix socket 启动装配；Unix 模式会执行 runtime
目录、目录级单实例锁、stale socket 身份、owner/group 和 `0660` 权限检查，Worker control
socket 路径仍由同一 runtime 文档选择。`auctionctl deployment-check/start/restart` 已恢复跨进程
拓扑校验：Admin/Worker control socket 与三组数据密钥必须一致，Public/Admin 实际 TCP/Unix
listener 不能冲突，Admin readiness 跟随 runtime 选择的 listener；部署探针区分 Public/Admin
HTTP 的 `/health/ready` 与 Worker control 的 `/v1/health/ready`。同宿主机隔离 systemd/Caddy R1 已
证明顺序、探针、restart、drain、回执和失败回滚；真实域名/TLS、专用生产账号、正式 secret 与流量
切换仍需独立批准。
systemd Admin 模板也已开放 `/run/hv-auction`，Unix 示例 group 与模板运行组保持一致，避免代码
支持 Unix 而受保护 unit 无法创建 socket 的装配分叉。

A28/A30/A31 用户管理首片已接入：`auction-application` 声明用户安全投影、创建、改名、论坛账号
ID 修改、登录 key 撤销/重签、黑名单/解封和改名回滚端口；`auction-storage` 在 PostgreSQL
Serializable 与 SQLite `BEGIN IMMEDIATE` 事务内完成 state-version fence、命令账本、密文
key 信封、改名/黑名单追加历史和审计；`auction-http` 接入 `/users`、改名回滚和黑名单路由，
管理端 `/users` 页面已接入列表、创建、改名、筛选分页、黑名单历史/次数/创建时间展示、论坛账号 ID 修改、key 撤销/重签、黑名单与改名回滚
交互；创建用户改为旧版风格的对话框，自动生成的登录 key 和重签 key 都通过独立的一次性展示框交付，
不会出现在用户行反馈或列表中；论坛账号 ID 修改在 PostgreSQL 中会原子同步既有论坛映射，相关动作、key 和黑名单动作已补齐确认对话框及黑名单原因输入，并同步五种语言；A29 的改名历史列表同时返回当前用户版本，页面用该版本提交 A30
回滚命令。MoogleMail 注册/换绑仍只在 PostgreSQL 完整档，
Admin 用户管理的 Lite 支持不代表完整外部邮件能力已经对等。

A32 凭据管理首片已接入：`auction-application` 声明凭据种类、world/shared scope、secret
校验、命令摘要、版本 fence 和安全投影端口；`auction-integrations` 使用独立
`credential_data_key` 为 MoogleMail/E-Hentai Cookie 生成带类型域的 AES-256-GCM 密文信封。
PostgreSQL `000009_credential_admin_compat` 补齐 `forum_cookie` 闭集，并允许删除凭据后保留
命令账本与审计证据。`auction-storage` 在同一 Serializable 事务内完成导入、改名、启停、
删除的凭据行、命令账本和追加审计；`000030_single_active_external_cookie` 以失败关闭的迁移前置检查
和 partial unique index 保证 `mail_cookie`/`forum_cookie` 各自全局最多一条 active 记录，已有重复项
必须先通过上述 Admin 命令停用。`auction-http` 与 Admin 页面只接触安全投影，管理页补齐
kind/world/status 筛选、数字分页、改名对话框和删除确认，导入 secret 不会回显。Worker 配置独立密钥后可按 world/shared 读取唯一 active Cookie 并在内存解密，远端确认
登录失效时幂等回写 `invalid` 并从当前进程移除，不自动轮换到第二个 Cookie；凭据 owner 会按 kind/world
聚合 active/disabled/invalid、scope 更新时间和最近 Admin 变更时间，并结合 Worker 启动心跳判断是否
需要重启；旧 v1 dashboard 只公开由这些事实计算出的 MoogleMail `main`/`isekai` availability、观测时间
和最近 Admin 变更时间。未配置独立密钥时继续使用 `.keyenv`/敏感
配置兼容入口，其中 `.keyenv` 仅将 `hv-ck-key1` 用作 Worker Cookie，`hv-ck-key2` 只供显式隔离
验证；Admin mail section 在配置 `dashboard_mailboxes` 时还按 Admin runtime 投影核对全量
mailbox poller；
SQLite Lite 明确不提供该完整能力。仍需在真实外部服务和旧版数据上完成切换前
对照，尤其是写入开始后的未知结果边界。

## 对比旧版的当前复核矩阵

截至 2026-08-26 的重新审计结果如下：旧版业务 HTTP 路径和 Public/Admin 浏览器路由在 Rust 中没有
数量缺项，但静态注册不能证明行为等价。审计确认的参与资格锁、黑名单副作用和慢速代理 Worker owner
与实时 wire 已完成静态修复并等待同库/同部署 R1；旧版严格 JSON、业务方法 405、错误 `data: null`、
Public 规则语言读取和登录来源返回也已完成静态修复，其中 HTTP 边界与登录 CTA 取得局部 R1。论坛
两阶段恢复、邮件 retryability、Admin 业务 runtime 映射/稳定错误/凭据列和四项配置默认/安全边界也已
完成静态修复；同宿主机隔离 systemd/Caddy 生命周期也已取得 R1，其余 WebUI、外部服务和真实生产
切换证据仍按 `legacy-parity-audit.md` 逐项收口。下表把已完成代码、仍需外部/浏览器/生产证据的部分和明确不在旧版
等价范围内的扩展分开；静态复核不能替代行为验证。新增 `delivery_returned_mail` 只从
`recipient_not_found + take_required=true` 的人工对账事实派生，在原总量 signal 之外给出取回资产
提醒；PostgreSQL `000020` 扩展持久化闭集而不改写历史 fingerprint。

| owner | 旧版能力 | 当前状态与下一步 |
| --- | --- | --- |
| `auction-domain` / `auction-application` / `auction-storage` / `auction-jobs` | 预览、开放、Final phase、慢速代理 due、关闭与结算的完整生命周期 | Preview 可变录入、Open 后继轮次、物化预览、open 时间/前序约束、开放前取消、Final slow proxy 转换/失格冻结、持久化 Final due fence、结算前 due 校验、精确 lot/代理锁、代理关闭、sold/unsold 投影和 `leader_since` 连续领先语义均已落地；慢速代理恢复了数据库时钟扫描、旧版稳定命令 ID/请求摘要、事务内 generation/due/authorization/截止时间/owner 资格 fence、stale 成功跳过和 `lot.slow_proxy_tick` 审计。PostgreSQL `000012`/`000013` 与 SQLite `000005` 至 `000007` 补齐调度和历史投影，PostgreSQL `000014` 恢复身份不可变、前序顺序和初始 draft/intake target 防线，`000032` 再把唯一录入目标扩展到 Draft/Preview，并固定 Open 时才创建和切换下一期。Auction 生命周期、慢速代理与结算共享独立 `60s + [0,5s]` 主循环，三个阶段各自最多 100 条、PostgreSQL 8 并发波次、30 秒单项总时限，SQLite Lite 有效并发固定为 1 且不启用结算；一阶段错误不再跳过后续阶段或终止其它 owner。剩余是用同库旧数据对照边界时刻、并发重复扫描、锁等待和切换前运行证据。 |
| `auction-domain` / `auction-application` / `auction-storage` | 拍卖经济策略在竞价运行时的完整应用（PPM 增量、代理开关/默认模式/次数上限、final phase、旧版默认值和 intake 起拍价统一） | 已完成主要运行时切片：主配置 `[auction]` 经校验后冻结为 schema v4 `policy_snapshot`；PostgreSQL/SQLite 竞价锁内恢复完整 `AuctionRules`，普通/代理/慢速/最终阶段规划使用旧版 PPM 计算，公开 lot 列表/详情的下一口价也读取同一快照，代理禁用、重复进场次数、默认模式和 final phase 慢速限制已接入，取消也按同一拍卖快照读取截止规则。默认值已对齐旧版共享配置（起拍 1000、PPM 50000、软截止/延长 1 分钟、次数上限 3、final phase 1 分钟）；MoogleMail 起拍价默认继承 `[auction.economics]`，仍允许显式覆盖。剩余是与真实旧数据、外部服务及切换前行为的对照验证。 |
| `auction-application` / `auction-storage` / `auction-jobs` | recovery 退回后的 relist、refund、resolution 和完整 action 对账 | recovery 退回证据、Take、取消/恢复、COD 买家黑名单、guarded relist、buyer refund fulfillment 收口和状态审计已经接入；Worker 已对齐旧版 `case → return → action → relist → refund` 顺序，recovery action 也已补齐 `pending → prepared → enqueued` 两阶段 delivery identity、action fence 和可重放 activation。Fulfillment/Recovery 已从全局 5 秒 tick 拆为独立 `60s + [0,5s]` 循环，恢复 100 条批次、8 并发、60 秒 lease/重试和 30 秒单项时限；Recovery 各 phase 聚合失败但继续尝试后续 owner。prepare/activation 前按 Users owner 的 active 收件人投影复核资格，非 active 收件人持久化退避。旧版 recovery health 刷新也已接入：按 world 读取 Balance，持久化 low/stale/unavailable 状态并同步重算 recovery 计数；HV 战斗页另以 busy 状态保留凭据和最后成功余额，结束战斗后由后续刷新自动恢复。仍需更广泛的外部回执对照、真实部署对照和切换前验证。 |
| `auction-integrations` / `auction-application` / `auction-storage` / `auction-jobs` | MoogleMail main/isekai 独立轮询、读取/处理重试、超时和失败退避 | 每个已配置 world 现在拥有独立调度任务、持久化 cursor、claim 和手动唤醒器，一个邮箱的读取或数据库失败不会阻塞另一个邮箱，也不会终止 Worker。`schedule` 接受标准五字段 cron、描述符、`@every`、进程本地时区及 `TZ=` / `CRON_TZ=` IANA 时区前缀；现行产品默认 `*/5 * * * *`（每 5 分钟），再附加最多五秒抖动。共享 HTTP transport 恢复一秒最小请求间隔、15 秒请求超时、固定生产/回环安全 endpoint 和 8 MiB 响应上限；批次为 100 封，读取与可重试业务处理各三次、每次 30 秒，使用 `1s → 30s` 指数退避。读取只对暂时错误内联重试；业务错误由应用层显式分为 `Retryable`/`Permanent`，确定性输入、配置和业务拒绝立即以永久失败收口，超时与可恢复依赖错误才内联重试，游标和持久化退避不会提前推进。成功后按 cron 保存下一次 `next_poll_at`，失败后保存下一 cron 与退避时刻的较大值。Admin 管理的共享 runtime 覆盖 main/isekai、发件人 allowlist、装备过滤和 14 个模板；Worker 主配置继续拥有 transport、凭据、调度、mailbox identity 和全局启用开关。长批次处理期间按 10 秒周期续租 60 秒 claim，token/owner/version 或续租 fence 失效后取消陈旧 future；业务重试只通过显式短生命周期副本携带正文和装备 key，所有副本销毁时清零，稳定命令 ID 负责恢复可能已提交的本地步骤。剩余是用真实 main/isekai 邮箱对照分页、限流、Cookie 失效和写入不确定结果。 |
| `auction-integrations` / `auction-application` / `auction-storage` / `auction-jobs` / `auctionctl` | 论坛 `start`、`cancel`、PM `notify` 以及对应回执/不确定结果处理 | 经对照 Auction-BF 的 `forumingest`，`start`、`cancel` 和 PM-only `notify` 在旧版本身就是 warning-only，并未连接业务 owner；当前 Rust 保持该边界。MM `notify`、主题发布/读取和回帖竞价导入已接入；Forum 已从全局集成 tick 拆为独立、立即执行的顺序循环，默认五分钟加最多五秒抖动、100 条批次、300 秒逐主题 claim 和 240 秒单主题总时限；成功扫描把下一次 due 持久化到一个完整周期后，多 Worker 不会因 Auction 的一分钟循环提前重复领取。主题读取按同 origin/thread 的规范 `st` 补齐分页，默认最多 10 页且硬上限 20 页，超过上限、分页身份漂移或 post ID 重复时失败关闭；发布失败仍继续回帖扫描，任何一阶段失败都不会终止其它 Worker owner。回帖批次按 world 聚合，只有该批所有主题成功且命令账本完成写入后才推进扫描游标，已入账但通知暂未完成的命令会在后续轮次重放通知；主题正文现在可按 `worker.forum.public_base_url` 生成与旧版一致的公开拍卖绝对链接；发布超时或本地证明写入失败先进入 reconciliation-only：Worker 按精确标题 GET，命中时只补一份本地 topic 证明，未命中时停在 `write_authorization_required`；只有 `auctionctl authorize-forum-write-retry` 经逐字目标确认写入 PostgreSQL `000016` 幂等授权账本与审计后，下一次领取才可再次 POST，禁止盲重发。剩余是用隔离论坛账号核对分页/限流、同标题命中、未命中无授权和授权后单写的 R1。若要让这些 warning-only 命令真正改变论坛或拍卖状态，属于超出旧版等价范围的新契约，需另行冻结 owner、幂等账本和回执证明。 |
| `auction-application` / `auction-storage` / `auction-jobs` / `auction-http` / `auction-web-admin` | operations alert 信号、通知批次、Worker control 聚合和完整 dashboard section | 旧版 15 类 signal、阈值 evaluator、fingerprint、观察/恢复、批次 claim lease/fencing、三次 Create 尝试、MoogleMail 通知正文、入队原子 cooldown、稳定失败终态，以及 Admin 的 health/process、Worker、tracing、alerting、全量 mail poller、realtime lag/failure water、delivery、fulfillment、recovery 安全汇总已接入 PostgreSQL 完整档；`000015` 同时修复了普通重试写入会被 CHECK 拒绝、第三次失败误入人工对账和领取时过早消耗 cooldown 的差异。scan、dispatch、reconcile 已恢复为三个独立的一分钟有界循环，错误互不阻塞，阈值、批次窗口、冷却、重试和正文边界默认值与旧版一致。credential scope 聚合和无 active 凭据时的 degraded 启动语义也已接入，SQLite 明确为能力缺失。剩余是通知外部回执、真实部署和切换前验证；外部广播 sink 不属于旧版能力。 |
| `auction-observability` / `auction-process` / `auction-jobs` | Prometheus/OpenTelemetry metrics/tracing exporter、真实状态心跳与进程停机 | Public/Admin 已接入默认关闭、固定 `/metrics` 的进程本地 Prometheus registry、旧版同等的 CPU/内存/文件描述符/线程数/启动时间进程资源指标和安全身份指标；配置层已恢复启用 metrics 时有效 listener 必须是 loopback TCP 或 Unix 的旧安全边界，在绑定前拒绝非本地 TCP。Worker control 的 peer-authenticated Unix socket 也已提供同一 `/metrics` 路径，并注册旧版 9 类 Worker loop 的结果、耗时、批次、重试和调度延迟指标；Public/Worker 主配置与 Admin runtime 均已接入 OTLP/HTTP tracing exporter，使用角色专属敏感 headers、自有有界批处理和队列丢弃计数；三类进程已登记 PostgreSQL/SQLite 连接池当前借出、空闲、总数、最大连接数及 SQLx 存储池边界的十项 acquisition 指标；固定低基数的业务事件结果/耗时指标也已覆盖 Public 竞价/代理命令和 Worker 邮件、拍卖生命周期、履约、recovery、operations-alert 循环。当前追加审计 INSERT 均会累加进程内饱和失败计数；三进程周期心跳每轮复用数据库/schema readiness，读取该计数和各自 tracing 安全投影，启动/停止状态也不再使用占位值。三进程现已统一处理 SIGINT/SIGTERM、10 秒业务 drain、二次信号强制收口、停止心跳及配置有界的 exporter flush；绕过存储池和审计观测边界的外部原生调用不纳入对应累计统计。剩余是实际部署下的 exporter、心跳 TTL 与故障聚合证据。 |
| `auction-http` / `auction-web-*` | 旧版全部 Public/Admin 页面真实数据和交互 | Public 核心目录、世界切换、当前/历史拍卖、竞价/代理、账户、规则、黑名单、补偿、论坛、实时与价格历史，以及 Admin 登录、拍卖/lot、邮件、用户、凭据、规则、配置、审计、delivery、realtime、settlement/recovery 与 dashboard 页面均已接入真实 API；URL 状态、权威快照恢复、稳定 `command_id` 重试、五语言资源、移动端表格和主要旧版视觉交互已有源码 owner。Admin API 客户端现统一解析稳定错误 code 并通过五语言 Fluent 映射可操作反馈；配置页恢复旧版启用 world/装备过滤事实卡和业务 runtime JSON；凭据表按旧页面实际语义恢复 last checked/last used/imported by，并保留新版 version。SPA 入口现只提供给 14 + 21 条显式浏览器路由，未知 API/health 与页面 404 已隔离；Public/Admin 读响应及静态文件缓存头也按旧版语义和 Dioxus 稳定 WASM 文件名完成收口。HTTP/stream DTO 已统一由 `auction-contracts` 拥有，两个 `RustEmbed` 类型也已移入各自装配根形成制品级隔离；全部业务路由现恢复旧版严格 JSON、显式方法 405 和错误 `data: null`，Public 规则语言切换与登录来源返回也已有独立 owner。Public 14 条路径已完成 `en` 桌面匿名/空态巡检，规则与补偿池已有五语言浏览器 R1；剩余是补足动态数据行、有效登录与写动作、错误/冲突、移动端逐页状态，以及 Admin 配置保存/重启和其余已认证页面的视觉/交互 R1。 |
| `auction-integrations` / `auction-storage` | 凭据失效后的自动标记、单 Cookie 所有权和 dashboard 聚合 | 读侧 session 失效会回写当前 Cookie 为 `invalid`，写侧不盲重试且不会自动轮换；PostgreSQL `000030`、按 kind 的 session advisory ownership、MoogleMail/E-Hentai 接入和 Admin dashboard 聚合已完成。内部 owner 继续计算 scope 更新时间、Worker 重启状态并核对已配置 `dashboard_mailboxes` 的 poller 完整性；旧 v1 dashboard 只公开由这些事实导出的 MoogleMail `main`/`isekai` availability、观测时间和最近 Admin 变更时间。失效或替换后需要重启 Worker 释放旧 ownership 并装配新 Cookie；仍需真实外部服务对照和切换前行为验证。 |
| `auction-storage` / `auctionctl` / `auction-http` | 旧版 `/health/live`、`/health/ready` 探针与 `migrate-only` 对未知数据库的保护 | Public/Admin 两类兼容健康探针已补齐；`ready` 只读检查数据库、全部内嵌迁移和进程内审计写入完整性，持久化 service heartbeat 复用同一进程 readiness；`auctionctl migrate` 在没有 SQLx 迁移标记但检测到用户对象时失败关闭，不会把未知数据库初始化为 Auction 数据库。同宿主机隔离 PostgreSQL systemd/Caddy 已证明两个 surface 的 live/ready 200、controller 编排和上游停止后的 502；SQLite Lite 与真实生产环境仍需各自的部署证据。 |
| `apps/auctionctl` / `auction-storage` / `auction-integrations` | 旧版 Worker `-bootstrap-mail-credentials`、`-bootstrap-mail-world`、`-bootstrap-mail-label` 一次性登记邮件凭据 | 已补齐 `bootstrap-mail-credentials --world --label`：Cookie 只从标准输入读取，限制为 `main`/`isekai` 且要求 Worker 已启用对应 world，复用凭据数据密钥和 Admin 凭据服务写入 PostgreSQL；命令不回显 Cookie、密文或密钥。仍需在真实部署中按目标 secret 注入方式做一次切换前演练。 |
| `ops` / `apps/*` | 旧版 `ops/deploy` 的启动脚本、Caddy 反向代理、环境示例、数据库备份/恢复/保留资料 | 已补充当前三进程/TOML/健康探针的部署目录、启动顺序、安全权限、Caddy 反向代理模板、systemd 单角色与总控示例、`auctionctl deployment-check` 只读预检，以及 Rust `auctionctl deployment start/stop/restart/status` 总控：总控验证 unit 与 TOML 绑定，执行严格预检、地址检查和 forward-only migration，按 Worker → Admin → Public 逐角色等待应用 readiness，失败反序回滚本次启动的 unit，并用 flock 与 systemd InvocationID 发布私有启动回执。PostgreSQL retention、recovery preflight、backup/verify、restore/rebuild 和 canonical v28 接管证据链均已有 owner。部署模板继续由 systemd 发送 SIGTERM；Public/Admin 的共享 Hyper 连接服务会停止接收并排空现有连接，Worker 同时响应 SIGINT/SIGTERM，三进程执行 10 秒有界 drain、停止心跳和 tracing flush，单角色 unit 以 45 秒上限覆盖最长 exporter 关闭余量。旧版 retention 本身明确只做 inventory、禁止 mutation，完整 retention mutation 不是旧版漏迁。 |
| `auction-storage` / `apps/auctionctl` / `ops` | canonical Auction-BF PostgreSQL v28 的现有数据接管 | 专用 v28 backup/verify/adopt 链路已完成：只接受 PostgreSQL 18.x、干净 migration 28、精确 canonical 表集合、零其他 session 和可无损恢复的竞价投影，要求专用恢复点、逐字确认和不可覆盖 receipt；执行时在同一持锁物理连接复核完整预检，再在事务 bridge 中建立 SQLx `1..=5`、`7` 等价基线并继续前向迁移。`000014` 统一生命周期约束并移除旧论坛隐式入队 trigger，`000015` 归一化 operations alert 重试/终态/cooldown，并保留合法的历史终态 delivery 孤儿；`000016` 增加论坛未知写入的人工再授权命令账本。普通 `migrate`/`restore` 不会猜测处理。2026-08-26 已在旧对照 PostgreSQL 18.6/canonical v28 的隔离副本上实际完成专用 backup/verify、恢复点复原、adopt 到 SQLx 16、五语言规则导入、当前格式 backup、同名 rebuild、空库证明和单事务 restore；恢复前后规则摘要、审计与迁移计数一致。旧副本没有 auction/lot/bid/user/realtime 业务行，因此仍需非空旧业务数据抽样和真实部署切换。 |
| 验收与切换 | 双跑、对照、双写、切换前行为验证 | 当前按用户授权继续 PostgreSQL 完整档、SQLite Lite 边界和旧版的隔离人工对照，并已完成 Auction-Test 中 MoogleMail main 的最小 Credits/装备真实写入；仍不编写或运行测试，论坛/其它外部写入、双写和生产流量切换需逐项独立批准。 |

SQLite Lite 的 PostgreSQL 专属 recovery、MoogleMail、论坛完整档、结算报告和管理完整写入不属于
“漏做后补齐”的任务，而是已声明的部署能力边界；不得用空结果或内存状态伪装成已实现。

### 旧版等价收口与剩余证据

截至 2026-08-26，旧版 HTTP 路径和浏览器路由在静态清单上没有数量缺项；重新从实际消费链、
进程边界和项目规范交叉审计后，以下内容记录已完成的本地代码收口、
明确的部署能力边界，以及仍需真实环境证明的等价证据。
三进程 SIGINT/SIGTERM、有界 drain、停止心跳和 tracing flush 已完成。Public/Admin 共用 HTTP
boundary 也已完成：请求 ID 与命令审计关联、W3C trace context、访问日志、panic recovery、
安全响应头、普通请求/5 秒 read-header timeout、可信代理地址解析，以及 4096 项确定性淘汰限流器
均已接入。显式浏览器入口、API/页面 404、GET/HEAD 静态方法边界及
`no-cache`/`no-store`/`immutable` 缓存矩阵也已恢复。周期心跳同样已改为每轮读取真实数据库/
migration readiness、audit failure 和 tracing health；Admin runtime 的人工提前开放与装备来源
策略也已进入实际运行 owner。Web HTTP/stream DTO 已统一收敛到 `auction-contracts`，Web crate 只
保留展示状态；Public/Admin 的 `RustEmbed` 类型也已分别移入对应装配根，`auction-http` 只接收
泛型资源类型，因此制品级隔离不依赖链接器裁剪。以上项目均不再列入未完成项：

首批可重建缓存读模型也已完成：Public 规则按语言接入有限 TTL/get/set，命中快照经过应用校验，
Admin 规则发布成功后执行精确 invalidate；缓存故障、损坏或失效失败始终回源权威规则表，且
不进入竞价、会话、幂等或审计正确性路径。

Worker control transport 的所有权已从 `src/worker/` 调度目录移至 `auction-jobs/src/control/`：
协议 DTO、Unix socket 安全、peer allowlist、实例锁、清理和 Admin client 现在由 control 子模块
统一维护，调度器只保留 `WorkerControlHandle` 状态/唤醒端口；该传输不接受业务写入，也不复用
Public/Admin HTTP listener。

- 本轮已完成 HTTP/config settings/Worker runtime/Worker bootstrap/Public Web surface、HTTP DTO、observability metrics、Users/Admin
  storage backend、auctionctl runtime 入口、auction-http routes、MoogleMail parser/transport、
  E-Hentai client、MoogleMail application service、operations alert ports、Public auction reads 以及
  storage account/lots/operations/recovery/realtime、competition PostgreSQL/SQLite transaction、recovery
  commands/scheduler、settlement、credentials、fulfillment scheduler、retention inventory、legacy v28、
  delivery scheduler、Worker control transport、equipment source、Admin Dashboard、Admin 拍卖/用户/结算页面、Public 拍卖目录、
  MoogleMail poller、mail poll state/poll/inbound、delivery Admin reads/commands/mapping、Admin runtime document/command/ports/validation、Worker validation 的 Auction/Delivery/Fulfillment/Recovery/control/MoogleMail/Forum/Operations Alert、recovery action、PostgreSQL competition Admin、observability telemetry、realtime Admin replay、Public price history、sessions Admin/Public、scheduled-round claim/resolution、settlement workflow claim/resolution、PostgreSQL/SQLite 生命周期事务、Users/Admin PostgreSQL/SQLite、Users Mail 以及 Admin Mail PostgreSQL 适配器的具名文件拆分，并修复此前因错误使用 `$$` 而无法执行的
  `layout-check`，新增每个 src 必须存在职责子目录、源码命名、同前缀聚集、配置注释和五语言
  Fluent 键集合门禁；子目录仍不能只成为巨型 `mod.rs` 的外壳。
- Public HTTP handler 已进一步按 owner 拆入 `routes/public/session.rs`、`account.rs` 和
  `competition/{bid,proxy}.rs`，`routes/public/competition/mod.rs` 与 `routes/public/mod.rs` 只保留
  模块装配与稳定导出；竞价/代理的认证、request hash、command ID、错误状态码和 DTO 契约未改变。
- Worker 配置模型已进一步按 Auction、control、Delivery、Fulfillment、Recovery、Forum、
  MoogleMail 和 Operations Alert 拆入 `config/settings/worker/`，`worker/mod.rs` 只负责装配与
  稳定导出；TOML 字段、默认值、校验入口和公共类型路径未改变。
- Worker Recovery loop owner 已将 cadence/阶段顺序、health、case、return、action、relist/refund
  resolution 和结果指标分别拆入 `worker/recovery/loop/{mod,health,cases,returns,actions,resolutions,common}.rs`；
  claim 顺序、每阶段 deadline、并发波次、lease/重试和 WorkerResult 合并语义保持不变。
- Config 通用设置 owner 已将进程角色、HTTP listener、缓存策略、拍卖经济/结算/恢复策略和
  Metrics/tracing/realtime 分别拆入 `config/settings/auction/{role,http,cache,policy,observability}.rs`；
  `auction/mod.rs` 只负责稳定导出，TOML 字段、默认值、`Role::default_listen` 和策略模板转换语义保持不变。
- Settlement domain 已进一步按 policy、amount、facts、obligations 和 plan 拆入
  `auction-domain/src/settlement/`，`settlement/mod.rs` 只负责稳定导出；策略校验、金额守恒、
  结算曲线与义务闭集规则保持原实现。
- Competition domain 已进一步按触发器模型与竞价规划拆入 `auction-domain/src/bidding/competition/`，
  `competition/mod.rs` 只负责稳定导出；普通竞价、代理、慢速/最终阶段、软截止和版本校验规则保持原实现。
- Competition domain 的规划器已继续拆入 `competition/plan/`：`mod.rs` 只保留公共结果与规划入口，
  `validation.rs`、`ordinary.rs`、`proxy.rs` 和 `state.rs` 分别负责输入/结果校验、普通竞价、代理生命周期
  和状态/软截止收口；`CompetitionPlan` 公共路径与领域规则保持不变。
- `auctionctl` deployment control 已将 Unix/systemd 生命周期实现移入
  `bootstrap/deployment_control/unix/mod.rs`，根 `mod.rs` 只保留宿主机兼容入口和参数边界；启动顺序、
  readiness、回滚、状态与回执契约保持不变。
- `auction-storage/src/database/mod.rs` 已将后端错误与 URL 识别、Store/迁移/readiness、连接池指标、
  PostgreSQL 连接池和 SQLite 连接池拆入 `backend.rs`、`store.rs`、`pool/{mod,metrics}.rs`、
  `postgres_store.rs` 与 `sqlite_store.rs`；根文件只保留模块装配、兼容导出和既有根级测试，
  `DatabaseBackend`、`DatabaseStore`、`PostgresStore` 与 `SqliteStore` 的公共路径不变。
- Admin Web surface 已将会话状态编排、站内 return 路径、登录/认证外壳和重试/刷新分别拆入
  `app/surface/auth/{mod,redirect,session,login,shell}.rs`，跨页面状态面板、错误重试和
  时间格式化移入 `app/surface/shared.rs`；surface 装配与资源初始化位于 `app/surface/mod.rs`，路由表和
  页面入口包装器位于 `app/surface/router.rs`，Admin 路由与组件契约不变。
- Admin API 启动装配已将 tracing/HTTP 生命周期拆入 `apps/admin-api/src/bootstrap/service.rs`，
  状态入口保留在 `state.rs`，数据库完整档、无数据库 feature 的降级装配和 heartbeat 分别拆入
  `state/{database,fallback,heartbeat}.rs`；缓存、Worker control、安全策略和 dashboard 范围投影
  拆入 `config.rs`。`bootstrap/mod.rs` 只保留模块装配和稳定启动入口，监听器与退出契约不变。
- Public API 启动装配已将 tracing/HTTP 生命周期拆入 `apps/public-api/src/bootstrap/service.rs`，
  数据库、端口、heartbeat 和 realtime 状态拆入 `state.rs`，缓存与安全策略拆入 `config.rs`；
  `bootstrap/mod.rs` 只保留模块装配和稳定启动入口，启动顺序、监听器、优雅退出 drain 与
  PostgreSQL/SQLite feature fallback 契约不变。
- `auctionctl` PostgreSQL rebuild 已将授权/参数模型、破坏性执行、确认与 URL 校验、空库/迁移后置证明
  以及不可覆盖回执发布分别拆入 `postgres_rebuild/model.rs`、`execute.rs`、`validation.rs`、
  `postconditions.rs` 和 `receipt.rs`；`mod.rs` 只保留 `Options`、`Mode`、`execute` 的稳定调用路径，
  删除前预检、advisory lock、失败回执和迁移后置契约保持不变。
- `auction-observability/src/health/` 已将安全健康模型与校验、心跳发布/持久化端口、三进程 dashboard
  聚合分别拆入 `model.rs`、`publisher.rs` 和 `dashboard.rs`；`health/mod.rs` 只保留模块装配和稳定导出，
  心跳 sequence、TTL、readiness 与安全投影契约保持不变。
- `auction-observability` telemetry processor 已将 exporter 健康包装与有界 span 队列/批处理/关闭控制
  分别拆入 `telemetry/processor/{mod,exporter,batch}.rs`；OTLP 丢弃计数、flush、shutdown 超时和
  runtime 的稳定类型路径保持不变。
- Application Admin credentials port 已将闭集模型/分页、安全值与密文信封、Worker 运行记录、dashboard 聚合、
  业务错误、敏感命令与写入载荷、查询/加密/Worker runtime/dashboard 端口分别拆入
  `ports/admin/credentials/value/{model,secret,runtime,health,error,common}.rs`、`commands.rs` 和 `ports.rs`；
  `credentials.rs` 只保留模块装配和稳定导出，明文清零、密文封装和幂等 request hash 语义保持不变。
- Application auction lifecycle service 已将服务状态/命令幂等事务编排、状态与 final proxy 计划、后继轮次与
  稳定 schedule ID 分别拆入 `services/auction/lifecycle/{mod,plan,schedule}.rs`；Serializable 事务、
  expected state-version fence、命令账本 replay、人工提前开放策略和 final proxy 资格冻结语义保持不变。
- Application MoogleMail processing port 已将装备过滤、短生命周期邮件 envelope、拍卖入站写入、delivery
  outbox、业务决定和共享安全错误分别拆入 `ports/mail/processing/{filter,message,auction,delivery,decision,common}.rs`；
  `processing.rs` 只保留模块装配和稳定导出，正文/装备 key 清理、密钥封装前置和无密文 delivery 边界保持不变。
- Admin dashboard HTTP adapter 已将认证后请求编排、各 owner section 聚合和稳定 response DTO 映射拆入
  `http/admin/dashboard/`；旧 v1 顶层、section 与 data 键闭集由映射层显式维持，known/degraded 不携带
  `error_code`，unknown 只携带稳定错误码，内部增强诊断不泄漏到同一路径。
- Admin Web 凭据页面已将页面加载/删除编排、录入与筛选、列表/状态/改名操作、删除确认和标签/幂等辅助分别拆入
  `app/identity/credentials/{page,form,table,dialogs,helpers}.rs`；`credentials.rs` 只负责组件装配和
  `page` 公共出口，secret 短生命周期与五语言文案边界保持不变。
- Public Web 竞价命令组件已将普通竞价、代理入口/创建表单/降低表单/成功反馈和共用金额提示/命令状态/错误映射分别拆入
  `app/auction/commands/{bid,proxy/{mod,create,authorization,success},common}.rs`；`commands.rs` 只保留稳定的 `BidFormInput`/`ProxyFormInput`
  出口，command_id、服务端资格与版本 fence、焦点/关闭交互和五语言文案保持不变。
- Application MoogleMail 处理器已将主题路由/注册换绑、装备读取过滤/拍卖入站和 delivery outbox 编排分别拆入
  `services/mail/processing/processor/{routing,auction,delivery}.rs`；`processor.rs` 只保留处理器配置、共享
  owner 状态和 `MailProcessor` 出口，幂等 request hash、密钥清理、事务 owner 与外部提交后置边界保持不变。
- Application MoogleMail Users port 已将结果闭集、登录 key/密文信封、注册/换绑载荷和存储/签发/读取
  窄端口分别拆入 `ports/mail/user/{mod,model,security,commands,ports}.rs`；稳定导出、明文清理、
  密文封装和 Users owner 的幂等边界保持不变。
- Storage MoogleMail 拍卖入站 owner 已将端口适配、装备/拍卖写入和审计/realtime 记录分别拆入
  `database/mail_intake/{mod,intake,record}.rs`；命令账本、world/auction/玩家锁、密钥信封、pending lot、
  预览版本和 preview 事件仍由同一个 PostgreSQL 竞价事务原子提交，明文邮件正文与装备 key 边界保持不变。
- MoogleMail gateway 已将单 Cookie 会话/失效收口、配置/读取结果模型和 inbox/Sent/Balance 有界读取分别拆入
  `clients/mooglemail/gateway/{credentials,model,reader}.rs`；`gateway.rs` 只保留稳定公共导出与错误辅助，
  Cookie、world scope、allowlist、分页 cursor 和摘要/详情一致性证明边界保持不变。
- MoogleMail parser reads owner 已将 inbox 分页、Sent 分页、Sent 详情附件/CoD 证明和 Item Shop 余额
  分别拆入 `clients/mooglemail/parser/reads/{mod,inbox,balance}.rs` 与
  `clients/mooglemail/parser/reads/sent/{mod,detail}.rs`；跨页 ID 去重、
  摘要/详情身份一致性、脚本与可见余额一致性和敏感字段清理语义保持不变。
- MoogleMail parser script owner 已将 JavaScript 遮罩/脚本值提取与正文/脚本装备 key、严格 JSON 访问分别拆入
  `clients/mooglemail/parser/script/{mod,javascript,equipment}.rs`；重复字段、伪造变量、身份不一致、
  key 格式和字段缺失仍按协议漂移或身份不匹配失败关闭。
- MoogleMail parser type owner 已将收件箱/详情 envelope、Sent 快照和 Item Shop 余额分别拆入
  `clients/mooglemail/parser/types/{mod,message,sent,balance}.rs`；敏感字段脱敏 Debug、正文/key 清理、
  content digest、remaining/collected 一致性和各项字段上限保持不变。
- MoogleMail writer 已将 compose 状态模型、共用安全解析辅助、staged compose 证明和最终发送响应分类分别拆入
  `clients/mooglemail/writer/{model,common,compose,response}.rs`；`writer.rs` 只保留稳定公共导出，token
  生命周期、资产/身份 fence、write-unknown 和成功/明确失败证明保持不变。
- Storage PostgreSQL rebuild adapter 已将公开预检/结果/错误模型、catalog 采集与安全预检、advisory-lock
  guard/破坏性执行、动态标识符与 locale SQL quoting 分别拆入 `database/rebuild/{model,catalog,guard,sql}.rs`；
  `rebuild/mod.rs` 只保留稳定导出，删库前保护、mutation 状态和重建后 OID/定义校验保持不变。
- Application Public query ports 已将世界/拍卖目录、lot/装备投影、账户代理、竞价日志和成交价格历史
  分别拆入 `ports/auction/queries/{world,auction,lot,account,bid_log,price_history}.rs`；
  `queries.rs` 只保留稳定公共出口，分页、keyset cursor、公开投影和安全字段边界保持不变。
- Application Public compensation port 已将补偿池批次摘要/分页与逐笔 movement/余额链校验分别拆入
  `ports/public/compensation/{mod,movement}.rs`；稳定导出、守恒约束、编号页和 keyset cursor
  边界保持不变。
- PostgreSQL 生命周期计划已将 preview/open/cancel/final phase 的事务写入与后继 Draft 建立分别拆入
  `database/competition/postgres/lifecycle/plan/{preview,open,cancel,final_phase,successor}.rs`；
  `plan.rs` 只保留动作分派，状态版本 fence、Preview 持续录入、Open 接管、锁内物化/lot 转换、
  取消返还义务和代理子计划写入保持不变。
- SQLite Lite 生命周期计划也已将 preview/open/cancel/final phase 的 `BEGIN IMMEDIATE` 内写入与后继 Draft
  建立分别拆入
  `database/sqlite/competition/transaction/lifecycle/plan/{mod,preview,open,cancel,final_phase,successor}.rs`；
  `plan/mod.rs` 只保留动作分派和共享状态版本校验，Lite 的串行写入、Open 后继接管、lot 状态转换、
  scheduled round fence 和 final phase 代理子计划原子性保持不变。
- Forum 预览解析已将 HTML 抽取、纯文本切分、目录字段、命令/金额语法、内部帖子模型和结果辅助分别拆入
  `src/forum/parser/{html,text,catalog,commands,model,helpers}.rs`；`forum/mod.rs` 只保留适配器
  与稳定出口，A05 的有界输入、warning 排序和“只解析不远程写入”边界保持不变。
- Admin 邮件入站页已将页面筛选/加载、轮询动作、入站表格、详情投影和状态标签分别拆入
  `app/fulfillment/mail/{page,actions,table,detail,helpers}.rs`；`mail.rs` 只保留页面出口，
  五语言文案、闭集筛选立即生效、分页、command ID 和安全详情投影保持不变。
- PostgreSQL recovery command 的范围 owner 已将数据库行模型、命令回放、锁定读取和状态/证据校验分别拆入
  `database/recovery_commands/commands/scope/{model,replay,locks,validation}.rs`；`scope.rs` 只保留
  内部稳定出口，Serializable 锁定顺序、命令 replay fence、证据闭集和退款激活语义保持不变。
- PostgreSQL recovery command 的写入推进已将 Admin 确认、Worker relist、buyer refund 和共享 fence/审计辅助
  分别拆入 `database/recovery_commands/commands/confirm/{command,relist,refund,common}.rs`；
  `confirm.rs` 只保留稳定出口，Serializable 重试、锁定顺序、状态版本和审计语义保持不变。
- Worker recovery action runtime 已将 action dispatch、prepared delivery 激活、买家黑名单、外部结果观察和持久化
  重试释放分别拆入 `worker/runtime/recovery/{dispatch,activation,blacklist,observation,retry}.rs`；
  `runtime/recovery.rs` 只保留稳定出口，`pending → prepared → enqueued`、action fence 与
  `case → return → action → relist → refund` 顺序保持不变。
- Storage recovery delivery action owner 已将 prepared 关联、enqueued 激活和外部结果/证明收口分别拆入
  `database/recovery_scheduler/scheduler/actions/delivery/{mod,prepare,activation,observation}.rs`；
  `delivery/mod.rs` 只负责装配，`pending → prepared → enqueued`、claim fencing、proof mismatch/manual reconcile
  和终态 state-version fence 语义保持不变。
- `auctionctl` Unix/systemd deployment control 已将跨角色生命周期编排、Controller 锁/预检/readiness/回滚操作、
  部署计划状态模型和路径、配置、unit 环境校验分别拆入
  `bootstrap/deployment_control/unix/{lifecycle,controller,model,validation}.rs`；
  `unix/mod.rs` 只保留父控制器包装，Worker → Admin → Public 启动、Public → Admin → Worker 停止、readiness、回滚和
  startup receipt 契约保持不变。
- Worker 论坛回帖导入已将主题/身份与 lot 映射、竞价命令账本、MoogleMail 通知和完成/错误辅助分别拆入
  `worker/forum/reply/{topic,command,notification,common}.rs`；`reply.rs` 只保留运行时与循环入口，
  每主题持久化调度/健康、稳定 command ID、warning-only 通知和外部 POST 结果未知边界保持不变；
  当前运行路径不再维护 world 级扫描游标。
- `auctionctl` canonical PostgreSQL v28 接管已将专用备份生成、离线 archive/manifest 校验、接管编排、
  预检/迁移后置证明、证据模型和不可覆盖回执分别拆入
  `postgres_legacy_v28/{backup,verification,adoption,common,model,receipt}.rs`；`mod.rs` 只保留稳定出口，
  写库前二次 artifact 校验、逐字确认、失败关闭和 receipt 发布顺序保持不变。
- `auctionctl` PostgreSQL artifact owner 已将外部命令环境、manifest/hash、文件系统安全、pg_dump/restore
  进程、来源预检和 stdin Cookie 输入分别拆入
  `runtime/postgres/artifacts/{environment,manifest,filesystem,process,source,input}.rs`；`artifacts.rs`
  只保留原有稳定导出，备份不可覆盖、passfile 清理、SHA-256 证据和外部命令脱敏边界保持不变。
- `auctionctl` PostgreSQL command owner 已将 MoogleMail 凭据引导、retention/recovery 只读预检、备份/manifest
  和 restore/legacy-v28/rebuild 专用 owner 桥接分别拆入
  `runtime/postgres/commands/{mod,bootstrap,preflight,backup,delegation}.rs`；命令名称、PostgreSQL 能力缺失
  错误、确认门槛、退出码和私有 receipt/证据边界保持不变。
- `auctionctl` PostgreSQL restore owner 已将恢复编排、备份 pair/空库/恢复后 schema 证明、pg_restore 外部进程和
  不可覆盖 receipt 分别拆入 `bootstrap/postgres_restore/{mod,verification,process,receipt}.rs`；预检后关闭自身
  连接、TOCTOU 二次校验、single-transaction 参数和失败后保持角色停止的契约保持不变。
- Storage PostgreSQL recovery preflight owner 已将预检结果/错误模型与只读事务策略、schema identity/迁移历史
  查询分别拆入 `database/recovery_preflight/{mod,model,query}.rs`；`mod.rs` 只保留单一
  REPEATABLE READ/READ ONLY 事务编排，数据库身份校验、空目标判断和恢复前安全边界保持不变。
- Worker realtime consumer failure owner 已将失败记录与 replay 收口分别拆入
  `database/realtime_consumer/failure/{mod,record,resolve}.rs`；`failure/mod.rs` 只保留稳定导出，
  PostgreSQL 行锁、SQLite Lite 串行事务、stream 归属校验、重放序号 fence 和失败状态幂等语义保持不变。
- Operations alert claim owner 已将新批次领取、过期租约回收和 PostgreSQL 行映射分别拆入
  `database/operations_alert/claims/{mod,create,reclaim,mapping}.rs`；claim token/fencing、三次重试
  上限、delivery 准备审计、正文摘要校验和目标 world/recipient 身份 fence 保持不变。
- Operations alert snapshot owner 已将 PostgreSQL process/delivery/fulfillment/recovery 水位读取与
  signal 观察事务分别拆入 `database/operations_alert/snapshot/{mod,reads,observe}.rs`；各 source
  独立降级、signal fingerprint/state-version fence 和观察事务提交边界保持不变。
- `auction-http` 共享会话适配器已将限流/认证守卫、应用错误映射、Cookie/Basic Auth/CSRF、安全响应
  和业务指标分别拆入 `routes/session/{guards,errors,cookies,responses,metrics}.rs`；`session.rs` 只保留
  模块装配与稳定出口，原有 session、CSRF、Cookie 属性、状态码和业务结果分类保持不变。
- `auction-http` Admin route owner 已将会话、生命周期命令、结算/恢复报告和 recovery 人工确认分别拆入
  `routes/admin/{session,lifecycle,reports,recovery}.rs`；`routes/admin.rs` 只保留稳定导出，原有
  Basic Auth、CSRF、command ID、版本 fence、状态码和安全投影边界保持不变。
- `auction-http` routes 根装配已将 Public/Admin 路由表与健康探针分别拆入
  `routes/surfaces.rs` 和 `routes/health.rs`；`routes/mod.rs` 只保留 HTTP 子模块、共享导入和稳定导出，
  health/live、health/ready、SPA fallback、缓存层和所有冻结业务路径契约保持不变。
- Public realtime HTTP owner 已将事件列表读取、WebSocket 连接、目标解析和事件校验/DTO 映射分别拆入
  `http/public/realtime/{mod,list,websocket,target,mapping}.rs`；`mod.rs` 只保留稳定 handler
  出口，序号连续性、refetch_required、Public session、连接上限和 `/api/v1/events`/`/api/v1/ws`
  契约保持不变。
- Public Web API 根适配器已将服务端时间校准/GET 边界、lot 展示模型和拍卖/lot/竞价日志/价格历史
  查询分别拆入 `api/{clock,model,queries}.rs`；`api/mod.rs` 只保留模块装配与稳定导出，Date
  校准、分页参数、公开 DTO 映射和请求路径契约保持不变。
- Storage service heartbeat 适配器已将 `HeartbeatStore` 分派、PostgreSQL 完整档、SQLite Lite
  实现和闭集字段/错误映射分别拆入 `database/health/{mod,postgres,sqlite,mapping}.rs`；
  observation sequence fence、数据库时钟/本地时钟、TTL、readiness 和安全 details 边界保持不变。
- Worker runtime 根装配已将 `PersistentWorker` 端口/运行参数模型和启动、任务装配、drain/退出循环分别拆入
  `worker/runtime/{model,runner}.rs`；`runtime/mod.rs` 继续保留跨 owner 的共享 helper 与稳定导出，
  Worker control 状态、任务启动条件、排空顺序和失败关闭契约保持不变。
- Storage PostgreSQL settlement scheduler 的 claim owner 已将稳定 workflow/command identity、lot 锁与 due
  世代校验、审计与 realtime 证据分别拆入 `database/settlement_scheduler/claim/{mod,ids,locking,evidence}.rs`；
  `claim/mod.rs` 只保留领取编排，`SKIP LOCKED`、租约 fence、state-version、幂等账本和事务提交边界保持不变。
- Worker control server owner 已将 server 生命周期、peer credential 过滤监听和 runtime 目录/实例锁/socket
  身份安全分别拆入 `control/transport/server/{mod,listener,security}.rs`；Unix socket 单监听器、
  UID/GID allowlist、inode 清理、group/0660 权限和优雅关闭契约保持不变。
- Worker MoogleMail poll processing owner 已将 claim/续租/批次提交编排、远程 fetch retry 和单封业务 retry
  分别拆入 `worker/mail/poll/processing/{mod,fetch,business}.rs`；cursor/lease fencing、失败退避、
  stable command ID 和批次 commit 顺序保持不变。
- Worker bootstrap 的 MoogleMail owner 已将凭据恢复、processor/delivery 装配、world poll runtime/gateway
  和消息 envelope 映射分别拆入 `apps/worker/src/bootstrap/mail/{mod,credentials,processor,runtime,mapping}.rs`；
  world 配置、单 Cookie 失效、poll cursor/lease、外部错误分类和业务消息字段保持不变。
- Contracts Public catalog owner 已将拍卖上下文、竞价/价格历史、补偿池、论坛镜像和 lot/装备响应分别拆入
  `contracts/dto/public/catalog/{mod,auction,history,compensation,forum,lot}.rs`；serde 字段、
  untagged preview/live 形状和 Public 稳定导出保持不变。
- Config settings validation owner 已将拍卖策略、HTTP listener、安全/realtime/session、tracing 和路径解析分别拆入
  `config/settings/validation/{mod,auction,listener,security,tracing,paths}.rs`；错误分类、
  旧字段兼容解析、Unix 路径约束、OTLP header 禁止项和 SQLite URL 判定保持不变。
- Storage Public 补偿池 owner 已将后端分派、PostgreSQL 批次/余额查询和单场账本查询分别拆入
  `database/compensation/{mod,postgres/{mod,rounds,movements}}.rs`；分页 cursor、Serializable
  快照、公开字段闭集和 SQLite Lite 能力缺失边界保持不变。
- Storage Admin 装备录入 owner 已将同一 PostgreSQL 竞价事务中的事实写入与审计/realtime 追加分别拆入
  `database/admin_intake/{mod,write,record}.rs`；命令账本占位、world/auction/player/equipment 锁、
  Draft/Preview 目标、pending lot、Preview 展示项追加、版本推进和提交原子性保持不变。
- Domain proxy owner 已将代理快照/枚举、授权状态变更和追加式转换事实校验分别拆入
  `domain/proxy/{model,authorization,transition}.rs`；`proxy/mod.rs` 只保留稳定导出，
  Fast/Slow 状态机、版本推进、终态原因闭集和转换事实校验保持不变。
- Config secrets owner 已将敏感字段/脱敏容器、PostgreSQL 维护密钥读取、MoogleMail `.keyenv`
  解析和 Admin runtime 文件投影分别拆入 `config/settings/secrets/{model,maintenance,keyenv,runtime}.rs`；
  未知字段拒绝、owner-only 权限、路径/symlink/hard-link 防护、运行/隔离验证身份和所有现有 getter 契约保持不变。
- Integrations equipment parser 已将来源 URL/allowlist 校验、旧版 HTML 页面、装备身份映射、统计值
  规范化和共用文本边界分别拆入 `equipment/parser/{url,page,identity,stats,common}.rs`；
  `parser/mod.rs` 只保留稳定出口，固定 origin/world 路径、闭集类别、可交易条件、字段上限和 InvalidSource 失败关闭边界保持不变。
- Application Admin Users service 已将用户列表、创建/修改/改名回滚命令、幂等请求摘要和动作字段校验分别拆入
  `services/admin/users/{query,commands,digest,validation}.rs`；`users/mod.rs` 只保留服务状态、
  `AdminUserPort` 适配和稳定导出，key 生成/封装、state-version fence、命令 replay 和安全结果校验保持不变。
- Application Admin Credentials service 已将安全投影查询、导入/改名/启停/删除命令、幂等请求摘要和字段校验分别拆入
  `services/admin/credentials/{query,commands,digest,validation}.rs`；`credentials/mod.rs` 只保留服务状态、
  `AdminCredentialPort` 适配和稳定导出，明文 secret 不进入摘要，state-version fence、命令 replay 和安全结果校验保持不变。
- Storage Admin runtime 命令账本已将跨后端分派、PostgreSQL 只读查询、pending/applied/unknown/reopen
  事务、行映射和错误/SQLite Lite 能力边界分别拆入
  `database/commands/admin/runtime/{dispatch,queries,mutations,mapping,errors}.rs`；`runtime.rs` 只保留
  稳定公共出口，命令摘要、状态 fence、审计追加和未知结果重放语义保持不变。
- Storage Public 竞价日志已将公共 cursor/结果校验、PostgreSQL 查询/映射和 SQLite Lite 查询/类型转换
  分别拆入 `database/bid_log/{common,postgres,sqlite}.rs`；`bid_log/mod.rs` 只保留端口分派，时间、
  lot、状态版本、事实序号和 bid ID 的 keyset 顺序以及两套后端的非对等 SQL 能力保持不变。
- Storage 论坛回帖 owner 已将 PostgreSQL 主题、lot/身份/代理查询、命令账本、论坛身份历史/审计、
  固定楼层内容版本与连续处理楼层 CAS 和输入/能力校验分别拆入
  `database/forum/reply/{queries,commands,identity,fixed_posts,validation}.rs`；`reply.rs` 只保留
  跨后端端口分派，论坛命令幂等、锁内读取、逐主题健康/调度和 SQLite Lite 能力边界保持不变。
  `auction_forum_reply_cursors` 仅保留为旧迁移兼容表，`cursor` 模块和当前 writer 已移除。
- Storage 论坛主题同步 owner 已将 PostgreSQL 领取租约、快照一致性读取、固定 #1/#2 发布证明、失败状态推进和输入/能力校验
  分别拆入 `database/forum/sync/{claims,snapshot,publishing,validation}.rs`；`sync.rs` 只保留
  跨后端端口分派，claim token/fencing、REPEATABLE READ 快照和 SQLite Lite 能力边界保持不变。
- Storage Public 拍卖目录 owner 已将 PostgreSQL 查询、SQLite Lite 查询以及跨后端行映射/列表参数校验
  分别拆入 `database/auctions/{postgres,sqlite,mapping}.rs`；`auctions/mod.rs` 只保留
  `AuctionReader` 端口装配，cursor、总数和 Lite/完整档投影语义保持不变。
- Storage PostgreSQL Admin Users command owner 已将 recovery 黑名单、用户创建、用户状态/身份变更和改名回滚
  分别拆入 `database/users/admin/postgres/commands/{mod,blacklist,create,mutation,revert}.rs`；
  `postgres.rs` 仍只负责 PostgreSQL 子模块装配，Serializable/advisory lock、命令账本、state-version fence、
  密文信封和追加审计的单事务边界保持不变。
- Storage SQLite Admin Users command owner 已将用户创建、用户状态/身份变更和改名回滚分别拆入
  `database/users/admin/sqlite/commands/{mod,create,mutation,revert}.rs`；每个 owner 仍通过
  `BEGIN IMMEDIATE` 保持命令账本、state-version fence、密文信封、黑名单历史和追加审计的串行写入边界。
- Storage Admin Audit owner 已将请求校验/后端分派、PostgreSQL 查询、SQLite 查询以及 SQLite 行/时间映射分别拆入
  `database/admin_audit/{mod,postgres,sqlite,mapping}.rs`；追加式读取、严格筛选、keyset/数字分页、同事务 total
  和 metadata object 安全投影语义保持不变。
- Storage Rules owner 已将跨后端 Public/Admin 规则安全快照读取与 SQLite 时间映射、PostgreSQL 规则发布事务
  分别拆入 `database/rules/{mod,reads,transaction}.rs`；规则 revision CAS、`rules_admin_commands` 幂等账本、
  Serializable 重试、追加审计和 SQLite Lite 只读/发布不支持边界保持不变。
- Application Admin Users owner 已将安全用户投影/分页模型、用户与 recovery 命令/写入载荷、
  密钥/Users/recovery 窄端口和共享字段校验分别拆入 `ports/admin/users/{model,commands,ports,common}.rs`；
  `users/mod.rs` 只保留稳定导出，用户命令幂等、密文信封和 recovery 黑名单 proof 语义保持不变。
- Application Admin Mail owner 已将轮询/入站安全投影、暂停/触发命令与账本、应用/事务错误、
  查询/命令/事务窄端口以及共享字段校验分别拆入 `ports/admin/mail/{model,commands,errors,ports,common}.rs`；
  `mail/mod.rs` 只保留稳定导出，状态版本 fence、命令幂等和安全投影边界保持不变。
- HTTP shared state 已将 Public 与 Admin 的端口状态容器分别拆入 `http/shared/state/{public,admin}.rs`；
  `state/mod.rs` 只保留兼容导出，两个 surface 的依赖注入、缓存/安全策略和 realtime 连接边界保持不变。
- HTTP Admin Users owner 已将用户列表、创建/属性命令、黑名单与改名状态动作、DTO 映射和共享错误边界
  分别拆入 `http/admin/users/{list,commands,status,mapping,common}.rs`；`users/mod.rs` 只保留稳定
  处理器出口，session、CSRF、command ID、版本 fence 和用户安全投影契约保持不变。
- HTTP Admin Mail owner 已将轮询状态、入站列表/详情、暂停/恢复命令、Worker 唤醒和错误映射分别拆入
  `http/admin/mail/{state,intake,commands,trigger,common}.rs`；`mail/mod.rs` 只保留稳定处理器出口，
  邮箱筛选、状态版本、CSRF、幂等摘要和 Worker control 契约保持不变。
- HTTP Admin Realtime owner 已将 lag/failure 读取、replay 命令、安全投影映射和共享分页/错误边界分别拆入
  `http/admin/realtime/{reads,replay,mapping,common}.rs`；`realtime/mod.rs` 只保留稳定处理器出口，
  旧版嵌套分页、CSRF、request hash 和 replay 状态契约保持不变；旧 wire 缺少 `command_id` 时
  由 HTTP 防腐层稳定派生，新版显式 ID 仍进入同一核心账本。
- HTTP Admin Credentials owner 已将凭据分页读取、导入/改名/启停/删除命令、无 secret DTO 映射和共享
  CSRF/分页/错误边界分别拆入 `http/admin/credentials/{list,commands,mapping,common}.rs`；
  `credentials/mod.rs` 只保留稳定处理器出口，凭据密封、版本 fence、幂等和 PostgreSQL/Lite 能力边界保持不变。
- HTTP Admin Delivery owner 已将交付列表、release/retry/reconcile 命令、快照映射和共享能力/错误边界
  分别拆入 `http/admin/delivery/{list,commands,mapping,common}.rs`；`delivery/mod.rs` 只保留稳定处理器出口，
  动作闭集、CSRF、fencing、request hash 和外部结果未知对账边界保持不变。
- HTTP Admin Auctions owner 已将拍卖列表、创建命令、DTO 映射和共享错误边界分别拆入
  `http/admin/auctions/{list,commands,mapping,common}.rs`；`auctions/mod.rs` 只保留稳定处理器出口，
  旧分页、能力投影、零版本创建 fence、命令幂等和事务错误契约保持不变。
- HTTP Admin Dashboard section owner 已将邮件、realtime、履约/投递/recovery、凭据、alerting 和安全降级
  分别拆入 `http/admin/dashboard/sections/{mail,realtime,operations,credentials,alerting,fallback}.rs`；
  `sections/mod.rs` 只保留聚合映射出口，旧 known/unknown/degraded 形状、时间字段和 section JSON 闭集
  由 HTTP 防腐层统一维护。
- Admin Web Dashboard 页面由 `app/operations/dashboard.rs` 持有唯一 snapshot resource，
  `app/operations/dashboard/{content,controls}.rs` 分别负责旧卡片流和操作区；浏览器先逐层校验原始 JSON
  键集合再反序列化，不再挂载扩展 metrics/runtime/list 请求。
- Storage PostgreSQL Admin lot owner 已将事务端口适配、锁内安全读取、lot 状态/起拍价变更、withdrawn-return
  义务和审计/realtime 事件分别拆入 `database/competition/postgres/admin/lot/{transaction,read,mutation,obligation,events}.rs`；
  `lot/mod.rs` 只负责装配，world → auction → lot 锁顺序、preview revision、state-version fence、命令账本和
  同事务事件/履约义务语义保持不变。
- Storage recovery return owner 已将退回事务编排、严格证据模型、world/obligation/金额校验、action 幂等写入和
  PostgreSQL 行映射分别拆入 `database/recovery_scheduler/scheduler/returns/{record,model,validation,action,mapping}.rs`；
  `returns/mod.rs` 只负责装配，Serializable、退回邮件 evidence mismatch 阻断、action ordinal 和 recovery
 state-version fence 的同一事务边界保持不变。
- Public HTTP reads owner 已将世界/拍卖目录、竞价与价格历史、lot 列表/详情分别拆入
  `routes/reads/{world,auctions,history,lots}.rs`；`reads/mod.rs` 只保留稳定处理器出口，游标/分页边界、
  公开投影校验和既有 Public URL/JSON 契约保持不变。
- Application 核心端口已将事务策略、命令幂等账本、拍卖仓储、竞价事务和 UTC 时钟分别拆入
  `ports/core/{policy,ledger,repository,competition,clock}.rs`；`ports/mod.rs` 只保留模块装配与稳定
  公共导出，原有事务隔离、命令重放、锁内竞价和时钟端口语义保持不变。
- Application settlement fulfillment owner 已将服务编排、无 secret delivery intent、claim/外部证据校验、
  proof 摘要和跨 owner 载荷模型分别拆入
  `services/settlement/fulfillment/{mod,service,intent,validation,evidence,proof,model}.rs`；
  prepare/activate/reconcile 端口、稳定 command/request hash、Sent/Balance 证明和 manual reconcile
  边界保持不变。
- Application settlement command owner 已将 Serializable 结算事务编排、due 世代与活动代理关闭校验、
  以及结算义务闭集构造分别拆入 `services/settlement/commands/{mod,validation,obligations}.rs`；
  稳定结算命令、命令账本 replay、锁内快照、代理终态计划和义务前置依赖语义保持不变。
- Config AppConfig owner 已将 TOML 输入模型、配置定位/读取、敏感值 getter 和 runtime/Worker/数据库
  最终校验构建分别拆入 `config/settings/app/{mod,model,load,accessors,build}.rs`；`AppConfig` 公共
  类型路径、所有配置字段、敏感值最小暴露和 SQLite Lite 并发边界保持不变。
- Storage MoogleMail delivery outbox 已将跨后端端口分派、命令闭集校验、普通 outbox 入队、recovery
  两阶段激活和 PostgreSQL 错误/载荷辅助分别拆入
  `database/mail_delivery/{mod,validation,outbox,activation,common}.rs`；命令摘要幂等、
  `prepared → pending` 激活、审计追加和外部结果后置边界保持不变。
- MoogleMail DeliveryTransport owner 已将入口 operation dispatch、通知/结算消息发送、单 Cookie 会话和
  return/take 动作前置身份校验与后置 proof 分别拆入
  `clients/mooglemail/transport/delivery/{mod,messages,credentials,actions}.rs`，并将普通邮件
  payload 校验与结算 payload/模板渲染拆入 `transport/payload/{mod,mail,settlement}.rs`；
  transport type owner 又将普通/结算载荷、模板快照和 transport 装配状态拆入
  `transport/types/{mod,delivery,settlement,templates,transport}.rs`；
  payload 闭集、session 失效后的不确定结果分类、availability gate 和外部重试边界保持不变。
- Storage Public account mapping owner 已将普通/代理竞价事实、代理状态投影以及跨后端读取与分页校验分别拆入
  `database/account/mapping/{bids,proxy,common}.rs`；`mapping/mod.rs` 只保留稳定导出，公开账户边界、代理
  capability 派生和 PostgreSQL/SQLite 投影校验语义保持不变。
- E-Hentai client owner 已将客户端装配、固定 origin/限速/响应边界、主题分页和主题表单/发布回执分别拆入
  `clients/ehentai/client/{mod,request,pagination,topic}.rs`；公共客户端构造与 Forum 端口契约、session
  失效、write-unknown 和远端主题身份证明语义保持不变；分页只接受同 origin/thread 的规范 `st`，
  并在配置页数与 32 MiB 聚合上限内补齐全部页面，每个响应都会重新核对主题身份。
- MoogleMail client owner 已将 typed target/配置装配、有界 GET/POST 与结果分类、固定 endpoint/响应
  判定/限速/敏感载荷辅助分别拆入 `clients/mooglemail/client/{mod,request,security}.rs`；Cookie、
  write-unknown、重定向/编码拒绝和请求间隔语义保持不变。
- MoogleMail protocol owner 已将世界/操作/敏感值对象、固定 URL 构造和输入/endpoint 校验分别拆入
  `clients/mooglemail/protocol/{mod,model,builder,validation}.rs`；Cookie、mmtoken、装备 key 的脱敏
  生命周期，生产/本地 endpoint allowlist，以及 inbox/Sent/Balance/equipment 路径参数校验保持不变。
- Config scheduling owner 已将标准 cron 公共模型、五字段范围/名称解析以及 `@every` Go duration
  解析分别拆入 `scheduling/standard_cron/{mod,fields,duration}.rs`；描述符、`TZ=`/`CRON_TZ=`、
  本地/UTC/IANA 时区和整秒触发语义保持不变。
- Application Admin intake owner 已将装备来源类型/规范化事实、命令摘要、来源快照、脱敏事务记录、
  窄端口和共享字段错误分别拆入 `ports/admin/intake/{equipment,commands,source,records,ports,common}.rs`；
  `intake/mod.rs` 只负责稳定导出，明文 key 清理、密文信封、幂等摘要和来源先读后写边界保持不变。
- Storage Public lot owner 已将 PostgreSQL/SQLite 的列表查询、详情查询和数据库行映射分别拆入
  `database/lots/{postgres,sqlite}/{mod,list,detail,mapping}.rs`；顶层 `lots/mapping.rs` 继续拥有
  跨后端公开投影、装备字段校验和 Lite 时间/UUID 转换，`lots/mod.rs` 只保留端口分派，preview/live
  互斥投影、分类统计、分页和两种数据库能力边界保持不变。
- Storage SQLite competition transaction owner 已将命令账本、锁内快照、计划编排、代理/lot 写入、
  追加审计和 realtime 事件分别拆入
  `database/sqlite/competition/transaction/bidding/{mod,ledger,snapshot,plan,proxy,facts,audit,realtime}.rs`；
  `transaction.rs` 仍独占 `BEGIN IMMEDIATE`、提交/回滚和重试边界，竞价计划的幂等、版本 fence、
  审计与事件原子写入语义保持不变。
- Storage PostgreSQL competition transaction owner 已按同一职责边界拆入
  `database/competition/postgres/bidding/{mod,ledger,snapshot,plan,proxy,facts,audit,realtime}.rs`；
  `competition/postgres.rs` 仍独占 Serializable/行锁事务、提交/重试和端口装配，竞价、代理、
  审计与 realtime 事件的同事务顺序保持不变，并与 SQLite Lite owner 保持结构对称。
- Admin Web realtime owner 已将页面状态/replay、筛选器、lag 表/分页和 failure 表/分页分别拆入
  `app/operations/realtime/{mod,page,filters,lag,failure}.rs`；surface 路由、replay 的
  `command_id` 派生和安全投影契约保持不变。
- Admin Web delivery owner 已将页面编排、状态筛选、列表/分页、动作对话框、共享状态模型和标签辅助分别拆入
  `app/fulfillment/deliveries/{mod,page,filters,table,dialog,model,helpers}.rs`；release/retry/reconcile
  动作、CSRF、`command_id`、fencing version 和外部结果未知边界保持不变。
- Admin Web rules owner 已将页面加载、规则编辑 workspace、Markdown 预览、发布状态模型和字段辅助分别拆入
  `app/content/rules/{mod,page,workspace,preview,model,helpers}.rs`；语言切换、服务端规范化校验、revision
  fence、最近校验草稿限制和发布 command_id 复用语义保持不变。
- Shared Web Markdown owner 已将共享展示器的模型、块级解析、行内解析和安全 DOM 渲染分别拆入
  `components/markdown/{mod,model,blocks,inline,render}.rs`；Public 规则正文与 Admin 预览继续共用同一
  Markdown/GFM 子集，原始 HTML 转义、HTTP(S)/相对路径/mailto 链接白名单和图片懒加载语义保持不变。
- Public Web board owner 已将看板入口/数据编排、世界选择、实时连接刷新判定和历史导航分别拆入
  `app/surface/board/{mod,data,world,realtime,navigation}.rs`；当前/历史拍卖读取、lot 快照对齐、
  倒计时、事件断档回权威快照和 URL 导航语义保持不变。
- Public 页面 owner 已将历史拍卖列表、lot 详情/竞价、通用重试状态和路由薄包装分别拆入
  `app/surface/pages/{history,lot/{mod,detail,equipment,history},shared,routes}.rs`；`pages.rs` 只保留路由组件稳定出口，
  URL 查询状态、实时刷新、竞价展示和 14 条公开路由保持不变。
- Public 补偿池页面 owner 已将批次列表/展开交互与流水详情/类型金额展示分别拆入
  `app/content/compensation/{mod,table,movements}.rs`；分页、URL 状态、公开投影和补偿守恒展示语义保持不变。
- Admin 拍卖生命周期动作 owner 已将列表行、确认对话框和动作可用性/文案模型分别拆入
  `app/auction/auctions/actions/{row,dialog,model}.rs`；`actions/mod.rs` 只保留稳定组件出口，
  capability 过滤、状态版本 fence、CSRF、command ID 重试和成功/失败反馈语义保持不变。
- Public 价格历史筛选 owner 已将筛选主表单、日期/价格控件、错误模型和输入规范化分别拆入
  `app/auction/reads/history/filters/{mod,components,model,validation}.rs`；K/M 金额转换、日期闭区间、
  URL 查询字段和五语言错误文案语义保持不变。
- 体量复核仍保留少数后台装配、领域聚合和适配器文件作为后续结构优化候选；本轮明确复核了
  `http/admin/dashboard/sections/`（统一 dashboard section 安全投影适配）、`http/admin/lots.rs`
  （单一 lot 列表/详情边界）、`routes/public/competition/`（同一 competition command owner）和
  `services/admin/users/`（单一 Users 用例、key/事务 owner），它们当前职责边界清晰，不能把“可继续拆分”
  误报成旧版功能缺口或测试任务。当前已完成拆分的 storage、Worker 和 Web owner 不再列为未完成项。

规范门禁此前还存在 CI 漏接：工作流直接执行 Cargo 命令，却没有运行 Justfile 中的目录和配置
检查。本轮将共同规则收敛到 `ci/check-policy.sh`，本地 Justfile 与 CI 复用同一实现，并补充五种
语言文件闭集、字面量消息引用、重复 ID 和点号 ID 检查。新门禁实际发现并修复了 Admin 改名页中
`admin-renames-previous`/`admin-renames-next` 同时表示分页按钮和用户名列的重复定义；页面保留表头
含义，未使用的旧分页键已从五种语言同步删除。

得出该结论前，本轮又从旧版配置消费链发现并修正了三组此前被完成表掩盖的静态差异：MoogleMail
轮询从固定间隔恢复为标准 cron/描述符/`@every`/时区调度，MoogleMail transport 恢复 endpoint、
请求间隔与请求超时配置，Worker control 恢复安全 runtime 目录、实例锁、group/0660、inode 清理
和显式 UID/GID allowlist。部署拓扑还补齐了 Admin/Worker control socket、三组数据密钥及
Public/Admin TCP/Unix listener 的一致性检查，Admin readiness 会跟随 runtime 的真实 listener。
这些项目均已进入代码和文档，不再列为剩余任务；上面的后续结构优化项仍需单独按 owner 审计，
不得被本轮已完成的拆分结论覆盖。

以下是仍需真实环境完成的行为/部署证据，不属于上面的代码数量盘点；测试相关工作继续按用户要求
暂缓：

- 本轮已用仓库锁定的 `dx` CLI 分别构建 Public/Admin Web 产物，两个 WASM、wasm-bindgen、入口 HTML
  和 surface/base CSS 均成功生成并复制到后端稳定嵌入目录；Public 的 14 条路径随后已完成 `en` 桌面
  匿名/空态浏览器巡检，规则和补偿池另有五语言证据。动态数据行、认证动作、错误/冲突、移动端逐页
  状态和 Admin 其余页面仍不能由编译或这轮局部 R1 证明。
- 使用真实 main/isekai 邮箱与 E-Hentai 论坛继续核对分页、限流、凭据失效、远端写入不确定结果和回执；
  2026-08-30 已在 Auction-Test 使用有效 main 隔离账号完成两组凭据的 Balance/Inbox 读取、手动唤醒与
  暂停/恢复幂等控制，并用 1 Credit 和 1 件可交易装备逐项证明发送、业务拒绝、通知、`[RTS]` 退回、
  精确 take 和原装备恢复；最终 mail claim、失败、非终态 delivery 与人工对账均为零。该证据不覆盖
  isekai、凭据失效收口、论坛写入、生产账号或生产流量。当前 Worker 已收敛为只装配一条运行 Cookie；
  该历史双凭据读取事实不代表继续提供自动轮换。
- canonical PostgreSQL v28 的 backup/verify/adopt/rebuild/restore 安全边界已在隔离旧对照副本完成；
  仍需具有非空 auction/lot/bid/realtime 事实的数据抽样，以及真实部署切换批准；
- 同宿主机隔离 systemd/Caddy 已核对启动顺序、readiness、失败回滚、Worker control、严格文件权限和
  unit 配置绑定；真实域名/TLS、专用生产账号、正式 secret 注入及流量切换仍需独立批准；
- 继续用实际 Dioxus 浏览器产物核对 Public 动态/认证/错误/移动端状态，以及 Admin 其余页面的字体、
  换行、焦点顺序、空态和响应式布局；
- 继续在当前授权的隔离环境核对 PostgreSQL 完整档、SQLite Lite 边界和旧版双跑；Auction-Test 的
  MoogleMail main 最小 Credits/装备写入已取得一次性证据，论坛/其它外部写入、双写与最终生产切换
  仍需逐项独立批准。

上述隔离人工对照按当前授权继续；测试用例仍按本仓库约定暂缓。本轮没有新增或运行单元测试、
集成测试、功能测试、端到端测试或浏览器测试。

本轮 Public 价格历史页面已补齐旧版的 world/name/买家/卖家筛选、可折叠高级筛选、月/年日期范围、
K/M 价格单位、日期与价格范围校验、URL 状态、成交统计、拍品与拍卖链接和分页；历史表格的装备
类别也会按当前语言显示。

本轮 Admin 拍卖列表与 lot 页面已补齐旧版的状态徽章、动作组、时间字段语义和窄屏表格字段标签；
拍卖创建表单的 `datetime-local` 现在按浏览器本地时区转换为 UTC，并与提交前的时间校验使用同一
规则，避免非 UTC 管理员创建时发生时间偏移；lot、代理历史和邮件入站表中的闭集状态/类型也已
通过五种语言资源展示，不再直接暴露内部枚举代码；邮件入站 `datetime-local` 筛选也按浏览器
本地时区转换为 UTC，避免时间范围查询偏移；
Admin 全部旧版页面的源码结构与样式规则现已逐页对照；剩余是用真实浏览器产物核对字体渲染、
实际换行、焦点顺序和空态细节，不影响已接入的业务动作与安全投影。

本轮继续对照邮件审查页：world、kind、status 三个旧版闭集筛选恢复为选择后立即回到第一页并重新
读取；邮箱、决策码和时间范围仍由显式加载/应用动作提交，避免输入过程产生无界请求。入站表的首列
改回旧版展示的外部邮件 ID，内部 intake UUID 继续只在详情面板中展示，列表动作仍使用内部 ID 读取
详情，避免改变后端契约。

凭据页继续补回旧版可见的最近轮换时间列；该值来自现有安全 DTO 的 `rotated_at`，为空时显示统一
的空值文案，不暴露凭据内容，也不改变版本 fence 或命令边界。

本轮 Public 错误态已补齐 lot 详情、竞价历史、代理加载和实时状态的重试入口；重试只重新读取
权威 API，不在浏览器端缓存或伪造业务状态。

本轮 Admin 拍卖创建、生命周期、intake 和 draft lot 修改已对齐旧版的幂等重试语义：同一表单输入
或同一状态版本动作会复用 `command_id`（创建同时复用 `auction_id`），字段改变后才生成新命令。

本轮 Admin 其余写入口也已完成同一语义：delivery release/retry/reconcile、用户创建/改名/身份/key/黑名单、
凭据导入/改名/启停/删除、邮件暂停/恢复/立即轮询、recovery 确认、realtime replay 和改名回滚，
均由页面按输入或状态版本复用 `command_id`；字段、原因、公开 replay payload 或版本改变后才生成新命令。
这只解决浏览器重试与后端幂等账本之间的契约缺口，外部服务回执、真实部署对照和切换前验证仍未完成。

本轮 Admin delivery、凭据、Users、改名、realtime、邮件入站、审计和结算/Recovery 表已补齐旧版的
移动端字段标签、状态徽章、动作组、行 hover 和表格最小宽度；窄屏卡片化布局不再出现空字段标题，
其余 Admin 页面也已完成源码级视觉收尾，真实浏览器逐页验收仍单独保留。

本轮继续补齐结算/Recovery 表的窄屏卡片化：这些表没有复用 Public catalog 壳层，现通过已有
`data-label` 显示字段标题，并保持金额列在窄屏下左对齐，避免横向滚动时难以逐行阅读。

本轮还把 Admin 拍卖创建入口移回旧版的 world/status 筛选操作栏，并在宽屏靠右、窄屏回到
普通流布局；创建弹窗、命令 ID 复用和提交契约保持不变。

本轮补齐旧版部署脚本中可安全自动化的只读预检：`auctionctl deployment-check` 会加载
Public/Admin/Worker 三份 TOML，确认敏感/runtime 文件存在、三进程数据库档位一致、Public/Admin
实际 TCP/Unix listener 不冲突、Admin/Worker control socket 与三组数据密钥一致，并校验启用的
Worker control runtime 目录、group 和显式 UID/GID allowlist；`--strict-permissions` 可把
敏感文件的 group/other 权限作为切换前硬门槛。预检不连接数据库、不执行迁移、不启动进程，也不
触发外部服务；后续条目已经补齐启动编排、PostgreSQL restore 和独立 rebuild 契约，retention
mutation 则因旧版本身明确禁止而不属于等价迁移范围。

本轮新增 `auctionctl retention-inventory` 与 `auction-storage` PostgreSQL 只读盘点 owner：命令要求
`--confirm-database` 与 `current_database()` 完全一致，在 `REPEATABLE READ READ ONLY` 事务中读取
`auction_schema_identity`、SQLx 迁移历史、public 关系统计和编译进制品的版本化
`retention-policy.v1.json` 政策矩阵；证据包含政策 SHA-256、政策表缺失覆盖、数据族汇总、realtime
诊断水位、外键依赖和待人工决定项，以不可覆盖的私有 JSON 保存。关系数量超过 512 时失败关闭，
realtime/外键输出带显式覆盖标记，结果固定声明未评估、未授权保留资格。它补齐了旧版 retention
工具的主要只读证据边界，但不代表备份、恢复、归档、删除或生产 retention mutation 已完成。

本轮继续新增 `auctionctl recovery-preflight` 与 `auction-storage` PostgreSQL 只读恢复预检 owner：
它要求精确确认 `current_database()`，在只读事务中记录主节点/默认只读状态、其他连接、数据库大小、
用户对象数量、Auction schema identity 和 migration 历史，并以不可覆盖私有 JSON 保存；
`restore_preflight_passed` 只代表空库、无其他连接和可写主节点等基础条件满足，不验证 archive/manifest，
也不执行 `pg_restore`、停止连接或任何数据库 mutation。

本轮继续新增 `auctionctl postgres-backup` 与 `postgres-backup-verify`：前者要求源库通过精确数据库
身份、Auction schema identity 和当前/成功 SQLx migration 检查，调用外部 `pg_dump` 生成 custom-format
archive，再用 `pg_restore --list` 验证并在前后核对迁移版本；密码只通过短生命周期的 0600 `.pgpass`
传递，archive 与 manifest 均以 0600、不可覆盖的私有文件保存，manifest 固定记录数据库身份摘要、
schema 版本和 archive SHA-256。后者只做离线配对、哈希和 archive 目录校验。

本轮继续补齐 `auctionctl postgres-restore`：命令只接受同一备份链路生成且二次验证未变化的
archive/manifest，要求精确数据库确认、可写主节点、零外部连接和零 PostgreSQL 用户对象；预检后
显式关闭工具自身 SQLx pool，再执行
`pg_restore --exit-on-error --single-transaction --no-owner --no-privileges`。提交后必须证明 Auction
schema identity 与 manifest 的 clean migration 版本一致，才以不可覆盖 0600 JSON 发布
`auction-postgres-restore.v1` receipt；高于当前二进制迁移版本的 archive 在 mutation 前失败关闭。
空库创建、外部进程停止、destructive rebuild 和完整 retention mutation 仍不属于该命令。

本轮继续补齐旧版最后一个本地部署代码缺口 `auctionctl postgres-rebuild`：命令默认只读，要求
当前 clean Auction schema、PostgreSQL 18 主节点、精确目标、零外部 session，以及 verified backup
或逐字确认的一次性环境豁免；实际 mutation 还要求 `--execute`、`REBUILD <数据库名>` 和新的私有
receipt。`auction-storage` 在 `postgres` 维护库持有集群 advisory lock，拒绝自定义 database ACL/
role setting、prepared transaction、subscription 和活动 logical slot，二次校验 backup 后才执行
`DROP DATABASE ... WITH (FORCE)`，并保留 owner、UTF8、locale provider、tablespace 和 connection
limit。`empty-for-restore` 证明零用户对象，`fresh-schema` 运行并证明当前迁移；破坏性步骤后的失败
回执显式记录数据库是否已删除/重建。OPS-003 已证明隔离进程停止与 systemd/Caddy 演练；正式恢复点
保管、生产账号和流量切换仍需部署批准。

本轮进一步补齐现有 Auction-BF 数据的显式接管边界：`auction-storage/database/legacy_v28` 在
只读预检中要求 PostgreSQL 18.x、唯一且 clean 的 migration 28、精确 canonical public 表集合、
零其他 session、有效 schema identity 和可无损恢复的 policy/lot/bid realtime 事实；事务 bridge
补齐 Rust 生命周期列，将无领先者的旧起始价投影转换为空当前价，从原始 bid event 恢复逐事实
截止时间，保留并升级 final-fast 代理证据，建立缺少的兼容表/视图和 lower 查询索引。`auctionctl`
新增专用 v28 custom-format 备份/离线验证与默认只读 adoption；实际执行要求逐字
`ADOPT LEGACY V28 <数据库名>`、二次 artifact 校验和不可覆盖 receipt，写入 SQLx `1..=5`、`7`
当前 checksum 基线后只运行剩余迁移。普通 migrate/restore 不接受旧版 manifest；完整手册位于
`ops/deploy/LEGACY_V28_ADOPTION.md`。OPS-001 已在 canonical v28 隔离副本完成接管与恢复 R1；非空旧业务
事实抽样和生产切换仍属于显式验收。

本轮继续对照旧版最终 schema，确认 Rust 压缩迁移曾遗漏拍卖身份不可变、前序顺序和
draft/intake target 一致性触发器，也确认早期 Rust `000003` 遗漏了既有 draft 的 target 回填。
追加 PostgreSQL `000014_lifecycle_database_invariants` 后，迁移会先锁定并拒绝歧义投影，为可唯一
映射的 draft 补齐 target，再以当前定义安装三类防线；同一迁移移除旧 v28 的论坛自动入队 trigger，
避免数据库与 Worker 同时成为论坛任务 owner。`retention-policy.v1.json` 的 schema fence 当前随
`000016` 推进到 `16`，并把 `forum_sync_admin_commands` 归入永久保留的拍卖历史族，避免只读
retention inventory 因策略版本或表覆盖仍停在旧状态而无条件失败。SQLite Lite 不复制
PostgreSQL 延迟触发器。

后续产品变更追加 PostgreSQL `000032_preview_intake_handoff`：每个 world 只允许一个 Draft 或 Preview
当前录入场次，录入目标必须精确指向该场次并携带匹配状态。Preview 不再提前建立后继轮次；Admin/MM
在 Preview 期间继续向当前拍卖写入 pending lot，并在同一事务追加新 revision 的预览展示项和 realtime
重读信号。Open 才创建后继 Draft、登记下一轮并迁移录入目标；Preview 取消也会建立可继续接收装备的
后继 Draft。迁移拒绝旧实现留下的 Preview 与后继 Draft 同世界并存窗口，升级必须避开该窗口，不能
自动猜测数据归属。SQLite Lite 没有外部录入目标，但生命周期同样在 Open 或 Preview 取消时建立后继。

同一轮旧版对照还发现 Rust operations alert 的第一次重试会被既有 CHECK 拒绝：代码曾把非空
Create 错误码写入 `pending`，并在第三次失败时错误转为 `manual_reconcile`；同时 signal cooldown
在领取阶段过早生效。追加 `000015_operations_alert_state_invariants` 后，普通失败只持久化无错误码
退避，identity mismatch 与三次耗尽分别固定为 `dependency_mismatch` / `retry_exhausted`，lease
过期由 fencing 失败关闭，批次、delivery 和 signal cooldown 在入队确认事务中原子推进。迁移会
归一化当前草案和 v28 历史状态；旧版 Create 前合法形成的终态孤儿保留为审计事实，外键对迁移后的
新写入立即生效，并在不存在历史孤儿时完成全表验证。

运行时对照还发现 operations alert 曾被合并进全局 5 秒 tick，scan 的来源读取失败会连带跳过
dispatch/reconcile，而且批次窗口、冷却、重试、批次大小、正文上限和水位阈值多项默认值偏离旧版。
现在 scan、dispatch、reconcile 已恢复为三个独立的一分钟循环：scan/reconcile 以 claim lease 为
deadline，dispatch 预留来源读取超时；单类临时失败只影响自身下一轮。上述策略默认值已对齐旧版，
recovery action wave 的默认并发也从 1 恢复为 8。

继续向下复核 Worker 调度后，确认 fulfillment/recovery 虽然处理函数齐全，却仍被串在同一个全局
5 秒 tick 中：fulfillment 单轮上限实际等于全局并发，缺少独立 item timeout；任一 claim 错误还会
终止整个 Worker。现在两者都恢复为立即首轮、随后 `60s + [0,5s]` 的独立循环，默认批次 100、
并发 8、lease/持久化重试 60 秒、单项总时限 30 秒。Fulfillment 按并发波次领取，Recovery 继续按
`case → return → action → relist → refund` 尝试全部 phase；单轮错误只记录指标并等待自身下一轮，
只有循环任务异常退出才使进程失败关闭。SQLite Lite 不装配这两类 PostgreSQL 完整档循环。

Auction 主循环此前也只有全局 1 并发/5 秒 tick：生命周期与结算单轮上限都被并发值截断，没有
旧版 30 秒单项预算，claim 失败还会让整个 Worker 退出；重新审计又确认慢速代理只有领域计划而
没有到期扫描/执行 owner。现在 Auction 已拥有独立立即首轮和 `60s + [0,5s]` cadence；生命周期、
慢速代理和结算各自使用 100 条上限、共同并发波次和单项总时限，一阶段错误仍继续尝试后续阶段。
慢速代理使用数据库时钟扫描和事务内 generation/due/authorization/截止时间/owner 资格 fence，
沿用旧版稳定命令身份、stale 跳过和审计。配置省略并发时按 PostgreSQL=8、SQLite Lite=1 计算
有效值，Lite 不启用结算；邮件手工唤醒不再顺带执行 Auction。

继续对照旧版 MoogleMail 写入协议后，确认 Rust return/take 曾只核对动作按钮，POST 后也只依赖
成功文字或按钮消失，无法证明实际处理的是预期邮件和资产。现在动作状态会脱敏保留
To/From/Subject、正文、装备 ID、Credits、COD 和附件计数，但不保留装备 key 或原始 HTML。
transport 在 POST 前校验 sender；take 还与 outbox 的预期资产做集合精确比对。return POST 后
优先要求当前协议的唯一只读 `[RTS]` 回执与动作前身份、标题、正文、Credits/COD、装备 ID 和附件数
逐项匹配，并证明 return/take 动作消失；旧协议仍要求收件箱表头/结果行结构合法且目标 `mid` 不再被
任何链接或动作引用。take POST 后要求邮件身份不变、资产/COD 归零、claimable 消失，并核对正文
追加的唯一 `CoD Paid`/`Removed attachment` 回执。前置或后置无法证明都进入人工对账，不会误报成功
或盲目重发。

同一轮还确认 Rust 的最终 send 响应曾把旧版可证明的“收件人不存在”和“无法支付邮资”
统一收敛为 `write_unknown`，且 transport 失败端口无法保留退信证据；compose 初始页解析失败或残留资产
还曾被当作可确定失败。现在非干净 compose 统一以 `preflight_unproven` 人工核对，parser 只检查排除
script/style/noscript/textarea 后的可见 body 文字；收件人不存在保留唯一合法退信 `mid`、
`reason_code=recipient_not_found` 和 `take_required=true`，并在退信资产取回前进入人工对账；邮资拒绝在未暂存资产时是确定
失败，已暂存 Credits/装备/COD 时则是 `send_rejected_after_stage` 人工对账。delivery 失败端口与
PostgreSQL 写回同步扩展为可保留有界安全 metadata 和外部引用，storage 仍覆盖权威
`safe_detail` / `outcome_unknown`，Admin 只接收已有安全投影字段而不接收 metadata 原文。
Operations alert 现在会对上述严格退信证据追加 `delivery_returned_mail` signal，摘要明确要求核对
后台交付页和 MM；它沿用人工对账数量阈值、保留总人工对账提醒，并排除自己的通知 delivery。

继续核对旧版 Delivery Worker 后还恢复了发送前 availability gate：Rust 现在会从合法 payload
解析目标 world，并在 `claimed → sending` 前检查 scoped/shared Cookie；当前无凭据时以完整
lease token/fencing 释放回 `pending`，而不是把 `credential_unavailable` 误写成失败终态。非法
payload 仍进入正式发送路径确定失败；进入 `sending` 后的凭据竞态、网络和协议依赖错误进入
`manual_reconcile`。登录 key 不存在与无效分别恢复为 `secret_not_found` / `invalid_secret`，
存储或解密依赖不可用则保持人工核对。

同一条旧版 Worker 对照还补回了 Delivery 独立运行边界：`[worker.delivery]` 现在分别控制
poll、jitter、batch、concurrency、lease、renew、sending grace、完整请求和 finalize timeout，并恢复旧版
60s/5s/100/8/60s/10s/120s/30s/5s 默认值。Worker 只在并发槽空闲后领取下一条，避免已领取任务在
队列中耗尽租约；多步 compose 在 `sending` 期间用同一 token/fencing 按配置续租。请求超时、网络
错误和凭据竞态都按结果未知收口；续租丢失时只尝试原 fence 的结果未知写回，stale claimant 不能
覆盖新结果。数据库仅在 lease 再加 sending grace 到期后把遗留 `sending` 隔离为
`manual_reconcile`，不会直接重发。每轮完成后使用 `poll + [0, jitter]` 再唤醒，单轮 claim 或
持久化错误只记录并延后重试，不再终止整个 Worker；循环任务异常退出仍失败关闭。

继续对照旧版 Delivery repository 后发现 Rust 曾只保存状态事实，缺少旧版覆盖创建/准备/激活、
Worker claim/start/release/终态、过期 sending 隔离和 Admin 人工动作的同事务审计；recovery 邮件
激活还未保存独立 activation identity，任意后续状态都可能被误当作重放。现在 PostgreSQL owner
已统一追加上述安全审计，审计失败会回滚状态事务，幂等重放不会重复记账；邮件与结算两阶段激活
都会写入并核验 activation command ID、请求摘要和时间，已进入 claimed/终态后的重放也只能由
原激活身份确认。续租不改变业务状态，沿用旧版不逐次写审计的边界。

本轮 Public 补偿池页已补齐旧版的批次分页、展开/收起、补偿守恒摘要、流水分页、拍品链接、
装备/世界/流水类型投影和金额正负格式；`/compensation` 兼容路径与旧版一样回到补偿池视图。
Public 与 Admin 的可见时间统一使用浏览器当前语言的 `Intl.DateTimeFormat`，固定旧版约定的
`Asia/Shanghai` 时区，`datetime` 属性仍保留原始 RFC3339 值。

本轮 Public 竞价日志还补齐了旧版的拍品文本/UUID 筛选、URL 状态、真实 lot UUID 链接和 credit
金额格式；分类编码与 UUID 筛选都在存储层先作用于全量结果再分页，避免只有当前页命中的假筛选，
服务端已有的 `world`/`lot` 查询边界现在由浏览器 DTO 正确消费。

本轮 Admin 邮件入站轮询摘要中的最近轮询与下次轮询时间也统一使用管理端本地化时间格式，避免该
页面仍直接暴露 RFC3339 原始值；其余 Admin 页面仍按旧版逐页做视觉细节收尾。

本轮 Admin 拍卖对照补齐了计划开放就绪判断：拍卖到达 `scheduled_open_at` 后会自动显示开放
操作；lot 表恢复起拍价字段展示，创建、生命周期、lot 修改和 delivery 操作对话框补齐 Escape
关闭与首焦点行为。后续运行链审计又修复了手动提前开放被 HTTP capability 固定开启的问题：
capability 现在读取启动时冻结的 `allow_manual_early_open`，计划时间之前的人工开放还会在锁内按
同一策略失败关闭，命令重放仍先返回已提交结果。装备来源适配器也在同一轮接入 runtime 的
allowlist、响应上限、共享请求间隔和总超时，并改为在流式读取期间实施大小限制。

本轮 Admin 改名历史恢复了旧版的回滚确认对话框，回滚命令仍使用快照中的用户状态版本和稳定
`command_id`；筛选也允许只填写用户名，不再强制输入用户 ID。

本轮继续完成 Admin 视觉收尾：补齐拍卖 draft/settling/settled 状态与可开放徽章、审计筛选的四列/窄屏
响应式布局、审计 metadata 的有界展示、lot/改名确认对话框的遮罩与表格占位行复位，以及金额字段的
等宽数字呈现。上述调整只作用于展示和交互布局，不改变 HTTP、命令账本、状态版本或外部回执边界；其余
Admin 页面仍需在真实浏览器产物上逐页核对字体、间距和空态细节。

本轮继续对照 Admin 拍卖页：筛选栏恢复旧版轻量横向布局，手工装备录入恢复旧版五列布局并在中等
宽度自动切换为单列，创建弹窗的 world/sequence/datetime 字段改为纵向稳定排列；拍卖表格继续保留
现有的状态筛选、时间线、ready 提示、动作确认和窄屏 `data-label`，只调整布局与间距，不改变业务
命令或状态版本边界。

本轮继续对照 Admin 邮件页：邮箱加载控件、闭集筛选和轮询状态卡恢复旧版的轻量间距与字段宽度，
窄屏统一切换为纵向表单；入站详情、分页、轮询命令和状态反馈保持现有契约，不改变邮箱筛选或
外部回执边界。

本轮继续对照 Admin 凭据页：导入区恢复旧版页面标题顺序、32rem 宽度和纵向字段排列，kind/world/status
筛选保持轻量横向布局并在窄屏切换为纵向控件；secret 展示、状态动作、删除确认和 PostgreSQL 完整档
边界保持不变。

本轮继续对照 Admin 配置页：非敏感 JSON 编辑器恢复旧版全宽、无圆角和 34rem 最小高度，标题、说明与
fingerprint 状态分层展示；候选校验、fingerprint fence、稳定 command ID 和重启提示保持原有契约。

本轮继续对照 Admin Users 页：页面标题与创建用户入口恢复为旧版同一顶栏，移除列表区域内重复标题，
窄屏时顶栏纵向排列；创建对话框、一次性 key 展示、筛选提交和用户命令边界保持不变。

本轮继续对照 Admin Delivery 页：状态筛选恢复旧版无边框横向布局，移除通用面板带来的额外占位，
窄屏切换为全宽纵向控件；投递安全投影、fencing version、确认动作和外部回执边界保持不变。

本轮继续对照 Admin Audit 页：actor/action/entity/request 文本、来源/结果枚举和时间范围筛选恢复旧版
字段宽度，并按 60rem/48rem 断点重排；审计记录仍保持追加只读、metadata 有界展示和既有分页契约。

本轮继续对照 Admin Realtime 页：标题、连接状态与高级筛选恢复旧版同一相对定位壳层，高级筛选在宽屏
作为右上角弹层、窄屏回到文档流；lag/failure 安全投影、分页、replay 确认和外部广播 sink 边界不变。

本轮继续对照 Admin 全局外壳：导航恢复旧版扁平顺序和 `/cookies` 单一凭据入口，移除分组标题、重复
凭据入口和状态页导航项；登录表单恢复旧版窄表单与无卡片边框布局，别名路由仍可通过旧 URL 访问，
并按旧版以无历史记录跳转收敛到 canonical 地址，同时保留查询字符串与片段，未改变 HTTP 或权限契约。

本轮补齐 Public/Admin 旧版别名的 canonical 跳转：`/auction`、`/account`、`/compensation` 以及
管理端 `/dashboard`、`/auction`、`/mail`、`/delivery`、`/credentials`、`/rules` 不再重复渲染
正式页面，而是使用浏览器路由 `replace` 收敛到旧版对应地址；公开端保留筛选/分页查询参数，管理端
额外保留片段，避免旧书签在返回操作中形成别名导航环。

Admin 登录流程也恢复旧版的深链接回跳：未认证访问受保护地址时转到带编码 `return` 参数的
`/login`，认证成功后仅允许回到站内路径并使用 `replace` 返回原始地址；无效或缺失的 return
值回到总览，不把外部 URL 交给浏览器路由。

Admin 浏览器会话还恢复旧版的生命周期维护：根据服务端下发的 idle expiry 触发 GET touch，
请求失败时每 30 秒重试，绝对过期仍由服务端决定；刷新任务只持有安全会话投影，不读取或保存
HttpOnly session Cookie。

Admin 全局壳层也恢复旧版的 readiness 可访问性提示：登录页和已认证页都会读取 `/health/ready`，
初始显示 checking，未就绪或请求失败显示 degraded，成功显示 ready；提示继续使用屏幕阅读器可见、
视觉隐藏的状态组件，不把进程探针误当成业务 dashboard 健康结果。

Admin 弹窗键盘交互也恢复旧版全局焦点边界：最内层 `aria-modal` 弹窗的 Tab/Shift+Tab 会在可用控件
之间循环，不会把焦点带到遮罩外；关闭最后一个弹窗后恢复最近的外部触发控件，并恢复页面原有
滚动状态。Escape、提交锁定和各页面已有的 autofocus 行为保持不变。

Public 未匹配路由也恢复旧版行为：使用同一 `replace` 边界回到首页，而不是在 Public 外壳之外
渲染独立 404 页面；原始查询字符串继续保留，便于旧链接恢复其筛选上下文。

本轮继续对照 Admin Rules 编辑页：事实栏间距、Markdown 编辑区的 36rem 最小高度、48rem 预览上限和
窄屏单列布局恢复旧版；语言切换、候选校验、发布门槛和 revision fence 不变。

本轮继续对照 Admin Dashboard 展示层：页面恢复旧版单 snapshot 请求、扁平卡片、统一边框、固定连续顺序
和操作区，不再挂载分页列表、扩展 metrics/runtime 请求；health、Worker、tracing、realtime、mail、delivery、
fulfillment、recovery、credential availability 与 alerting 均只消费旧 v1 安全投影。

本轮继续对照 Public 页头连接提示：实时状态徽章恢复旧版圆点、raised 背景和统一边框，并为 checking、
connecting、connected、error 保留稳定状态色；实时订阅、游标恢复和手动刷新行为不变。

本轮继续对照 Public Catalog 窄屏表格：字段标签恢复旧版自然宽度，不固定为 `5rem`；`data-label`、参与者投影、
竞价动作和移动端字段语义保持不变。

本轮继续对照 Public 倒计时：HTTP 响应补齐标准 `Date` 头，浏览器以 500ms 中点样本和 0.3 EMA 估算
服务端时钟偏移，拍卖看板与 lot 详情统一使用修正后的当前时间；closing 提示恢复为剩余 15 分钟，
超过 24 小时按 `Xd HH:MM:SS` 展示，缺失/非法截止时间显示 `—`，过期状态使用五语言收尾文案；
这些都是展示层状态，不改变拍卖状态、竞价资格或服务端截止时间判断。

本轮继续对照 Public 拍卖页：预览/历史切换恢复旧版的轻量文字链接与分隔语义，拍卖标题恢复旧版
`--text-xl` 字号；历史拍品中的“返回拍卖历史”移动到届次、世界和已结算状态所在的同一标题栏，
不再单独占行；历史轮次导航、竞价动作和 URL 状态保持现有契约不变。

本轮继续对照 Public 竞价弹窗：普通竞价与代理弹窗恢复旧版的首个输入框聚焦、Escape/遮罩关闭、
`aria-modal` 内 Tab 焦点循环、背景滚动锁定和关闭后触发按钮焦点恢复；共用焦点生命周期下沉到
`auction-web-shared`，不改变命令 ID、服务端资格校验或失败重试语义。

竞价金额输入也恢复旧版即时解析反馈，并修正 `M` 后缀一位小数的十进制缩放：`1.5M` 现在严格解析
为 `1_500_000`，不再误算为 `1_050_000`；输入同时受浏览器 JSON 安全整数上限约束，服务端仍执行
最终金额、不变量和资格校验。

竞价弹窗的视觉与表单结构继续恢复旧版契约：标题携带 lot code，面板使用旧版紧凑宽度和纵向表单，
普通竞价、代理创建及降低上限都提供明确的取消/提交动作组；代理模式恢复为带详细说明的 Fast/Slow
单选项，最终阶段禁用 Slow 时把原因关联到对应控件。

lot 详情继续恢复旧版登录态代理摘要：读取同一权威代理投影，展示上限、模式、配额、可用动作、完整
变更历史和安全本地化状态；活动授权的入口改为“降低代理上限”，加载失败保留原位重试，列表行不展开
该详情以保持旧版目录密度。

普通竞价成功后的弹窗生命周期也恢复旧版行为：服务端接受出价后立即关闭弹窗、恢复触发按钮焦点并
刷新权威拍卖快照；失败时继续保留弹窗和同一输入对应的 `command_id` 供安全重试。代理授权成功仍
保留弹窗展示是否领先、当前状态、配额和完整授权变更历史；结果由弹窗父级持有，不会被随后的权威
代理数据刷新覆盖，并提供显式关闭入口，不与普通竞价错误地共用关闭策略。

本轮 Admin 用户、凭据和投递操作对话框也补齐了旧版的 Escape 关闭与取消按钮首焦点，敏感凭据
和生成的用户 key 仍只在一次性对话框中展示，不进入列表或持久化状态。

本轮 Admin 投递列表又补齐了来源类型的本地化展示，结算列表金额恢复旧版的 K/M 缩写格式；
包括 `operations_alert` 在内的已冻结来源都会显示业务名称，未知来源类型仍保留原始值，避免
丢失未来扩展的诊断信息。

本轮 Admin delivery 安全投影补回旧版的 COD 列：storage 只从 `schema_version=1` 且
`operation=send` 的内部 payload 提取非负 JSON 安全整数，HTTP 与浏览器只接收 `cod` 数值，
不暴露 payload 原文。

本轮又将旧版 credit 金额的 K/M 缩写、尾零裁剪和普通金额千位分隔收敛为
`auction-web-shared` 的共用展示规则，Public、Admin lot、结算和 delivery 页面不再各自维护
可能产生差异的格式化实现。

本轮还补齐旧版 delivery 安全投影中由 `result_metadata` 派生的 `reason_code`、`take_required` 和
`returned_mail_id`；这些字段经过格式校验后进入稳定 DTO，现已由 MoogleMail 收件人不存在的
协议拒绝链路生成；该链在退信资产取回前保持 `manual_reconcile`，原始 metadata 仍不会离开
storage owner。

本轮 Admin 拍卖总览恢复旧版在选择 `main` 或 `isekai` 时显示的手工装备录入表单；表单与 Draft/Preview lot 页
共用 `auction-web-admin/src/app/auction/intake.rs`，继续通过稳定 `command_id`、URL/canonical JSON、起拍价
和卖家玩家名称走既有 A04 契约，避免两个页面的录入校验和幂等语义分叉。

本轮还补齐 Admin 改名历史页旧版已有的空结果提示与分页控件；页面直接消费既有改名列表投影的
`page`、`page_size` 和 `total`，不新增接口字段或改变回滚命令边界。

本轮 Users 列表也补齐筛选无结果时的明确空态；用户字段缺失继续使用 `—`，列表无记录则使用独立
的本地化提示，避免两种状态混淆。

本轮 Admin lot 页也补齐旧版零 lot 空态；Draft 与 Preview 均可继续录入装备，有记录时才渲染 lot 表格；
改价/撤回仍仅限 Draft，分页和 intake 命令契约保持不变。

本轮又将 Admin 的拍卖、lot、代理历史、delivery、凭据、Users、改名、邮件入站、realtime、审计和
结算/recovery 列表统一接入旧版已有的直接跳页分页交互；页码上限仍由既有安全投影的
`page/page_size/total`（或已声明的页数兜底）决定，不新增接口字段、不在浏览器端缓存列表快照。

本轮继续补齐 Public 旧版共享 `Pager` 的直接跳页交互：拍卖目录、历史、竞价日志、价格历史、补偿池、
账户、lot 竞价历史均复用统一控件；已知总数使用 `total`，游标兼容响应只允许跳到当前可确认的下一页，
不改变 URL 状态或 API 契约。

本轮 Public 黑名单也恢复旧版的 URL 页码分页：请求使用 `page_size=10`，页码写入既有 `page` 查询状态，
页面继续只展示安全公开投影，不把黑名单原因或玩家标识扩展为新的写入能力。

随后使用旧版 8092 与新版 8090 的实际 embedded 制品完成 Public 14 条路径的 `en`、1440×1000
匿名/空态巡检。拍卖/竞价/价格历史、账户、黑名单和补偿池的一页 pager、空态间距及登录页几何已恢复；
补偿池修正了把需要 `$balance` 参数的 Fluent 消息直接用于无参数 `aria-label` 所导致的 WASM
`unreachable`，成功响应后不再停在 Loading。价格历史 Submit 的尺寸、字重和内边距，以及顶部世界
下拉的 Isekai/Main 选项顺序与 Main 默认选中状态也已逐项对齐。最终 14 条路径均无浏览器异常，三个
旧 alias 收敛到 canonical 地址；动态行、有效登录、竞价/代理动作、错误/冲突和移动端仍单独保留。

Public 补偿池又在新旧两端逐项切换 `en`、`zh-CN`、`zh-TW`、`ja`、`ko`，十组页面的 `hv.lang`、
`<html lang>`、picker、标题、0K 余额和空态文案逐项相同且无异常。这是一次性人工运行态证据，未新增
或运行浏览器/功能测试用例。

本轮 lot 详情装备属性表恢复旧版的每页 10 条分页；分页只作用于展示层，不改变装备属性安全投影、游戏
链接或竞价历史请求。

本轮继续对照旧版规则页：Public 规则正文和 Admin 编辑预览统一使用共享 Markdown/GFM 展示器，支持标题、
列表、表格、引用、代码块、强调文本和安全链接/图片；原始 HTML 只作为转义文本展示，不在浏览器端执行，且
规则页继续移除正文首个文档标题，避免页面标题重复。

本轮恢复 Admin 长标识的旧版可读性：审计、realtime、邮件入站、凭据、Users、改名、lot 代理历史、delivery
和结算/recovery 列表统一使用共享截断组件。表格按字段类型提供紧凑、中等和宽文本宽度，完整安全值保留在
原生 `title` 提示中；这只改变展示，不改变 DTO、分页、命令或敏感值边界。

本轮继续逐页对照 Admin 旧版交互：审计筛选恢复为字段变化后立即回到第一页并重新读取，移除额外的
“加载”按钮；delivery 列表补回投递 UUID 列；结算 lot 表补回旧版的扣留比例和挂牌价列，其中挂牌价在当前
安全投影没有该事实时明确显示 `—`，扣留比例只由已返回的税额/成交价展示层计算，不扩展后端 DTO。

Admin realtime failure 表也补回旧版可见的 failure/event 标识列；标识只取现有安全投影并在表格中显示短值，
完整值仍通过共享截断组件的提示保留，replay 命令和外部广播 sink 边界不变。

本轮继续核对 Public 拍卖生命周期：论坛镜像请求恢复为仅针对当前 live 拍卖，预览和历史拍卖不再因
不存在论坛主题而显示错误状态；live 拍卖的 Admin 取消动作恢复为旧版仅在 draft/preview 提供，领域层
仍保留截止时间与状态迁移校验；Public 登录 key 提交前恢复去首尾空白，避免与旧版请求语义产生差异。
论坛主题处于 pending/publishing 时每两秒读取安全投影，published 后降为每 30 秒读取；failed 或
存在同步错误时暂停自动轮询，直到用户手动重试，避免错误状态产生无界请求。预览拍卖到达 `opens_at`
且实时连接可用时触发一次权威当前拍卖刷新，并按拍卖 ID/开放时间隔离刷新状态。

本轮继续恢复 Admin 拍品列表与旧版的关键可见语义：列表首列使用按装备分类生成的稳定拍品编码，补回
提交编号的紧凑展示并保留完整值悬浮查看；软截止信息继续通过安全 DTO 中的授权编号提供旧版提示。
随后按产品要求把分类编号从纯展示提升为持久化不变量：每个拍卖的每个装备分类都从 1 独立递增，公开与
管理投影返回分类内编号；命令仍以 `lot_id` 标识拍品，价格历史 cursor、生命周期和结算顺序仍使用内部全局
`lots.lot_number`。已经发布的论坛主题保留旧全局编号方案，新主题使用分类内编号，且不暴露装备密钥或邮件凭据。

本轮还修正 Admin runtime 配置页的版本 fence 语义：候选校验返回的 fingerprint 只属于规范化候选，
不会覆盖当前 TOML 文件 fingerprint；保存时继续携带当前文件版本，避免校验修改后的有效配置被页面
自身错误地提交为过期版本。

本轮继续补齐 Public 黑名单与旧版的展示契约：恢复页面说明文本、表格移动端字段标签，并对玩家标识和
公开原因使用共享安全截断组件；完整值仍保留在原生提示中，不改变公开投影、分页或查询参数。

本轮同时补齐 Public 账户页与旧版的展示契约：恢复账户页和代理授权说明、移动端表格字段标签、领先竞价行
高亮及状态徽章；后续精确对照又恢复 `bid_page,page_size,proxy_page` 请求顺序、四列/六列表头、分页在各自
section 内的 DOM、账户专属 CSS 及五语言旧版词法，并移除认证页新增的说明和总数。竞价、代理授权的安全
数据投影、独立分页和操作语义保持不变。

本轮还修正 Public `/auctions` 的迁移残留文案：列表已经读取真实拍卖查询投影，页面改为说明世界/状态筛选，
不再显示“查询用例尚未接入”的过时提示，也不再复用历史价格页说明。

本轮继续补齐 Admin 手工装备录入与旧版表单的交互契约：改为原生提交事件，增加 URL、卖家、正整数价格
的前置校验和 canonical JSON 结构校验，补回必填、自动完成、提示文本与移动端可用的表单语义；成功提示
同时显示拍卖序号、lot 号和装备名称，输入错误与网络失败分开提示，五种语言资源保持同步。

Admin dashboard 继续恢复旧版运维信息的可见粒度：凭据卡只展示 MoogleMail `main`/`isekai` availability，
scope 计数、更新时间和重启要求保留在内部 owner；alerting 卡按旧 snapshot 展示 active/due/claimed/failed/
manual 与最近观测时间。Worker 通过 PostgreSQL 单例投影发布实际启用状态；关闭时返回中性的 disabled
健康结果，启用后的空扫描仍推进独立水位，配置状态时间倒退则失败关闭。浏览器严格拒绝同一路径的
额外字段，不新增敏感字段或外部接口。

Admin 拍卖总览继续恢复旧版表单语义：创建 draft 使用原生表单提交和必填时间字段，解析错误会归类为
输入校验失败；列表表头补回 readiness 列，和每行的可开放提示保持对齐。

Admin lot 列表继续补齐旧版表格契约：恢复 submission 独立表头，装备名称使用共享安全截断组件并保留
属性提示，代理授权历史表补齐移动端 `data-label` 和命令编号截断显示，避免宽表在小屏下失去字段语义。

Admin 邮件页恢复旧版的全局立即轮询入口：未加载具体 mailbox 时也可以触发所有已配置 poller，页面仍
使用稳定 `command_id`、刷新安全投影并复用原有成功/失败提示；已加载 mailbox 后的暂停/恢复动作保持不变。

本轮继续补齐 Admin 凭据导入表单的旧版提交语义：创建入口改用原生 `form` 提交，恢复标签和 secret 的
必填约束、字段关联和空值前置校验；提交按钮在输入不完整或请求进行中保持禁用，secret 仍只在当前请求
期间存在并在成功后清空。

本轮还恢复 Admin realtime 失败页的主状态筛选：全部、未解决、已解决现在与旧版一样位于页面主区域，
筛选变化会清空当前 replay 选择并回到第一页；流、消费者和错误码继续保留在可展开的高级筛选中，且
高级筛选改用原生表单提交，不改变 realtime 安全投影或外部广播 sink 边界。

本轮继续对齐 Admin 规则编辑页：标题恢复原生必填约束，规则校验入口改为表单提交，键盘回车可直接
触发候选校验；发布仍保持独立按钮、候选签名门槛和 revision/command_id fence，不改变发布契约。

本轮还将 Admin runtime JSON 编辑器恢复为原生表单：空文档先由浏览器必填约束拦截，回车或 submit
按钮触发解析与服务端候选校验；保存按钮继续独立使用当前 fingerprint、规范化候选和稳定 command_id，
不把校验结果误当作当前文件版本。

本轮又将 Admin recovery 确认框恢复为原生表单提交：需要原因的动作使用 required 文本域，回车与确认
按钮走同一提交路径；reclaimed 等无需原因的动作仍提交空原因，命令 ID、报告 state_version 和刷新
权威 recovery/settlement 投影的行为保持不变。

本轮继续补齐 Admin 用户创建弹窗：用户名和手动 key 字段恢复原生 form/required 语义，论坛账号 ID
为可选的规范十进制 `showuser` 值；回车可提交，空的手动 key 会在请求前拦截；生成 key 的一次性
展示、命令幂等和成功后刷新行为不变。

本轮还恢复 Admin Users 用户名筛选的旧版提交边界：输入先保留在草稿，按回车提交后才更新 active
查询并回到第一页；状态闭集选择仍立即应用，避免每次键入都触发列表请求。

本轮对照旧版部署探针，补齐 Public/Admin 的 `/health/live` 与 `/health/ready`：前者只报告进程存活，
Admin ready 通过应用层 readiness 端口只读检查数据库连接及当前二进制的全部 SQLx 迁移；Public ready
还要求 realtime relay owner 存活且最近成功消费未过期。缺少数据库、迁移未完成或 relay 失去新鲜度
时返回 503；探针继续使用旧版 `{code,message,data}` 外壳，不计入 feature API 数量。

本轮再次按旧版复核 Admin `/status` 页面：页面只保留本地化静态服务说明，不读取 `/health/live` 或
`/health/ready`。登录与已认证外壳仍独立消费 readiness 作为视觉隐藏的可访问性提示，探针 HTTP 契约和
业务 dashboard 语义均不改变。

本轮还补齐 `auctionctl migrate` 与旧版 `migrate-only` 的目标保护：执行 SQLx 迁移前只读检查迁移标记，
空数据库允许初始化，已有 `_sqlx_migrations` 历史交给 SQLx 继续做版本/校验和校验；没有迁移标记但存在
用户对象的未知数据库直接拒绝，不执行写入。

本轮继续按旧版 `AuctionPage` 对齐 Admin 拍卖总览：主表恢复“编号、世界、状态、开放、结束、就绪、
动作”七列结构，管理 lot 链接与生命周期命令重新合并到同一动作组，编号恢复 `#` 前缀；预览/开放
使用主按钮，取消/final phase 使用危险按钮，final phase 确认也保持危险动作语义。状态徽章恢复旧版
中性色，创建与生命周期弹窗统一使用旧版 32rem、有滚动上限和圆角的拍卖弹窗边界。新增的状态筛选、
服务端 capability、state-version fence 和命令幂等保持不变；真实浏览器产物仍需继续逐页核对。

本轮同时按旧版 lot 管理表恢复“编码、装备、卖家、提交、状态、起拍价、当前价、动作”八列结构及
固定列宽，慢速竞价调度信息重新收进状态列；改价、撤回与代理历史的顺序及主按钮/危险按钮语义恢复
旧版呈现。代理竞价历史从表格下方的内联面板恢复为可按 Escape 关闭的模态弹窗，lot 状态徽章和
改价/撤回弹窗也复用旧版中性色与 32rem 弹窗边界。当前 API 中额外的类别、来源、更新时间、版本和
幂等字段仍保留在安全 DTO 与命令链路中，只是不再挤占主列表；真实浏览器产物仍需继续逐页核对。

代理竞价历史弹窗继续恢复旧版紧凑行结构：标题带当前 `#lot` 编号，事件按“用户、代理上限、模式、
状态、授权/使用序号、时间”六项展示，不再把 command ID 与 state version 作为主视觉列；这两个字段
仍保留在安全 DTO 和服务端账本中。历史加载、错误重试、分页、授权 ID 提示和五语言状态文案保持现有
能力，弹窗表格使用旧版可横向滚动的紧凑表格边界。

Lot 页标题和返回入口也恢复旧版顺序与五语言文案：标题位于返回入口之前，已加载时用场次 sequence
展示“拍品 — 第 N 号场次”等格式，加载或失败时显示省略号，不再把内部 auction UUID 当作用户可见
场次编号；返回链接恢复前置箭头。

本轮继续收口 Web 首屏产物：Public/Admin 分别增加 crate 级 `Dioxus.toml`，固定 WASM 启动前的
`Auction` 与 `Auction 管理端` 标题；运行后的 `document::Title` 仍由五语言 Fluent 文案接管，避免
浏览器标签在客户端启动前显示 Dioxus CLI 默认标题。两个 surface 的 `just web-build`（内部使用
`dx bundle --platform web`）均已成功生成带预加载 CSS 的静态产物，并复制到
`target/dx/build/<package>/web/public/`，与后端 `rust-embed` 的 debug/release 嵌入路径一致。
后续 Public 14 条路径已完成 `en` 桌面匿名/空态浏览器巡检，规则和补偿池也取得五语言证据；
动态数据行、有效登录与写动作、错误/冲突、移动端逐页状态和 Admin 其余页面仍保留为后续验收范围。
Public 五语言资源中已经没有调用、且仍声称“数据接口接入后替换示例数据”的迁移期死文案同时删除。

本轮静态审计又清理了 Admin/Public 五语言资源中仍把已落地的拍卖生命周期、拍品修改、邮件处理、
投递、运行配置、规则内容、公开竞价和账户读取描述为“待迁移”的提示；这些文案现在改为说明实际
的 PostgreSQL 完整档、版本命令和安全投影边界，未把外部回执、浏览器和部署验收误标为完成。

Lot 改价与撤拍交互继续对齐旧版：表头和动作使用旧版短文案，弹窗标题使用 `One01` 等拍品编码而非
裸序号，撤拍警告明确装备将以 COD 0 寄回卖家且操作不可撤销。改价改为原生表单提交，支持回车与
required/step 语义；本地非正整数错误与 API 更新失败分别展示，二者不再共用误导性的网络错误反馈。

Lot 行还补回旧版供辅助技术读取的“#序号 + 装备名”隐藏标题；装备截断提示改为把名称和完整属性 JSON
放进同一个 `title`，避免嵌套元素的 title 覆盖导致属性详情实际不可见。

改价和撤拍成功后现在恢复页面级 `role=status` 反馈；提交新动作会清除旧反馈，成功后再刷新权威 lot
列表，避免仅关闭弹窗却不给管理员完成确认。可选卖家和提交编号为空时也恢复旧版空单元格呈现。

拍卖生命周期弹窗继续按旧版补齐届次与动作语义：预览、开放、取消和 final phase 标题都显示当前
sequence；Preview 正文说明仍可继续录入，Open 正文说明此时才把后续装备切换到下一期，取消说明托管
装备退回，final phase 说明不可撤销；取消原因同时
恢复“写入审计历史”的提示。弹窗改为原生表单提交，取消原因获得首焦点，其他动作首焦点仍在取消按钮；
surface 级焦点陷阱负责关闭后恢复触发控件。成功结果改回页面级 `role=status`，并按动作显示五语言文案，
失败仍留在动作行供重试；表格首列也补回旧版供辅助技术读取的场次隐藏标题。

本轮生产边界审计又收口了时间与版本溢出路径：Admin 邮件状态版本、Worker 的 lifecycle/settlement/
fulfillment/recovery/forum/MoogleMail 退避、E-Hentai 限速截止时间以及 Public realtime 的连续序号
现在统一使用 checked 计算；溢出时失败关闭、断开或要求重新获取权威快照，不把异常值折回当前时刻、
静默提前重试或伪造连续事件。审计同时复核了生产 `unwrap`/`expect`/`panic` 和日志字段，剩余调用仅为
启动期已校验的内置模板、已由外层闭集保证的不可达分支和既有测试代码；测试编写与运行仍按用户要求暂缓。

竞价边界也已完成同一收口：`BidAmount` 不再把超出信用网格的下一口金额饱和到最大值，固定增量
与 PPM 增量均以 `AmountOverflow` 失败，并沿普通竞价、代理竞价、慢速代理、Public lot 投影和
账户代理能力查询链路传播；不合法的非网格 `minimum_increment` 会在规则校验阶段拒绝。

本轮继续收口边界：Auction Worker 的派生 claim lease 与 Public 补偿池/账本页的条目数量、序号和
页尾计算均改为 checked 转换；异常大小或派生溢出会拒绝配置/公开投影，不会静默饱和或接受未校验索引。
Application、HTTP 和存储层的 Public/Admin、账户、实时与价格/竞价历史分页投影，以及 retention
inventory 汇总，现也统一使用 checked 转换和加法；MoogleMail 附件计数、Worker control 计数和
协议投影超出可表示范围时拒绝处理，不把异常值映射为最大整数。

只读 `auctionctl deployment-check` 复核又发现 URL 解析器会把无路径 origin 表示为根路径 `/`，导致示例
Public/Admin 配置和论坛 `public_base_url` 被错误拒绝；配置层与 HTTP origin 策略现统一接受空路径/根路径的
等价表示、规范化末尾斜杠并拒绝语义重复项，未放宽凭据、查询、片段或非根路径限制。

同一轮部署安全复核还将 `deployment-check` 的文件边界与实际 `deployment start` 对齐：配置目录、三个主
配置、runtime 配置和敏感 TOML 均拒绝符号链接；严格模式下主配置/runtime/目录拒绝 group/other 写入，
敏感文件继续要求无 group/other 权限，非严格模式仅报告权限警告，避免示例目录无法被只读预检加载，同时
避免只读预检与真正启动使用不同的安全标准。

本轮目录结构复核又收口了剩余的同前缀文件：连接池归并到 `database/pool/{mod,metrics}.rs`，MoogleMail
详情解析归并到 `parser/detail/{mod,helpers,parse}.rs`，Sent 列表与详情归并到
`parser/reads/sent/{mod,detail}.rs`；所有 app/crate 的 `src` 根仍只保留薄入口文件，职责和公共模块
路径不变。这样同一职责目录不再同时堆放 `pool*`、`detail*` 或 `sent*` 文件。

本轮又完成部署模板的离线工具校验：Caddyfile 通过 `caddy validate --adapter caddyfile`；systemd
示例先映射为合法 `.service` 文件，并在临时 root 中提供占位可执行文件与基础 target 后通过
`systemd-analyze verify`。该阶段只证明模板语法、依赖和字段形状；后续 OPS-003 已补充真实 unit 安装、
systemd 顺序、readiness、回滚和隔离凭据文件边界，仍不外推为生产流量切换。

本轮重新静态核对旧版与 Rust 浏览器路由：Public 的 14 条、Admin 的 21 条路径分别由 Dioxus
`Route` 枚举、`auction-http/src/assets/routes.rs` 的精确/参数 allowlist 和旧版别名清单共同覆盖；
Public 的 `/lots/:lot_code` 与 Admin 的 `/auctions/:auction_id/lots` 参数路由也逐一对应。未匹配
路由仍只在 Dioxus 内部进入 NotFound，后端不会因此开放未知 SPA 入口；这项静态证据随后由 Public
14 条路径的 `en` 桌面匿名/空态巡检补充了导航、深链接和当前布局 R1，但仍不替代动态数据、认证动作、
错误/冲突、移动端和 Admin 其余页面的浏览器验收。

后续在旧对照实例定位到唯一且 clean 的 PostgreSQL 18.6/canonical v28 数据库，并按私有临时目录、
专用角色和同名隔离库边界完成 OPS-001 全链。真实执行同时修复 PG18 关键字别名、连接池自会话、
普通 owner 读取 `pg_user_mapping` 和显式 `pg_default` 权限四类阻塞；最终接管 receipt、SQLx 16、
规则五语言、当前备份、rebuild OID 变化、空库 preflight 和 restore receipt 均成立，恢复前后摘要一致。
该旧副本核心业务表为空，因此证据只关闭 schema/灾备运维链，不替代非空业务历史或生产切换验收。
`deployment-check --strict-permissions` 对示例目录按预期以退出码 2 拒绝 group/other 写入的结论保持不变。

OPS-002 发布制品链也已按 Auction-BF 的实际 Makefile/release-check 合同收口。Public/Admin 新增显式
`web-embed` feature，API-only 与 embedded 四个 PostgreSQL ELF 在独立 Cargo target 中生成；无 feature
时静态 owner 指向确定缺失目录并保留明确 503，不会因为本地已有 Dioxus 文件而偶然嵌入。Web 构建每次
清理 Dioxus 实际 release 中间目录和稳定目录，使用优化 WASM profile、关闭 debug symbols，并生成
逐文件哈希与版本化 payload manifest；由此发现并修复了旧指纹 JS/WASM 在中间目录残留的问题，两个
surface 现在各自只有六个 payload 文件和一组 JS/WASM。制品核验扫描 ELF 中的 payload 摘要和 title，
证明 Public/Admin 只携带自己的 surface，Worker/auctionctl 与 API-only 不携带 Web payload；隔离
PostgreSQL 运行探针又证明 API-only 两端根路径为明确 503，embedded 自有资源 200、跨 surface 资源 404。
确定性 package 使用成员 allowlist、固定 mode/owner/mtime、双次 archive 哈希比较、内部 SHA256SUMS、
源码指纹和依赖摘要；正式模式拒绝 dirty，显式本地证据包会在名称与 manifest 中记录 dirty 状态。
发布制品本身不替代部署生命周期证据；后续 OPS-003 已在同宿主机隔离环境消费 embedded 制品并完成该层 R1。

OPS-003 使用 systemd 257、Caddy 2.11.4、四个独立 unit、18590/18591 应用端口、18680/18681 代理端口和
临时 PostgreSQL 完成受控演练。严格 deployment-check、Caddy/systemd 校验和 SQLx 16/16 前向迁移通过；
三角色按 Worker → Admin → Public 依次 ready，restart 先按 Public → Admin → Worker 停止再正序启动，
systemd InvocationID 与 0600 root 回执匹配。直接 metrics 为 200，Caddy `/metrics` 为 404；SIGTERM 日志
逐角色出现 drain/stopped 且退出码为 0。把隔离 Public unit 临时改为 `/usr/bin/false` 后，控制器以
`start-failure-v1=public-readiness` 和退出码 4 失败关闭，再按 Public → Admin → Worker 回滚；恢复 unit
后重新启动/停止成功。三角色 journald 敏感模式扫描为零命中，最终已删除全部临时 unit、目录、端口、
数据库和角色。该 R1 不包含真实域名/TLS、专用生产账号、正式 secret、外部服务和真实流量，因此不构成
生产切换批准。

本轮还以 `cargo build --locked --workspace --all-features` 完成实际链接构建，并确认
`auction-worker`、`auctionctl`、`auction-public-api` 和 `auction-admin-api` 四个 ELF 部署入口均已
生成；这只证明当前 Linux 工具链的制品链接完整；隔离 systemd 安装已另行取得 R1，专用生产账号、
正式凭据注入、域名/TLS 和流量切换仍需真实环境批准。

本轮同时以 Rust `1.94.1` MSRV 工具链完成
`cargo +1.94.1 check --locked --workspace --all-targets --all-features`；MSRV 编译通过，
未运行测试，该命令只做编译检查。

本轮 Batch D 又完成论坛同步两阶段恢复与 PostgreSQL `000016` 人工授权账本、MoogleMail
`Retryable`/`Permanent` 处理分类、旧版 Admin 业务 runtime 到 Worker 启动快照的映射、Admin
稳定错误反馈和凭据表列，以及 metrics listener、origin 默认、Admin 会话和装备来源节流四项配置
差异。隔离 SQLite/PostgreSQL 运行态又取得 Admin 配置事实卡、单类稳定错误和凭据列的局部认证浏览器
R1；共享语言选择器也修复了刷新后原生下拉与 `hv.lang`/`html lang` 分离的问题，并在 Admin 制品逐项
切换五种语言。真实论坛/邮箱回执、其余认证错误、配置保存和 Admin/Worker 重启仍按
`legacy-parity-audit.md` 保留 R1，且本轮没有新增或运行任何测试用例。

本轮 EVD-002 又使用旧版 8092 与新版 8090 的 embedded 制品完成 Public 14 条路径的 `en` 桌面
匿名/空态巡检，并逐页校正共用分页、历史筛选、登录/账户 CTA、黑名单和补偿池布局。首页与登录页的
新旧截图分别取得相同 SHA-256；`/history` 的世界选项顺序、默认值和 Submit 计算样式逐项一致。
补偿池修复了缺少 Fluent 参数导致的 WASM trap 后，五种语言的 `hv.lang`、`html lang`、picker、标题、
余额和空态文案均与旧版一致且无浏览器异常。动态数据行、真实竞价/代理、有效登录、错误/冲突、移动端
逐页状态和 Admin 其余页面仍未覆盖；该轮是一次性人工对照，没有新增或运行浏览器/功能测试用例。

本轮论坛/WebUI 同步性修复新增 PostgreSQL `000029`，把逐主题同步健康与下一次扫描时间、论坛身份
变更历史和审计事实纳入权威存储；固定帖编辑以原始 BBCode 的前后逐字一致证明收口，实时消费者增加
唯一 active owner（PostgreSQL session lock/SQLite 同主机 sidecar lock）、权威重读后的 sequence 基线和
单一周期 HTTP 回读，Public/Worker readiness 分别反映 relay 与论坛周期健康；单主题失败即使已经安全
写回也会降级本轮健康，过期租约只在 token/owner/fencing 仍未被替换时允许记录失败。公开论坛页同步
投影与错误暂停轮询、全局竞价日志筛选及历史拍品标题栏同时收敛。
本轮只完成源码与静态门禁，不宣称真实论坛编辑、异常帧重连或浏览器逐页交互 R1，也未新增或运行测试用例。

本轮又增加 PostgreSQL `000030`，把 `mail_cookie`/`forum_cookie` 各自唯一 active 的约束放入数据库，
并用按 kind 的 session advisory ownership 防止两个 Worker 同时使用同类 Cookie；失效后不自动轮换，
替换需要重启 Worker。MoogleMail 明确返回 `recipient_not_found` 时，原 delivery 保留退信资产后果并
进入人工对账，当前轮次只暂时跳过同一 `(world, recipient)` 的后续 delivery，其他玩家继续处理。
2026-09-02 对 `11111` 的一次零资产与一次 1 Credit 受控发送均被远端接受，未复现不存在收件人；
因此没有针对该名称写死规则，异常分支仍以远端明确协议响应为唯一判据。

本轮又扩展 A39 开拍前计划修改：Draft 可同时修改未来且严格有序的预览、开拍、结束时间，Preview
锁定已发生的预览时间，只允许修改未来的开拍与结束。PostgreSQL 在同一 Serializable 事务内同步
拍卖投影、schema v4 结束策略和持久调度三项 due；活动租约阻止修改，过期租约与失败退避可由实际
变更清除，Worker 完成/失败写回同时校验 owner、租约截止和领取后的轮次版本，陈旧 claim 不能越过
计划变更。`auction.schedule_updated` v1 仍只作权威快照重读唤醒，不原地扩展冻结 payload。

Public 独立 `/forum` 页面在发布后只显示可点击的帖子位置和“已更新到 #N”。新增 PostgreSQL
`000031` 为 `auction_forum_sync` 保存由完整主题读取证明的连续处理楼层，初始水位为固定系统帖 #2；
扫描遇到待重试命令会把水位钳在该楼之前，无正文变化的完整扫描也会推进并释放 claim。该水位不从
固定帖 `content_revision` 或远端 post ID 推算，SQLite Lite 继续不提供论坛完整档能力。

本轮追加 PostgreSQL `000032_preview_intake_handoff`，将 Draft 与 Preview 统一为当前录入窗口：Preview
期间 Admin/MM 继续录入当前拍卖并原子更新预览物化项、revision、审计和 realtime；Open 才创建后继
Draft、下一轮调度并迁移录入目标，Preview 取消也保证留下可用后继。迁移以唯一索引、状态匹配 CHECK、
延迟一致性触发器和世界级事务 advisory lock 关闭跨表并发缺口，并拒绝旧 Preview+Draft 重叠窗口。
MoogleMail 自动轮询 cron 已改为可配置且默认 `*/5 * * * *`；Worker 启动会安全收敛旧配置遗留的
过晚持久化调度水位，而不解除暂停、覆盖活动 claim 或跳过失败退避。本轮未新增或运行测试用例，
最终静态门禁与 Auction-Test 运行态证据在部署后记录。
