# Auction-BF → Rust 迁移实施计划

## Context

本仓库当前处于**功能迁移后期**：工作区、四个进程装配、健康检查、TOML 配置、双数据库适配器
边界、Dioxus 双 surface 与 5 语言已建立；旧版源码能力的静态盘点已收敛，当前工作重点是保持已有
静态门禁，并完成真实外部服务、浏览器产物和部署切换的行为对照。

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

当前已提交 PostgreSQL `000001` 至 `000015`：覆盖基础身份、拍卖与 lot、命令账本、
竞价事实、代理授权及历史、审计/realtime、结算/履约、生命周期、身份/外部命令、恢复/
可观测性、论坛同步、Final phase 调度、领先时间修正和生命周期/operations alert 数据库不变量。旧名 `bid_proxies` 由只读兼容视图提供，不再维护第二份权威
状态。`auction-storage` 已接入 PostgreSQL 竞价与结算事务适配器：命令账本重放/冲突、
权威状态锁、竞价/结算事实、履约初始状态、审计和 realtime 事件在对应事务内提交。
SQLite `000001` 至 `000007` 及迁移入口已追加，
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
身份，以及 `pending → prepared → enqueued` 的履约状态推进；recovery action 也已使用同一
两阶段本地 delivery 身份和 action fence；`deliveries` outbox 也已接入
持久化领取、发送中 fencing、成功、失败和 `manual_reconcile` 终态边界。MoogleMail 入站
读取适配器已经完成固定 endpoint、无重定向 GET、响应边界、分页 cursor、allowlist-first
详情读取和严格 HTML/脚本/附件解析；PostgreSQL `000008_mooglemail_polling` 迁移与
`MailPollStore` 又接入了 due claim、lease/fencing、连续失败退避，以及
`inbound_mail`、`mail_commands` 和 cursor 的同事务幂等提交。Worker 通过
`[worker.mooglemail]` 显式启用后，会按顶层 world 与 `additional_worlds` 为每个已配置世界装配
独立的持久化邮箱状态、定向 claim、读取 gateway、业务 processor、拍卖入站事务和 delivery
outbox；配置独立 `credential_data_key` 且使用 PostgreSQL 时，Worker 优先从 A32 active 凭据池
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
持久化 claim/fencing 和 `write_unknown` 人工对账；回帖读取先通过登记身份、lot 映射和命令账本，
再调用现有竞价端口。MM `notify` 已进入 MoogleMail delivery outbox；结算事务现在会为每个获胜
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
端口、存储适配器及 Worker 循环。再次核对旧版 `realtime/relay.go` 后确认，其 relay 只把持久化
事件广播到当前 Public 进程内 Hub，并不存在外部广播协议；当前 Rust 持久化 relay + Hub 已达到
该旧版边界。真正的外部广播 sink 属于新增系统能力，不再列作旧版功能等价缺口；如后续需要，仍须
另行冻结协议、幂等和回执 owner，不能伪造远程转发成功。
A05 已先落地为纯本地论坛预览解析：HTML/文本输入
在 2 MiB 边界内解析为 catalog、posts、warnings；E-Hentai 远程主题读写和 PostgreSQL 回帖
命令导入已在独立 Worker/集成端口接入，解析器仍不直接写数据库或拍卖状态。

**凭据通道：** A32 已将生产外部 Cookie 的管理入口拆为独立的
`credential_data_key_id`/`credential_data_key` 密钥域。Admin 只在 PostgreSQL 完整档提供凭据
导入、改名、启停、删除和安全投影读取；Worker 按 world/shared 读取 active 密文池，在内存
解密后按顺序尝试，密钥不匹配、密文损坏或已有 active 行全部无法解密时失败关闭；暂时没有
active 行时保留 degraded 运行态，实际操作返回 credential unavailable。该密钥不能复用装备或用户
登录 key，也不能进入日志、DTO、快照或提交内容。凭据 dashboard 已补齐 scope 最近更新时间、
最近 Admin 变更时间和 Worker 启动心跳重启判断；Worker 未被可靠观测时保持 unknown，不把未验证
的凭据加载状态误报为成功。

兼容回退仍允许 `auction-config` 的 `ehentai_cookie()` / `mooglemail_cookie()` getter，以及
`auction-config::load_mooglemail_keyenv` 读取 `.keyenv` 的两个固定键：`hv-ck-key1`（Bot）和
`hv-ck-key2`（测试号）。`.keyenv` 入口只允许 owner-only 文件权限和固定键闭集，不构成通用
环境变量覆盖；值仍由 `SecretString` 脱敏。

---

### 阶段 7：浏览器端真实数据接入

14 + 21 条浏览器路由、5 语言和 BEM 视觉契约均已建立。Public/Admin 正式页面都已接入真实
API；通用 panel/placeholder 只用于加载、错误或空结果，不再代表未实现页面。剩余 Web 工作是
在真实浏览器产物中逐页核对 Admin 视觉、交互和响应式细节，以及外部服务返回的错误状态。

**顺序：** 只读优先——拍卖列表 → 拍品详情 → 竞价记录 → 登录 → 出价 → 代理 → 管理命令。

**实时更新用 WebSocket**（`/api/v1/ws`），与旧 `realtime/hub.go` 契约一致；事件带
sequence/version，跳号或重连后重新读取权威快照，**不在浏览器端累积状态**。当前已先把
`/api/v1/events` 的持久化 Catch-up 已接到 PostgreSQL/SQLite 事件表，支持拍卖流、after 游标、
有界 page_size 和 `refetch_required`；`/api/v1/ws` 已接入 Public session、只读服务端推送、
128 个并发连接上限和 ping/pong，并始终以持久化 Catch-up 和权威快照作为恢复依据。
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
exporter。基于真实 readiness/audit/tracing 的周期心跳安全状态仍待收口。`auction-storage` 已在
SQLx 池的普通查询、事务和显式 acquisition 边界
接入旧版 pgx 对应的十项连接池指标；固定低基数的竞价、代理和主要 Worker 业务结果/耗时指标
也已接入。绕过该边界的外部原生池不纳入累计统计。
operations alert 已由
`auction-application` 定义信号/evaluator 与窄端口，`auction-storage` 负责 PostgreSQL 完整档
信号、批次、lease/fencing、三次 Create 尝试、成功入队后的 cooldown、delivery 状态和稳定终态
对账，`auction-jobs` 负责扫描/通知入队，
`auction-http`/`auction-web-admin` 已接入 alerting dashboard section。SQLite 明确返回能力缺失。
Worker control 协议本身已按 UDS/peer credential 边界接入。

**测试边界：** 按 `AGENTS.md`，在用户明确要求前不新增或运行单元测试、功能测试、集成测试、
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
PostgreSQL `000001` 至 `000015` schema 与竞价/结算/生命周期事务适配器已接入；阶段 4 的
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
和 owner capability；当前 `501 feature_not_supported` 只表达 SQLite Lite 或未装配依赖的明确
能力边界，不再代表 PostgreSQL 完整档的占位 handler。P18/P19 代理创建/降低命令已返回旧版
`{ authorization: ... }` 完整命令投影，quota、转换历史和终止原因由提交后的安全账户快照补齐，
并在授权状态版本不一致时失败关闭；代理重入的 `usage_number` 在 lot 锁内按用户历史最大值递增，
避免重入和命令重放混淆；Admin A06 lot 列表已接入
PostgreSQL Serializable 同快照的拍卖/lot/装备/投稿投影、Admin 会话保护和管理端表格，SQLite
Lite 因缺少完整生命周期字段明确返回 `501 feature_not_supported`。Admin 的 settlement-report
与 recovery-report 已接入 PostgreSQL 完整档一致只读/安全快照，SQLite 明确返回 `501 feature_not_supported`。Admin 的 preview/open/cancel/
final-phase 已接入统一生命周期端口；preview 建立唯一后继轮次并物化预览，open 校验时间与前序状态，
cancel 只允许开放前执行，final-phase 在锁内转换可继续的 slow proxy 并冻结失格授权。Admin 请求使用状态版本 fence，取消要求原因，并在同一数据库事务内写入命令账本、状态版本、取消元数据、追加审计和
注册表内的 lifecycle realtime 事件；A03 GET/POST 拍卖已接入 Admin 会话、旧分页/筛选、PostgreSQL Serializable 生命周期/lot 数量快照、管理端表格和创建表单；其创建命令已接入零版本 fence、命令账本、world/intake target 锁、策略快照、审计和 `auction.created` 事件，主配置 `[auction]` 模板会同时写入 schema v4 快照与遗留规则列；SQLite Lite 创建与列表均返回 `501 feature_not_supported`。Public `/api/v1/events` 已接入 PostgreSQL/SQLite
持久化事件回放、sequence 游标和 schema 安全边界；Worker 已接入 `scheduled_rounds` 的持久化 due claim、
租约、尝试次数、preview/open/final 生命周期调用和失败退避，并在 PostgreSQL 完整档接入结算
workflow 的 due claim、稳定 `settle_auction` 命令、精确拍卖/lot/代理锁、代理关闭、sold/unsold
结果、完成 fence 和失败退避；SQLite Lite 明确
不启用结算 workflow。公开端首页看板、lot 详情、规则文档、公开黑名单、补偿池/账本、论坛镜像、竞价日志、成交价格历史和 `/status` 持久化实时事件游标页已接入同源 Public API；
管理端 delivery 页面已接入状态筛选、数字分页、release/retry/reconcile 命令和旧版确认/对账交互，
操作携带 `command_id`、CSRF 和 fencing version；manual reconcile 的 succeeded/failed 决策分别
携带外部引用/失败代码。PostgreSQL 完整档还
接入结算后的履约 claim/fencing、delivery outbox 的持久化状态推进和重试边界；公开端拍卖看板
已按实时事件重新读取权威快照，管理端拍卖列表已提供带 CSRF、全新 `command_id` 和成功/失败反馈的
生命周期操作；基础通知与结算 MoogleMail transport、Sent/Balance 远程回执对账、论坛主题同步和
回帖竞价导入已接入；Public WebSocket 已补齐客户端事件信封/schema/连续游标校验，以及断档后
`refetch_required` 权威快照恢复；Public 登录错误码/封禁公开原因、页头会话菜单、登录 key 清理、
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
Admin A03 GET/POST 拍卖、A06 lot 列表、A15 代理历史和 A25 规则 GET/POST/PUT 已接入 PostgreSQL 完整档的一致安全投影、有限页码/opaque cursor、Admin 会话保护和管理端真实页面；A03 生命周期按钮现在按后端 capability 安全投影过滤，列表补齐预览/开放/结束时间线，A06 lot 页面接入真实页码分页、返回拍卖列表、加载失败重试、draft pending lot 改价/撤回确认、软截止信息、设备属性详情、金额/时间格式化以及代理历史关闭/重试交互；A03 POST 的策略字段现由主配置 `[auction]` 经应用层模板冻结，并同时写入快照与遗留规则列；A04 装备录入已接入 URL/canonical JSON 来源、短生命周期明文 key、AES-256-GCM 密文信封、PostgreSQL Serializable world/intake target/玩家/装备锁、同一事务的装备/密文/投稿/lot/版本/审计/realtime 写入，以及草稿 lot 管理端表单；密钥标识与材料只允许来自敏感 TOML，SQLite Lite 明确返回 `501 feature_not_supported`。A25 PUT 使用规则 revision CAS、命令账本和审计，管理端规则页已补齐旧版的语言标签、动态新增 locale、默认语言提示、版本事实、服务端规范化校验、校验门槛、轻量 Markdown 预览、稳定 command ID、发布冲突和重试反馈；SQLite 规则只读，发布明确
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
数据库/schema readiness、进程内审计写入失败计数和各进程 tracing 安全状态。Worker control
通过安全 runtime 目录、实例锁、固定
group/0660、inode 清理和显式 UID/GID peer allowlist 保护的 Unix socket 接入 live/ready/status，
无数据库启动骨架启用同一 control 时也只报告进程存活和零
poller，不伪造持久化就绪；Admin 现在消费完整安全 dashboard snapshot，展示 health/process、Worker、
tracing、realtime、credential scope、mail/realtime/delivery 已接入
轮询/入站、consumer lag/failure 和 delivery outbox 的安全汇总，fulfillment/recovery 复用
operations-alert source 的履约/恢复水位，credential 按 kind/world 聚合与 alerting section
也已接入；外部广播仍不属于本地 dashboard 汇总。
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
结构化后置证明：return 证明目标 `mid` 从收件箱完全消失，take 证明身份不变、资产归零与回执
精确匹配。结算退信、Sent/Balance 回执对账和 recovery 后续 action 已接入
PostgreSQL Worker；recovery 付款超时提醒由 recovery owner 接管，MoogleMail 协议层不拥有
recovery 业务 owner。Worker 的 MoogleMail 配置还接入旧版
`equipment_filter` 的闭集校验、装备属性/交易状态判定和退回原因；旧版 `mail_templates`
现已迁移为受校验的结构化 TOML 字段，并在 Worker 启动时冻结为模板快照。论坛扩展通知仍
保留 Rust 固定文案，因为它不属于旧版模板闭集，后续若要配置化需先冻结跨模块契约。
MoogleMail 调度现已按 world 拆为独立循环，补齐旧版读取/业务处理各三次的超时重试、指数退避、
周期抖动和全邮箱手动唤醒；Rust 的持久化 claim 在批次执行期间额外续租，失去 fence 后取消陈旧
处理，不能使用过期 token 推进 cursor。

A26/A27 Admin runtime 配置首片已接入：`auction-application` 的严格非敏感文档模型、稳定
fingerprint、版本化 request hash、pending/applied/unknown 恢复服务和窄端口已落地；Admin
对外就绪前会先执行同一恢复服务，可证明的命令自动收口，`unknown`/文件冲突保留人工核对
告警，其他恢复错误阻断启动；
`auction-config/src/runtime/` 负责 regular-file/权限/锁校验以及同目录 fsync + rename 原子
替换，`auction-storage` PostgreSQL 适配器负责 `runtime_config_admin_commands`、审计外键和
命令重放。`auction-http` 已接入 GET/validate/PUT，`auction-web-admin` 的 Config 页面已
使用 JSON 编辑器、候选校验和重启提示，五种语言文案已同步。SQLite Lite 明确返回
`501 feature_not_supported`；runtime 更新不热改当前进程，文件写入成功后必须重启 Admin。配置页
已补齐旧版的 JSON 对象解析提示、服务端候选规范化校验、校验签名门槛、稳定 command ID、
fingerprint 冲突反馈、重新加载和加载失败重试。
重启后的运行链也已收口：`allow_manual_early_open` 同时驱动 HTTP capability 和生命周期服务，
计划时间之前的人工开放在锁内按同一启动快照失败关闭，计划时间已到的开放与 Worker due 命令不受
影响；`[equipment_source]` 则映射为 host/path allowlist、响应上限、共享请求间隔和全请求超时，
适配器在固定 HTTPS origin、无重定向和精确 world 路径之上实施流式有界读取。
Admin runtime listener 已接入 TCP 与 Linux Unix socket 启动装配；Unix 模式会执行 runtime
目录、目录级单实例锁、stale socket 身份、owner/group 和 `0660` 权限检查，Worker control
socket 路径仍由同一 runtime 文档选择。`auctionctl deployment-check/start/restart` 已恢复跨进程
拓扑校验：Admin/Worker control socket 与三组数据密钥必须一致，Public/Admin 实际 TCP/Unix
listener 不能冲突，Admin readiness 跟随 runtime 选择的 listener；部署探针区分 Public/Admin
HTTP 的 `/health/ready` 与 Worker control 的 `/v1/health/ready`。其余真实部署与切换行为仍需外部对照。
systemd Admin 模板也已开放 `/run/hv-auction`，Unix 示例 group 与模板运行组保持一致，避免代码
支持 Unix 而受保护 unit 无法创建 socket 的装配分叉。

A28/A30/A31 用户管理首片已接入：`auction-application` 声明用户安全投影、创建、改名、游戏
身份修改、登录 key 撤销/重签、黑名单/解封和改名回滚端口；`auction-storage` 在 PostgreSQL
Serializable 与 SQLite `BEGIN IMMEDIATE` 事务内完成 state-version fence、命令账本、密文
key 信封、改名/黑名单追加历史和审计；`auction-http` 接入 `/users`、改名回滚和黑名单路由，
管理端 `/users` 页面已接入列表、创建、改名、筛选分页、黑名单历史/次数/创建时间展示、游戏身份修改、key 撤销/重签、黑名单与改名回滚
交互；创建用户改为旧版风格的对话框，自动生成的登录 key 和重签 key 都通过独立的一次性展示框交付，
不会出现在用户行反馈或列表中；游戏身份、key 和黑名单动作已补齐旧版确认对话框及黑名单原因输入，并同步五种语言；A29 的改名历史列表同时返回当前用户版本，页面用该版本提交 A30
回滚命令。MoogleMail 注册/换绑仍只在 PostgreSQL 完整档，
Admin 用户管理的 Lite 支持不代表完整外部邮件能力已经对等。

A32 凭据管理首片已接入：`auction-application` 声明凭据种类、world/shared scope、secret
校验、命令摘要、版本 fence 和安全投影端口；`auction-integrations` 使用独立
`credential_data_key` 为 MoogleMail/E-Hentai Cookie 生成带类型域的 AES-256-GCM 密文信封。
PostgreSQL `000009_credential_admin_compat` 补齐 `forum_cookie` 闭集，并允许删除凭据后保留
命令账本与审计证据。`auction-storage` 在同一 Serializable 事务内完成导入、改名、启停、
删除的凭据行、命令账本和追加审计；`auction-http` 与 Admin 页面只接触安全投影，管理页补齐
kind/world/status 筛选、数字分页、改名对话框和删除确认，导入 secret 不会回显。Worker 配置独立密钥后可按 world/shared 读取 active 凭据池并在内存解密，远端确认
登录失效时按稳定候选顺序轮换、幂等回写 `invalid` 并从当前进程池移除；Admin dashboard 已接入
按 kind/world 聚合 active/disabled/invalid 安全计数、scope 更新时间和最近 Admin 变更时间，并结合
Worker 启动心跳判断凭据是否需要重启；凭据 section 还分别校验 MoogleMail `main`/`isekai` 的
active、invalid 和更新时间，未配置独立密钥时继续使用 `.keyenv`/敏感
配置兼容入口；Admin mail section 在配置 `dashboard_mailboxes` 时还按 Admin runtime 投影核对全量
mailbox poller；
SQLite Lite 明确不提供该完整能力。仍需在真实外部服务和旧版数据上完成切换前
对照，尤其是写入开始后的未知结果边界。

## 对比旧版的当前复核矩阵

截至 2026-08-24 的静态复核结果如下：旧版业务 HTTP 路径在 Rust 中无缺项，Public/Admin 浏览器
路由完整对应，旧版 Worker 循环类别和五类 recovery action 也都有装配。下表把已完成代码、仍需
外部/浏览器/部署证据的部分和明确不在旧版等价范围内的扩展分开；静态复核不能替代行为验证。

| owner | 旧版能力 | 当前状态与下一步 |
| --- | --- | --- |
| `auction-domain` / `auction-application` / `auction-storage` / `auction-jobs` | 预览、开放、Final phase、关闭与结算的完整生命周期 | preview 后继轮次、物化预览、open 时间/前序约束、开放前取消、Final slow proxy 转换/失格冻结、持久化 Final due fence、结算前 due 校验、精确 lot/代理锁、代理关闭、sold/unsold 投影和 `leader_since` 连续领先语义均已落地；PostgreSQL `000012`/`000013` 与 SQLite `000005` 至 `000007` 补齐调度和历史投影，PostgreSQL `000014` 恢复身份不可变、前序顺序和 draft/intake target 的数据库级防线。Auction 生命周期/结算已从集成 tick 拆为独立 `60s + [0,5s]` 主循环，两类各自最多 100 条、PostgreSQL 8 并发波次、30 秒单项总时限，SQLite Lite 有效并发固定为 1；一类错误不再跳过另一类或终止其它 owner。剩余是用真实旧数据对照边界时刻、并发锁等待和切换前运行证据。 |
| `auction-domain` / `auction-application` / `auction-storage` | 拍卖经济策略在竞价运行时的完整应用（PPM 增量、代理开关/默认模式/次数上限、final phase、旧版默认值和 intake 起拍价统一） | 已完成主要运行时切片：主配置 `[auction]` 经校验后冻结为 schema v4 `policy_snapshot`；PostgreSQL/SQLite 竞价锁内恢复完整 `AuctionRules`，普通/代理/慢速/最终阶段规划使用旧版 PPM 计算，公开 lot 列表/详情的下一口价也读取同一快照，代理禁用、重复进场次数、默认模式和 final phase 慢速限制已接入，取消也按同一拍卖快照读取截止规则。默认值已对齐旧版共享配置（起拍 1000、PPM 50000、软截止/延长 1 分钟、次数上限 3、final phase 1 分钟）；MoogleMail 起拍价默认继承 `[auction.economics]`，仍允许显式覆盖。剩余是与真实旧数据、外部服务及切换前行为的对照验证。 |
| `auction-application` / `auction-storage` / `auction-jobs` | recovery 退回后的 relist、refund、resolution 和完整 action 对账 | recovery 退回证据、Take、取消/恢复、COD 买家黑名单、guarded relist、buyer refund fulfillment 收口和状态审计已经接入；Worker 已对齐旧版 `case → return → action → relist → refund` 顺序，recovery action 也已补齐 `pending → prepared → enqueued` 两阶段 delivery identity、action fence 和可重放 activation。Fulfillment/Recovery 已从全局 5 秒 tick 拆为独立 `60s + [0,5s]` 循环，恢复 100 条批次、8 并发、60 秒 lease/重试和 30 秒单项时限；Recovery 各 phase 聚合失败但继续尝试后续 owner。prepare/activation 前按 Users owner 的 active 收件人投影复核资格，非 active 收件人持久化退避。旧版 recovery health 刷新也已接入：按 world 读取 Balance，持久化 low/stale/unavailable 状态并同步重算 recovery 计数。仍需更广泛的外部回执对照、真实部署对照和切换前验证。 |
| `auction-integrations` / `auction-application` / `auction-storage` / `auction-jobs` | MoogleMail main/isekai 独立轮询、读取/处理重试、超时和失败退避 | 每个已配置 world 现在拥有独立调度任务、持久化 cursor、claim 和手动唤醒器，一个邮箱的读取或数据库失败不会阻塞另一个邮箱，也不会终止 Worker。调度契约和默认值已对齐旧版：`schedule` 接受标准五字段 cron、描述符、`@every`、进程本地时区及 `TZ=` / `CRON_TZ=` IANA 时区前缀，默认 `* * * * *`，再附加最多五秒抖动；共享 HTTP transport 恢复一秒最小请求间隔、15 秒请求超时、固定生产/回环安全 endpoint 和 8 MiB 响应上限；批次为 100 封，读取与业务处理各三次、每次 30 秒，使用 `1s → 30s` 指数退避。读取只对暂时错误内联重试，业务错误沿用旧版默认临时分类，成功后按 cron 保存下一次 `next_poll_at`，失败后保存下一 cron 与退避时刻的较大值。长批次处理期间按 10 秒周期续租 60 秒 claim，token/owner/version 或续租 fence 失效后取消陈旧 future；业务重试只通过显式短生命周期副本携带正文和装备 key，所有副本销毁时清零，稳定命令 ID 负责恢复可能已提交的本地步骤。剩余是用真实 main/isekai 邮箱对照分页、限流、Cookie 失效和写入不确定结果。 |
| `auction-integrations` / `auction-jobs` | 论坛 `start`、`cancel`、PM `notify` 以及对应回执/不确定结果处理 | 经对照 Auction-BF 的 `forumingest`，`start`、`cancel` 和 PM-only `notify` 在旧版本身就是 warning-only，并未连接业务 owner；当前 Rust 保持该边界。MM `notify`、主题发布/读取和回帖竞价导入已接入；Forum 已从全局集成 tick 拆为独立、立即执行的顺序循环，默认一分钟加最多五秒抖动、100 条批次、300 秒逐主题 claim 和 93 秒单主题总时限；发布失败仍继续回帖扫描，任何一阶段失败都不会终止其它 Worker owner。回帖批次按 world 聚合，只有该批所有主题成功且命令账本完成写入后才推进扫描游标，已入账但通知暂未完成的命令会在后续轮次重放通知；主题正文现在可按 `worker.forum.public_base_url` 生成与旧版一致的公开拍卖绝对链接；发布超时按远端写入结果未知停靠，远端发布成功但本地发布证明写入失败时也进入人工对账退避状态，不再静默等待租约过期后重复发布。若要让这些 warning-only 命令真正改变论坛或拍卖状态，属于超出旧版等价范围的新契约，需另行冻结 owner、幂等账本和回执证明。 |
| `auction-application` / `auction-storage` / `auction-jobs` / `auction-http` / `auction-web-admin` | operations alert 信号、通知批次、Worker control 聚合和完整 dashboard section | 旧版 15 类 signal、阈值 evaluator、fingerprint、观察/恢复、批次 claim lease/fencing、三次 Create 尝试、MoogleMail 通知正文、入队原子 cooldown、稳定失败终态，以及 Admin 的 health/process、Worker、tracing、alerting、全量 mail poller、realtime lag/failure water、delivery、fulfillment、recovery 安全汇总已接入 PostgreSQL 完整档；`000015` 同时修复了普通重试写入会被 CHECK 拒绝、第三次失败误入人工对账和领取时过早消耗 cooldown 的差异。scan、dispatch、reconcile 已恢复为三个独立的一分钟有界循环，错误互不阻塞，阈值、批次窗口、冷却、重试和正文边界默认值与旧版一致。credential scope 聚合和无 active 凭据时的 degraded 启动语义也已接入，SQLite 明确为能力缺失。剩余是通知外部回执、真实部署和切换前验证；外部广播 sink 不属于旧版能力。 |
| `auction-observability` / `auction-process` / `auction-jobs` | Prometheus/OpenTelemetry metrics/tracing exporter、真实状态心跳与进程停机 | Public/Admin 已接入默认关闭、固定 `/metrics` 的进程本地 Prometheus registry、旧版同等的 CPU/内存/文件描述符/线程数/启动时间进程资源指标和安全身份指标；Worker control 的 peer-authenticated Unix socket 也已提供同一 `/metrics` 路径，并注册旧版 9 类 Worker loop 的结果、耗时、批次、重试和调度延迟指标；Public/Worker 主配置与 Admin runtime 均已接入 OTLP/HTTP tracing exporter，使用角色专属敏感 headers、自有有界批处理和队列丢弃计数；三类进程已登记 PostgreSQL/SQLite 连接池当前借出、空闲、总数、最大连接数及 SQLx 存储池边界的十项 acquisition 指标；固定低基数的业务事件结果/耗时指标也已覆盖 Public 竞价/代理命令和 Worker 邮件、拍卖生命周期、履约、recovery、operations-alert 循环。当前 27 个追加审计 INSERT 均会累加进程内饱和失败计数；三进程周期心跳每轮复用数据库/schema readiness，读取该计数和各自 tracing 安全投影，启动/停止状态也不再使用占位值。三进程现已统一处理 SIGINT/SIGTERM、10 秒业务 drain、二次信号强制收口、停止心跳及配置有界的 exporter flush；绕过存储池和审计观测边界的外部原生调用不纳入对应累计统计。剩余是实际部署下的 exporter、心跳 TTL 与故障聚合证据。 |
| `auction-http` / `auction-web-*` | 旧版全部 Public/Admin 页面真实数据和交互 | Public 核心目录、世界切换、当前/历史拍卖、竞价/代理、账户、规则、黑名单、补偿、论坛、实时与价格历史，以及 Admin 登录、拍卖/lot、邮件、用户、凭据、规则、配置、审计、delivery、realtime、settlement/recovery 与 dashboard 页面均已接入真实 API；URL 状态、权威快照恢复、稳定 `command_id` 重试、五语言资源、移动端表格和主要旧版视觉交互已有源码 owner。SPA 入口现只提供给 14 + 21 条显式浏览器路由，未知 API/health 与页面 404 已隔离；Public/Admin 读响应及静态文件缓存头也按旧版语义和 Dioxus 稳定 WASM 文件名完成收口。HTTP/stream DTO 已统一由 `auction-contracts` 拥有，两个 `RustEmbed` 类型也已移入各自装配根形成制品级隔离。剩余是用真实 Dioxus 浏览器产物逐页核对字体、换行、焦点与响应式布局。 |
| `auction-integrations` / `auction-storage` | 凭据失效后的自动标记、轮换和 dashboard 聚合 | 读侧 session 失效只轮换未尝试候选，写侧不盲重试但回写 `invalid`；PostgreSQL 幂等状态更新、MoogleMail/E-Hentai 接入和 Admin dashboard 聚合已完成，dashboard 也按旧版检查 MoogleMail `main`/`isekai` availability、scope 更新时间、Worker 重启状态和已配置 `dashboard_mailboxes` 的 poller 完整性。仍需真实外部服务对照和切换前行为验证。 |
| `auction-storage` / `auctionctl` / `auction-http` | 旧版 `/health/live`、`/health/ready` 探针与 `migrate-only` 对未知数据库的保护 | Public/Admin 两类兼容健康探针已补齐；`ready` 只读检查数据库、全部内嵌迁移和进程内审计写入完整性，持久化 service heartbeat 复用同一进程 readiness；`auctionctl migrate` 在没有 SQLx 迁移标记但检测到用户对象时失败关闭，不会把未知数据库初始化为 Auction 数据库。剩余是实际 PostgreSQL/SQLite 部署中的探针与编排证据。 |
| `apps/auctionctl` / `auction-storage` / `auction-integrations` | 旧版 Worker `-bootstrap-mail-credentials`、`-bootstrap-mail-world`、`-bootstrap-mail-label` 一次性登记邮件凭据 | 已补齐 `bootstrap-mail-credentials --world --label`：Cookie 只从标准输入读取，限制为 `main`/`isekai` 且要求 Worker 已启用对应 world，复用凭据数据密钥和 Admin 凭据服务写入 PostgreSQL；命令不回显 Cookie、密文或密钥。仍需在真实部署中按目标 secret 注入方式做一次切换前演练。 |
| `ops` / `apps/*` | 旧版 `ops/deploy` 的启动脚本、Caddy 反向代理、环境示例、数据库备份/恢复/保留资料 | 已补充当前三进程/TOML/健康探针的部署目录、启动顺序、安全权限、Caddy 反向代理模板、systemd 单角色与总控示例、`auctionctl deployment-check` 只读预检，以及 Rust `auctionctl deployment start/stop/restart/status` 总控：总控验证 unit 与 TOML 绑定，执行严格预检、地址检查和 forward-only migration，按 Worker → Admin → Public 逐角色等待应用 readiness，失败反序回滚本次启动的 unit，并用 flock 与 systemd InvocationID 发布私有启动回执。PostgreSQL retention、recovery preflight、backup/verify、restore/rebuild 和 canonical v28 接管证据链均已有 owner。部署模板继续由 systemd 发送 SIGTERM；Public/Admin 的共享 Hyper 连接服务会停止接收并排空现有连接，Worker 同时响应 SIGINT/SIGTERM，三进程执行 10 秒有界 drain、停止心跳和 tracing flush，单角色 unit 以 45 秒上限覆盖最长 exporter 关闭余量。旧版 retention 本身明确只做 inventory、禁止 mutation，完整 retention mutation 不是旧版漏迁。 |
| `auction-storage` / `apps/auctionctl` / `ops` | canonical Auction-BF PostgreSQL v28 的现有数据接管 | 专用 v28 backup/verify/adopt 链路已完成：只接受 PostgreSQL 18.x、干净 migration 28、精确 canonical 表集合、零其他 session 和可无损恢复的竞价投影，要求专用恢复点、逐字确认和不可覆盖 receipt，在事务 bridge 中建立 SQLx `1..=5`、`7` 等价基线后继续前向迁移；`000014` 统一生命周期约束并移除旧论坛隐式入队 trigger，`000015` 归一化 operations alert 重试/终态/cooldown，并保留合法的历史终态 delivery 孤儿。普通 `migrate`/`restore` 不会猜测处理。剩余是真实 v28 副本上的接管与恢复演练。 |
| 验收与切换 | 双跑、对照、双写、切换前行为验证 | 按项目约束暂不编写或运行测试；只有用户明确要求后，才进入 PostgreSQL 完整契约、SQLite Lite 边界和旧版对照验证。 |

SQLite Lite 的 PostgreSQL 专属 recovery、MoogleMail、论坛完整档、结算报告和管理完整写入不属于
“漏做后补齐”的任务，而是已声明的部署能力边界；不得用空结果或内存状态伪装成已实现。

### 尚未完成的旧版等价收口

截至 2026-08-24，旧版 HTTP 路径、浏览器路由、Worker 类别、recovery action 和运维命令名称没有
数量缺项；第二轮从实际消费链、进程边界和项目规范交叉审计后，确认仍有以下**非测试代码任务**。
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
  `competition.rs`，`routes/public/mod.rs` 只保留模块装配与稳定导出；路由路径、状态码和 DTO
  契约未改变。
- 体量复核仍保留少数后台装配、领域聚合和适配器文件作为后续结构优化候选；它们当前职责边界已能由
  owner 和端口说明，不能把“可继续拆分”误报成旧版功能缺口或测试任务。当前已完成拆分的 storage、Worker
  和 Web owner 不再列为未完成项。

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

- 本轮已用仓库锁定的 `dx` CLI 分别构建 Public/Admin Web 产物，两个 WASM、wasm-bindgen、入口
  HTML 和 surface/base CSS 均成功生成；这只证明编译和静态资源打包链路，不证明真实浏览器中的
  字体、换行、焦点顺序、空态或响应式布局，因此浏览器逐页验收仍保留。
- 使用真实 main/isekai 邮箱与 E-Hentai 论坛核对分页、限流、凭据失效、远端写入不确定结果和回执；
- 在 canonical PostgreSQL v28 副本上演练 backup、verify、adopt、restore 和 rebuild 安全边界；
- 在真实 systemd/Caddy 部署中核对启动顺序、readiness、失败回滚、Worker control 和凭据注入；
- 用实际 Dioxus 浏览器产物逐页核对字体、换行、焦点顺序、空态和响应式布局；
- 在用户明确恢复行为验证后，执行 PostgreSQL 完整档、SQLite Lite 边界、旧版双跑/对照和最终切换。

最后一项仍按本仓库约定暂缓；本轮没有新增或运行单元测试、集成测试、功能测试、端到端测试或
浏览器测试。

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
回执显式记录数据库是否已删除/重建。外部进程停止、真实部署演练和恢复点保管仍需部署证据。

本轮进一步补齐现有 Auction-BF 数据的显式接管边界：`auction-storage/database/legacy_v28` 在
只读预检中要求 PostgreSQL 18.x、唯一且 clean 的 migration 28、精确 canonical public 表集合、
零其他 session、有效 schema identity 和可无损恢复的 policy/lot/bid realtime 事实；事务 bridge
补齐 Rust 生命周期列，将无领先者的旧起始价投影转换为空当前价，从原始 bid event 恢复逐事实
截止时间，保留并升级 final-fast 代理证据，建立缺少的兼容表/视图和 lower 查询索引。`auctionctl`
新增专用 v28 custom-format 备份/离线验证与默认只读 adoption；实际执行要求逐字
`ADOPT LEGACY V28 <数据库名>`、二次 artifact 校验和不可覆盖 receipt，写入 SQLx `1..=5`、`7`
当前 checksum 基线后只运行剩余迁移。普通 migrate/restore 不接受旧版 manifest；完整手册位于
`ops/deploy/LEGACY_V28_ADOPTION.md`，真实旧库副本和恢复演练仍属于显式验收。

本轮继续对照旧版最终 schema，确认 Rust 压缩迁移曾遗漏拍卖身份不可变、前序顺序和
draft/intake target 一致性触发器，也确认早期 Rust `000003` 遗漏了既有 draft 的 target 回填。
追加 PostgreSQL `000014_lifecycle_database_invariants` 后，迁移会先锁定并拒绝歧义投影，为可唯一
映射的 draft 补齐 target，再以当前定义安装三类防线；同一迁移移除旧 v28 的论坛自动入队 trigger，
避免数据库与 Worker 同时成为论坛任务 owner。`retention-policy.v1.json` 的 schema fence 同步推进到
`15`，避免只读 retention inventory 因策略仍停在旧版本而无条件失败。SQLite Lite 不复制
PostgreSQL 延迟触发器。

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
旧版 30 秒单项预算，claim 失败还会让整个 Worker 退出。现在 Auction 已拥有独立立即首轮和
`60s + [0,5s]` cadence；生命周期与结算各自使用 100 条上限、共同并发波次和单项总时限，一类
错误仍继续尝试另一类。配置省略并发时按 PostgreSQL=8、SQLite Lite=1 计算有效值；邮件手工唤醒
不再顺带执行 Auction。

继续对照旧版 MoogleMail 写入协议后，确认 Rust return/take 曾只核对动作按钮，POST 后也只依赖
成功文字或按钮消失，无法证明实际处理的是预期邮件和资产。现在动作状态会脱敏保留
To/From/Subject、正文、装备 ID、Credits、COD 和附件计数，但不保留装备 key 或原始 HTML。
transport 在 POST 前校验 sender；take 还与 outbox 的预期资产做集合精确比对。return POST 后
要求收件箱表头/结果行结构合法且目标 `mid` 不再被任何链接或动作引用；take POST 后
要求邮件身份不变、资产/COD 归零、claimable 消失，并核对正文追加的唯一 `CoD Paid`/
`Removed attachment` 回执。前置或后置无法证明都进入人工对账，不会误报成功或盲目重发。

同一轮还确认 Rust 的最终 send 响应曾把旧版可证明的“收件人不存在”和“无法支付邮资”
统一收敛为 `write_unknown`，且 transport 失败端口无法保留退信证据；compose 初始页解析失败或残留资产
还曾被当作可确定失败。现在非干净 compose 统一以 `preflight_unproven` 人工核对，parser 只检查排除
script/style/noscript/textarea 后的可见 body 文字；收件人不存在保留唯一合法退信 `mid`、
`reason_code=recipient_not_found` 和 `take_required=true`，并在退信资产取回前进入人工对账；邮资拒绝在未暂存资产时是确定
失败，已暂存 Credits/装备/COD 时则是 `send_rejected_after_stage` 人工对账。delivery 失败端口与
PostgreSQL 写回同步扩展为可保留有界安全 metadata 和外部引用，storage 仍覆盖权威
`safe_detail` / `outcome_unknown`，Admin 只接收已有安全投影字段而不接收 metadata 原文。

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

本轮 Public 竞价日志还补齐了旧版的拍品文本/UUID 筛选、URL 状态、当前页筛选结果、真实 lot UUID
链接和 credit 金额格式；服务端已有的 `world`/`lot` 查询边界现在由浏览器 DTO 正确消费。

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

本轮继续对照 Admin Dashboard 展示层：指标卡与运行时明细恢复旧版扁平卡片、统一边框和连续卡片流，
不再使用状态色顶边或独立 raised 背景；health、Worker、realtime、delivery、fulfillment、recovery、
alerting 投影、轮询操作和快捷入口保持不变。

本轮继续对照 Public 页头连接提示：实时状态徽章恢复旧版圆点、raised 背景和统一边框，并为 checking、
connecting、connected、error 保留稳定状态色；实时订阅、游标恢复和手动刷新行为不变。

本轮继续对照 Public Catalog 窄屏表格：字段标签恢复旧版自然宽度，不固定为 `5rem`；`data-label`、参与者投影、
竞价动作和移动端字段语义保持不变。

本轮继续对照 Public 倒计时：HTTP 响应补齐标准 `Date` 头，浏览器以 500ms 中点样本和 0.3 EMA 估算
服务端时钟偏移，拍卖看板与 lot 详情统一使用修正后的当前时间；closing 提示恢复为剩余 15 分钟，
超过 24 小时按 `Xd HH:MM:SS` 展示，缺失/非法截止时间显示 `—`，过期状态使用五语言收尾文案；
这些都是展示层状态，不改变拍卖状态、竞价资格或服务端截止时间判断。

本轮继续对照 Public 拍卖页：预览/历史切换恢复旧版的轻量文字链接与分隔语义，拍卖标题恢复旧版
`--text-xl` 字号；历史轮次导航、竞价动作和 URL 状态保持现有契约不变。

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

本轮 Admin 拍卖总览恢复旧版在选择 `main` 或 `isekai` 时显示的手工装备录入表单；表单与 lot 草稿页
共用 `auction-web-admin/src/app/auction/intake.rs`，继续通过稳定 `command_id`、URL/canonical JSON、起拍价
和游戏身份走既有 A04 契约，避免两个页面的录入校验和幂等语义分叉。

本轮还补齐 Admin 改名历史页旧版已有的空结果提示与分页控件；页面直接消费既有改名列表投影的
`page`、`page_size` 和 `total`，不新增接口字段或改变回滚命令边界。

本轮 Users 列表也补齐筛选无结果时的明确空态；用户字段缺失继续使用 `—`，列表无记录则使用独立
的本地化提示，避免两种状态混淆。

本轮 Admin lot 草稿页也补齐旧版零 lot 空态；有记录时才渲染 lot 表格，分页和 intake 命令契约保持不变。

本轮又将 Admin 的拍卖、lot、代理历史、delivery、凭据、Users、改名、邮件入站、realtime、审计和
结算/recovery 列表统一接入旧版已有的直接跳页分页交互；页码上限仍由既有安全投影的
`page/page_size/total`（或已声明的页数兜底）决定，不新增接口字段、不在浏览器端缓存列表快照。

本轮继续补齐 Public 旧版共享 `Pager` 的直接跳页交互：拍卖目录、历史、竞价日志、价格历史、补偿池、
账户、lot 竞价历史均复用统一控件；已知总数使用 `total`，游标兼容响应只允许跳到当前可确认的下一页，
不改变 URL 状态或 API 契约。

本轮 Public 黑名单也恢复旧版的 URL 页码分页：请求使用 `page_size=10`，页码写入既有 `page` 查询状态，
页面继续只展示安全公开投影，不把黑名单原因或玩家标识扩展为新的写入能力。

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
论坛主题未发布时继续每两秒读取安全投影，直到状态进入 `published`；预览拍卖到达 `opens_at` 且实时
连接可用时触发一次权威当前拍卖刷新，并按拍卖 ID/开放时间隔离刷新状态。

本轮继续恢复 Admin 拍品列表与旧版的关键可见语义：列表首列使用按装备分类生成的稳定拍品编码，补回
提交编号的紧凑展示并保留完整值悬浮查看；软截止信息继续通过安全 DTO 中的授权编号提供旧版提示。
这只改变浏览器安全投影的展示，不新增接口字段、不把装备密钥或邮件凭据暴露给前端。

本轮还修正 Admin runtime 配置页的版本 fence 语义：候选校验返回的 fingerprint 只属于规范化候选，
不会覆盖当前 TOML 文件 fingerprint；保存时继续携带当前文件版本，避免校验修改后的有效配置被页面
自身错误地提交为过期版本。

本轮继续补齐 Public 黑名单与旧版的展示契约：恢复页面说明文本、表格移动端字段标签，并对玩家标识和
公开原因使用共享安全截断组件；完整值仍保留在原生提示中，不改变公开投影、分页或查询参数。

本轮同时补齐 Public 账户页与旧版的展示契约：恢复账户页和代理授权说明、移动端表格字段标签、领先竞价行
高亮及状态徽章；竞价、代理授权的现有数据投影、分页和操作语义保持不变。

本轮还修正 Public `/auctions` 的迁移残留文案：列表已经读取真实拍卖查询投影，页面改为说明世界/状态筛选，
不再显示“查询用例尚未接入”的过时提示，也不再复用历史价格页说明。

本轮继续补齐 Admin 手工装备录入与旧版表单的交互契约：改为原生提交事件，增加 URL、卖家、正整数价格
的前置校验和 canonical JSON 结构校验，补回必填、自动完成、提示文本与移动端可用的表单语义；成功提示
同时显示拍卖序号、lot 号和装备名称，输入错误与网络失败分开提示，五种语言资源保持同步。

Admin dashboard 继续恢复旧版运维信息的可见粒度：凭据投影按 world/scope 展示 active、disabled、invalid、
更新时间和重启要求；alerting 摘要补回 succeeded 计数及最近观察、成功、失败时间。两者仍只使用既有安全
dashboard DTO，不新增浏览器敏感字段或外部接口。

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

本轮继续补齐 Admin 用户创建弹窗：用户名、游戏身份和手动 key 字段恢复原生 form/required 语义，
回车可提交，空的手动 key 会在请求前拦截；生成 key 的一次性展示、命令幂等和成功后刷新行为不变。

本轮还恢复 Admin Users 用户名筛选的旧版提交边界：输入先保留在草稿，按回车提交后才更新 active
查询并回到第一页；状态闭集选择仍立即应用，避免每次键入都触发列表请求。

本轮对照旧版部署探针，补齐 Public/Admin 的 `/health/live` 与 `/health/ready`：前者只报告进程存活，
后者通过应用层 readiness 端口只读检查数据库连接及当前二进制的全部 SQLx 迁移，缺少数据库或迁移
未完成时返回 503；探针继续使用旧版 `{code,message,data}` 外壳，不计入 feature API 数量。

本轮补齐 Admin `/status` 页面：不再误复用 realtime 运维页面，而是分别读取 `/health/live` 与
`/health/ready`，将存活、就绪和暂不可用状态以本地化安全徽章展示；`ready` 的 503 被视为未就绪
状态，不改变探针 HTTP 契约或业务 dashboard 语义。

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
Public 五语言资源中已经没有调用、且仍声称“数据接口接入后替换示例数据”的迁移期死文案同时删除。

Lot 改价与撤拍交互继续对齐旧版：表头和动作使用旧版短文案，弹窗标题使用 `One01` 等拍品编码而非
裸序号，撤拍警告明确装备将以 COD 0 寄回卖家且操作不可撤销。改价改为原生表单提交，支持回车与
required/step 语义；本地非正整数错误与 API 更新失败分别展示，二者不再共用误导性的网络错误反馈。

Lot 行还补回旧版供辅助技术读取的“#序号 + 装备名”隐藏标题；装备截断提示改为把名称和完整属性 JSON
放进同一个 `title`，避免嵌套元素的 title 覆盖导致属性详情实际不可见。

改价和撤拍成功后现在恢复页面级 `role=status` 反馈；提交新动作会清除旧反馈，成功后再刷新权威 lot
列表，避免仅关闭弹窗却不给管理员完成确认。可选卖家和提交编号为空时也恢复旧版空单元格呈现。

拍卖生命周期弹窗继续按旧版补齐届次与动作语义：预览、开放、取消和 final phase 标题都显示当前
sequence，正文分别说明目录冻结、截止时间不变、托管装备退回和 final phase 不可撤销；取消原因同时
恢复“写入审计历史”的提示。弹窗改为原生表单提交，取消原因获得首焦点，其他动作首焦点仍在取消按钮；
surface 级焦点陷阱负责关闭后恢复触发控件。成功结果改回页面级 `role=status`，并按动作显示五语言文案，
失败仍留在动作行供重试；表格首列也补回旧版供辅助技术读取的场次隐藏标题。
