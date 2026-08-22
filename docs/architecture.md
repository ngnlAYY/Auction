# Rust 重构架构

## 总体原则

Rust 版本从零开始，不把 Go 的包名机械翻译成 Rust module。边界按“谁拥有业务事实”
划分，依赖只向内：

```text
apps (composition roots)
  ├── HTTP / Worker / CLI adapters
  └── infrastructure adapters
          ↓
      application (use cases + ports)
          ↓
       domain (invariants, state machines, value objects)
```

`auction-domain` 不依赖 Axum、Dioxus、SQLx、reqwest 或环境变量。`auction-application`
只声明端口；数据库、论坛、邮件和 HTTP 都在外层实现端口。

“全 Rust”是仓库代码和程序库依赖约束，不是基础设施采购约束。PostgreSQL、SQLite、
Redis、反向代理和系统服务可使用非 Rust 官方实现，并通过适配器边界接入。

## Crate 所有权

| crate | 所有内容 | 明确不负责 |
| --- | --- | --- |
| `auction-domain` | Auction/Lot/Bid/Proxy/Settlement 模型、状态机、不变量 | HTTP、SQL、日志、外部网络 |
| `auction-application` | Create/Open/PlaceBid/Proxy/Settle/Rules/runtime-config 等用例、事务端口、幂等策略 | 具体数据库和路由 |
| `auction-contracts` | REST/WebSocket DTO、错误编码、分页和版本契约 | 领域写入和 SQL 模型 |
| `auction-storage` | PostgreSQL 完整适配器、SQLite Lite 适配器、migration、query、repository | 竞价规则本身 |
| `auction-cache` | 默认关闭的缓存边界、Redis 连接和带 TTL 的可重建值 | 权威状态、锁、幂等和审计 |
| `auction-integrations` | E-Hentai、MoogleMail、身份和通知适配器 | 拍卖状态迁移 |
| `auction-jobs` | due task、论坛扫描、投递重试、生命周期调度 | HTTP handler |
| `auction-http` | Public/Admin router、认证 middleware、DTO 映射、前端产物内嵌 | 领域决策 |
| `auction-web-shared` | 设计变量、基础排版、语言集合与偏好持久化、无业务偏向的展示组件 | 任一 surface 的业务组件与文案 |
| `auction-web-public` | 公开端 Dioxus 组件、路由、公开端译文 | 数据库、secret、管理端能力 |
| `auction-web-admin` | 管理端 Dioxus 组件、路由、管理端译文 | 数据库、secret、公开端组件树 |
| `auction-config` | 主/敏感 TOML 解析、默认值、校验、runtime TOML 原子文件适配器 | 业务规则与命令账本 |
| `auction-observability` | tracing、metrics、health、correlation id、心跳安全投影 | 业务日志内容 |

浏览器端是纯 CSR SPA。`dx build --platform web` 为每个 surface 生成静态
WASM/JS/HTML，`auction-http` 用 `rust-embed` 分别内嵌，产出单文件制品。Public 与 Admin
的组件树、路由树和静态产物完全隔离：公开端二进制不包含管理端资源。产物缺失时静态路由
返回 503 并说明需要执行的构建命令，不影响 `/health` 契约，也不让进程崩溃。

界面文案由 `dioxus-i18n` 从各 surface 的 `i18n/*.ftl` 取用，支持 `en`/`zh-CN`/`zh-TW`/
`ja`/`ko`。公开端默认英文，管理端默认简体中文。译文资源按 surface 隔离，语言选择能力
（受支持集合、`hv.lang` 持久化、`<html lang>` 同步）由 `auction-web-shared` 统一提供。

## 运行进程

```text
auction-public-api  ─┐
auction-admin-api   ─┼─ auction-application
auction-worker      ─┘  ├─ auction-storage ─┬─ PostgreSQL（完整生产档）
                       │                   └─ SQLite（Lite 档）
                       ├─ auction-integrations ─ E-Hentai/MoogleMail
                       ├─ auction-jobs
                       └─ auction-cache ─ Redis（可选）
auctionctl ──────────────────────────────── ops/migrations/config
```

Public 和 Admin 可以分别绑定端口、Unix socket 或反向代理；Worker 不暴露业务写入
HTTP；所有写入仍必须经过 application owner 的命令端口。这样保留 Auction-BF 的部署
边界，同时把 apps/*/src/main.rs 与各 crate 的 src/lib.rs 压缩为薄组合根；实际实现按
职责放入带明确 owner 的 src 子目录，禁止在 src 根目录平铺实现文件。

### 源码目录边界

每个应用和 crate 的 src 根目录只保留必要的 main.rs 或 lib.rs 入口。入口以外的源码必须
放在表达职责的子目录中，例如 domain、ports、services、storage、http、web 或 bootstrap。
一个职责即一个 owner；即使只有一个文件，也建立有意义的子目录，避免通过 common 或 misc
目录隐藏依赖方向。

## 部署档位

| 档位 | 数据库与拓扑 | 能力边界 |
| --- | --- | --- |
| 完整生产档 | PostgreSQL；Public、Admin、Worker 可独立部署并扩展 | 完整持久化能力、并发写入、多进程和后续高可用能力 |
| Lite 档 | SQLite；单主机、每个角色至多一个实例、Worker 并发为一 | 本地、演示和低并发部署；不承诺多副本、故障转移、在线迁移或全部生产功能 |

`database_url` 的 scheme 决定档位，不额外引入会与数据库选择冲突的 profile 开关。Lite
档可以缺少 PostgreSQL 专属或高并发系统能力，但每项缺失都必须在对应模块和部署文档中
明确；已经声明支持的竞价命令仍必须遵守领域不变量、事务原子性、幂等和追加式审计。
SQLite 数据库文件只能位于单主机受支持的本地文件系统，不能把网络共享文件系统当作
横向扩展方案。

## 缓存边界

缓存默认关闭。需要 Redis 时，对目标进程启用 `cache-redis` Cargo feature，在主配置中把
`cache.backend` 设为 `redis`，并仅在敏感 TOML 中提供 `redis_url`。实现直接复用 Redis
客户端的异步连接管理，不额外引入通用连接池或进程内缓存库。

缓存只能保存可重建的查询结果或派生视图，并必须带有限 TTL。读取 miss、Redis 不可用和
失效失败都回退到权威数据源；竞价事务、资格判断、命令幂等、锁、会话真值和审计不能依赖
缓存。由此，未编译 Redis feature、运行时选择 `disabled` 或 Redis 临时故障都不改变业务
正确性。

## 配置与敏感信息

每个主配置只包含监听地址、日志过滤器、缓存策略等普通字段，并通过 `secrets_file` 引用
第二个 TOML。Admin 主配置还可以通过 `runtime_config_file` 引用独立的非敏感 runtime TOML；
它承载监听、安全策略、会话时限和 Worker control 路径等可审计运行参数，不承载数据库 URL、
Redis URL、会话签名密钥、Admin actor/secret 或外部服务 Cookie。敏感值只存在于敏感文件；
不兼容旧的单文件字段，也不提供环境变量覆盖。相对路径以主配置所在目录解析，部署时敏感
文件和 runtime 文件都应置于受信任目录并限制运行账号读取，runtime 更新使用原子替换。

Admin A26/A27 的 runtime 更新属于 PostgreSQL 完整档：`auction-application` 拥有严格文档
模型、fingerprint fence、恢复服务和命令端口，`auction-config/src/runtime/` 拥有受权限
约束的 TOML 文件锁与原子替换，`auction-storage` 拥有
`runtime_config_admin_commands`、审计外键和 pending/applied/unknown 账本。更新后只写文件
和 durable intent，不在当前进程内热改 listener/security/session；成功响应必须带
`restart_required`。SQLite Lite 读取启动时 runtime 文档但不提供这项完整更新能力，HTTP 返回
`501 feature_not_supported`。

### Lite 能力边界

| 能力 | PostgreSQL 完整档 | SQLite Lite 档 |
| --- | --- | --- |
| 已实现命令的数据正确性 | 完整领域不变量 | 同一不变量，不允许弱化 |
| schema 与 migration | 完整生产 schema，追加式迁移 | 仅功能子集，追加式迁移，可接受维护窗口 |
| 写入并发 | Serializable + 明确行锁；队列路径可用 Read Committed + 显式锁 | WAL 下多读并发、写入串行、低并发 |
| 进程拓扑 | 独立进程与多副本 | 单主机、每个角色一个实例 |
| Worker | 持久化游标、可扩展并发 | 若启用则游标仍持久化，但实例和并发均为一 |
| 高可用与扩展 | 可建设故障转移、只读副本和横向扩展 | 不支持多副本、故障转移或网络共享数据库 |

当前仓库仍是启动骨架，这张表定义的是实现上限和验收边界，不表示对应业务功能已经完成。
以后新增持久化能力时，必须先标明它属于“完整档”“Lite 档”或“两者”，再决定是否需要
SQLite migration 和 repository 实现。

当前已落地的实现切片包括：纯领域 `CompetitionPlan`、代理转换与结算值对象，应用层
`TransactionPolicy`、`CommandLedger`、竞价/结算事务端口、`place_bid` 与
`settle_auction` 用例，以及 PostgreSQL `000001` 至 `000007` schema migration。
PostgreSQL 竞价、生命周期和结算适配器已把命令账本、锁定的权威状态、事实、状态版本、审计、履约
初始状态和 realtime 事件放进对应事务，并对关键 Serializable 冲突最多重试三次；旧名
`bid_proxies` 仅由兼容视图投影。SQLite `000001_lite_core.up.sql`、迁移入口和
`BEGIN IMMEDIATE` 竞价写适配器已接入，Lite Worker 并发也由配置层固定为 1。HTTP 已注册
Public 20 条与 Admin 37 条契约路径，并完成稳定响应、世界/拍卖/lot 只读查询端口和
PostgreSQL/SQLite 数据库适配；配置数据库后 `/api/v1/worlds`、`/api/v1/rules`、`/api/v1/blacklist`、`/api/v1/auctions`、
`/api/v1/auctions/current`、Public lot 列表/详情、竞价日志、价格历史和普通竞价命令返回权威目录或当前公开
projection，
拍卖列表保留页码与 keyset cursor 语义，lot 列表保留 preview/live 互斥投影、分类统计和旧分页
形状，竞价日志保留 world/lot 筛选、完整降序 keyset 边界和 cursor 页未知 total 语义，价格历史
保留旧筛选、成交条目与编号页统计；Public 补偿池与单场账本在 PostgreSQL 完整档使用结算
账本的同一 Serializable 快照、守恒校验和 keyset 游标，SQLite Lite 对该完整档能力明确返回不支持。
PostgreSQL 使用结算账本边界，SQLite Lite 使用 settled/sold 公开状态边界。Admin A03 拍卖列表 GET 和 A06 lot 列表在 PostgreSQL 完整档分别从同一 Serializable 快照读取生命周期/lot 数量与拍卖/lot/装备/投稿来源安全投影；A15 代理历史从同一快照读取命令账本中的代理计划并映射为安全事件，保留页码与 opaque cursor 两种分页；三者均保留 Admin 会话边界，SQLite Lite 因缺少完整 Admin 生命周期字段明确返回不支持。A03 创建拍卖 POST 已接入 PostgreSQL 完整档的命令账本、world/intake target 锁、策略快照、审计和创建事件；SQLite Lite 明确返回不支持。A04 装备录入已接入 `AdminIntakePort`、固定 HTTPS 装备来源/规范 JSON 适配器、AES-256-GCM key 信封、PostgreSQL Serializable world/target/玩家/装备锁，以及装备、密文、投稿、lot、版本、审计和 realtime 事件的同一事务；`/api/v1/admin/worlds/{world_id}/auction-intake/items` 与草稿 lot 管理页已接入，SQLite Lite 因没有 `equipment_secrets` 和完整录入事务明确返回 `501 feature_not_supported`。Public session、Admin session、账户读取、普通竞价以及代理创建/降低命令的
command ledger/竞价事务也已接入配置、应用端口与会话/竞价表；Admin session 已覆盖 Basic
登录交换、双 Cookie、CSRF 和持久化轮换/刷新/撤销；账户读取返回代理历史/配额/最近 bid 和 capability，代理命令返回旧版
`{ authorization: ... }` 完整命令投影，quota、转换历史和终止原因从提交后的安全账户快照补齐，授权状态版本不一致时失败关闭；
代理重入的 `usage_number` 在 lot 锁内按用户历史最大值递增，避免重入与命令重放混淆，
不暴露原始命令账本 payload。Admin 生命周期的 preview/open/cancel/final-phase 已通过统一应用端口接入，
命令账本、拍卖状态版本、追加审计记录和已注册 lifecycle realtime 事件在同一数据库事务内提交；Admin A25 规则 GET/POST/PUT 已接入规则 owner，PostgreSQL 使用独立 Serializable revision CAS、`rules_admin_commands` 幂等账本和追加审计，SQLite 只提供规则读取；A05 论坛预览已接入本地 HTML/文本解析端口与管理端契约，限制单次输入为 2 MiB，并只返回 catalog、posts、warnings 等安全预览数据；它不发起远程论坛请求、不写数据库，也不把本地解析结果误报成远程发布成功。A33 Admin 审计已接入两套数据库的追加式安全只读投影、严格精确筛选、keyset/数字分页和 metadata JSON object 校验；数字页的 total 与记录在同一读取事务内取得，管理端不接触原始 SQL 行。Public
`/api/v1/events` 已从 PostgreSQL/SQLite 持久化事件表提供带 sequence 游标的 Catch-up，`/api/v1/ws`
只提供有界的 Public session 服务端推送，恢复仍以 Catch-up/权威快照为准；
Admin A07 结算报告已接入 PostgreSQL 完整档的一致只读快照，SQLite 对该完整档能力明确返回不支持；
结算后的履约 claim/fencing、`pending → prepared → enqueued` 状态推进和 delivery outbox
的持久化领取/发送中 fencing 已由 Worker 阶段接入；Admin delivery 列表、release、retry 和
reconcile 也已接入 PostgreSQL 安全投影、fencing version 与独立命令账本。具体 MoogleMail
适配器已在 `auction-integrations/src/clients/mooglemail/` 完成读取首片：它持有固定 URL/响应
边界、分页 cursor、allowlist-first 详情读取、正文与附件资产严格解析、装备 key 双来源交叉
校验以及 Bot/测试 Cookie 切换；写入协议层又补齐 compose/detail typed target、表单闭集、
单次 POST、write_unknown 不确定性分类和发送成功页/附件状态证明；`MoogleMailSender` 负责把
这些步骤编排成逐步可证明的高层发送流程，但只接受已解析的远端装备 ID。PostgreSQL
`000008_mooglemail_polling` 与
`auction-storage/src/database/mail_poll/` 负责持久化状态、lease/fencing、失败退避以及
`inbound_mail`/`mail_commands`/cursor 的同事务幂等提交；Worker 在 `[worker.mooglemail]` 显式
启用后装配该读取适配器。当前只持久化 `parsed_pending_processing` 安全入站事实，尚未接入邮件
驱动的拍卖/交付/注册/重绑业务处理。出站 DeliveryTransport 仍等待消息模板、外部装备 ID/key
解析 owner 和回执/对账边界，当前不能把协议层完成误报为 delivery 已发送。
Admin A08 recovery 报告已接入 PostgreSQL 完整档的一致安全投影，SQLite 明确返回不支持；A14 recovery 确认已接入 PostgreSQL 完整档的 Serializable 版本 fence、证据重校验、命令账本、退款激活、guarded relist、审计和预览目录事件，SQLite 明确返回不支持；其余未迁移 handler 也明确返回 501。Worker 已接入 `scheduled_rounds` 的持久化 due claim、
租约、尝试次数、生命周期 preview/open 调用和指数退避失败记录；PostgreSQL 完整档还接入结算
workflow 的持久化领取、稳定结算命令、完成 fence 和失败退避，SQLite Lite 明确不启用该
workflow；结算履约还接入了 delivery/prepare/activation 身份和 outbox 的成功、失败、
人工对账终态边界；实时消费 runner 已接入 PostgreSQL/SQLite 的持久化游标、失败记录和
显式 replay 边界；Admin A21-A24 已接入 lag/failure 安全投影和事务内 replay、命令账本与审计，
但外部广播 sink 尚未冻结。公开端首页看板与 lot 详情已经接入同源 Public
API；公开端看板收到已注册拍卖/竞价事件后重新读取权威快照，Admin 拍卖列表提供带 CSRF、命令 ID
和结果反馈的生命周期操作；公开端规则、黑名单、补偿池/账本、论坛镜像、健康和实时 catch-up 页面已消费对应安全 DTO；论坛远程发布/回执仍由 Worker 外部适配器负责，MoogleMail 入站轮询已接入 Worker，但业务处理、出站传输、回执对账和其余 Web 数据接入仍在后续阶段，不能宣称
功能等价。

Admin A02 dashboard 已从两套数据库读取 `service_heartbeats` 的 health/process 安全投影，
并在 Admin session 保护下返回尚未迁移 owner 的显式 `unknown` section；Public、Admin、Worker
装配根已发布启动/就绪/周期/停止心跳，Worker control、外部广播和其余聚合仍未装配，不能把
该首片视为完整 dashboard 行为。

Admin draft lot 的 `PATCH /api/v1/admin/lots/{lot_id}` 属于 PostgreSQL 完整档能力：
`auction-application` 持有动作闭集、版本 fence 和命令端口，`auction-storage` 持有
world → auction → lot 锁顺序、lot 更新、预览 revision、审计、realtime 事件和
`withdrawn_return` 履约义务写入；`auction-http` 与 `auction-web-admin` 只负责 DTO、会话/CSRF
和页面交互。SQLite Lite 不暴露该完整管理写入，明确返回能力缺失。

Admin A16-A19 邮件状态与入站审查同样属于 PostgreSQL 完整档：
`auction-application` 的 `admin_mail` 端口/服务负责安全投影、状态版本 fence 和命令幂等，
`auction-storage/src/database/admin_mail/` 负责 `mail_poll_state`、`inbound_mail`、
`mail_admin_commands` 的查询和 Serializable 状态变更，`auction-http` 负责 Admin session、
CSRF、严格筛选和稳定 DTO，`auction-web-admin` 只负责页面交互。响应不包含原始邮件正文、
Cookie、外部凭据、游标或未审核 metadata；SQLite Lite 没有对应完整表/外部能力，明确返回
`501 feature_not_supported`。A20 `POST /api/v1/admin/mail/polls:trigger` 已由
`auction-application`/`auction-storage` 接入命令账本和状态版本 fence：它只幂等地推进下一次
轮询时间并返回 `202`，实际远程 GET 仍由 Worker 的持久化 lease、失败退避和入站幂等提交负责，
不会把排队动作伪装成远程读取成功。

Admin A29 改名历史读取属于“两者”能力：`auction-application` 的 `admin_renames` 端口拥有
筛选、分页和安全事实校验，`auction-storage/src/database/admin_renames/` 负责 PostgreSQL/
SQLite 的同事务读取，`auction-http` 映射 Admin session 和稳定 DTO，`auction-web-admin` 只
展示追加式历史。A30 回滚仍未迁移；它必须先补齐用户状态版本 fence、命令账本和用户名占用
冲突边界，不能由页面直接改写 `users`。

## 竞价写入不变量

第一版实现必须以以下顺序落地，不能先做页面再补一致性。PostgreSQL 实现完整用例和
repository port；SQLite 只实现 Lite 能力需要的端口，但已实现端口的事务细节和不变量
不能降级：

1. 为每个命令生成稳定 `command_id`，同一命令重放返回同一结果。
2. 在一个数据库 transaction 中锁定 auction/lot 和活动代理授权；PostgreSQL 的竞价和
   关键生命周期写入使用 Serializable、最多 3 次序列化冲突重试并配合明确行锁，队列
   领取路径才可使用 Read Committed 加显式锁；SQLite 使用明确的串行写事务，不能假设
   存在 `FOR UPDATE`。
3. 检查竞价资格、状态、金额、代理上限和有效结束时间。
4. 写入 bid fact、状态版本、audit event、outbox/event record。
5. 事务提交后由 Worker 投递通知；外部失败不能回滚已经接受的竞价事实。

Admin draft lot 修改还必须满足：同一 `command_id` 重放返回同一 lot 投影；过期的 lot
`state_version` 或非 draft/pending 状态失败关闭；lot 改动与 auction `preview_revision`、
auction `state_version`、审计和 realtime 事件同事务提交；withdrawn 后的装备退回只能通过
追加的 `settlement_obligations`/fulfillment 事实进入 Worker，不能在 HTTP 事务内直接调用外部
论坛或邮件服务。

PostgreSQL 是完整生产后端。SQLite 只是非对等 Lite 替代品，适用于本地开发、演示和
低并发单节点部署；必须在 runbook 中说明单写者、锁等待、备份和文件系统假设。

## 参考 Honk 的部分

Honk 使用根 Cargo workspace 管理多个独立 crate，并把设计文档、CI 和运维文件作为
一等目录；本项目采用相同的工程形态，但按 Auction 的 domain/application/adapters
边界重新命名。参考：<https://github.com/daeuniverse/honk>。
