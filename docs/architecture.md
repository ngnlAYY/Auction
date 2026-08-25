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
| `auction-application` | Create/Open/PlaceBid/Proxy/Settle/Rules/runtime-config/credentials 等用例、事务端口、幂等策略 | 具体数据库和路由 |
| `auction-contracts` | REST/WebSocket DTO、错误编码、分页和版本契约 | 领域写入和 SQL 模型 |
| `auction-storage` | PostgreSQL 完整适配器、SQLite Lite 适配器、migration、query、repository | 竞价规则本身 |
| `auction-cache` | 默认关闭的缓存边界、Redis 连接和带 TTL 的可重建值 | 权威状态、锁、幂等和审计 |
| `auction-integrations` | E-Hentai、MoogleMail、凭据加密、身份和通知适配器 | 拍卖状态迁移 |
| `auction-jobs` | due task、论坛扫描、投递重试、生命周期调度，以及独立的本机 Worker control 传输边界 | Public/Admin 业务 HTTP 与领域决策 |
| `auction-http` | Public/Admin router、认证 middleware、DTO 映射、前端产物内嵌、共享 TCP/Unix HTTP listener 安全绑定 | 领域决策与进程级监听选择 |
| `auction-web-shared` | 设计变量、基础排版、语言集合与偏好持久化、无业务偏向的展示组件 | 任一 surface 的业务组件与文案 |
| `auction-web-public` | 公开端 Dioxus 组件、路由、公开端译文 | 数据库、secret、管理端能力 |
| `auction-web-admin` | 管理端 Dioxus 组件、路由、管理端译文 | 数据库、secret、公开端组件树 |
| `auction-config` | 主/敏感 TOML 解析、默认值、校验、runtime TOML 原子文件适配器 | 业务规则与命令账本 |
| `auction-observability` | tracing、metrics、health、correlation id、心跳安全投影；Public/Admin 与 Worker control 的可选 `/metrics` scrape、进程资源指标和三类进程的有界 OTLP/HTTP exporter | 业务日志内容 |
| `auction-process` | SIGINT/SIGTERM 订阅、10 秒默认 drain、二次信号分类和 abort-on-drop Tokio 任务守卫 | 配置、listener、心跳发布和业务停止顺序 |

浏览器端是纯 CSR SPA。`dx build --platform web` 为每个 surface 生成静态 WASM/JS/HTML。
Public/Admin 装配根各自定义唯一的 `RustEmbed` 类型，`auction-http` 只接收泛型资源类型并实现
通用响应，因此任一部署制品都不会引用另一端资源，也不依赖链接器裁剪保证隔离。产物缺失时
静态路由返回 503 并说明需要执行的构建命令，不影响 `/health` 契约，也不让进程崩溃。

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
                       ├─ auction-cache ─ Redis（可选）
                        └─ auction-process ─ signals/drain/task ownership
auctionctl ──────────────────────────────── ops/migrations/config
```

`auction-jobs/src/control/` 是 Worker control 的独立子模块 owner，不属于 `src/worker/` 调度
实现：它拥有 UDS-only HTTP/JSON 协议、peer credential allowlist、实例锁、socket 清理和 Admin
client；`src/worker/` 只依赖状态句柄与唤醒端口。该 control 面不接受业务写入，不能复用 Public/Admin
HTTP listener 或 socket。

Public 和 Admin 可以分别绑定端口、Unix socket 或反向代理；Public 的 listener 位于主配置，
Admin 的主配置 listener 是 fallback，存在 runtime 文件时由 runtime listener 覆盖；Worker 不暴露业务写入
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

宿主机部署控制由 `auctionctl deployment` 与 systemd 分工：systemd 是单角色 PID、信号、
`TimeoutStopSec`、自动重启和 journald 的 owner；Rust 总控只拥有跨角色的严格 TOML/unit 绑定
预检、监听地址检查、forward-only migration、Worker → Admin → Public readiness 顺序、失败反序
回滚和 Public → Admin → Worker 停止顺序。控制操作通过私有状态目录中的 `flock` 串行化，成功
启动后以 systemd `InvocationID`、MainPID 和单调启动时间发布无敏感信息的原子回执。Worker 必须
启用 Unix control readiness；migration 一旦提交不会因后续角色启动失败而自动 down/restore。
`auction-stack.service` 可以在开机时进入同一边界，三个角色 unit 不应再被分别 enable。

三进程通过 `auction-process` 共用操作系统生命周期原语，但具体资源顺序仍由各装配根拥有。
Unix 下显式订阅 SIGINT 与 SIGTERM；首次信号让 Public/Admin 通过共享 Hyper 连接服务停止接收
新连接，让 Worker control 进入 draining 并通知各循环不再领取新工作。当前 HTTP 请求或 Worker
有界批次最多排空 10 秒，期间第二次终止信号或超时会触发
abort-on-drop 强制收口。后台周期心跳先停止，再发布 `stopping`；最后独立按
`tracing.shutdown_timeout_seconds`（最多 30 秒）刷新 exporter。部署 unit 的停止上限因此至少要覆盖
10 秒业务排空、最长 30 秒 exporter 刷新和少量进程回收余量，仓库示例固定为 45 秒。

`auctionctl retention-inventory` 属于 PostgreSQL 完整档的只读运维盘点能力，不属于
SQLite Lite。它把 `ops/deploy/retention-policy.v1.json` 编译进制品作为版本化政策矩阵，在
单个 `REPEATABLE READ READ ONLY` 事务中核对 `current_database()`、schema identity、SQLx
migration 状态，并读取 `public` 关系的估算行数、存储体积、政策表覆盖、数据族汇总、realtime
诊断水位和外键依赖；结果只能以不可覆盖的私有证据文件保存。realtime consumer 最小 offset
不是权威归档水位，外键依赖在政策明确前视为资格阻断因素。该盘点不执行迁移、备份、恢复、
归档、删除或任何业务写入，`eligibility_assessed` 与 `eligibility_authorized` 必须保持为
`false`，不能把盘点结果误报为旧版 retention 策略或生产恢复授权。

`auctionctl recovery-preflight` 是 PostgreSQL 完整档的另一项只读运维能力。它在同一
`REPEATABLE READ READ ONLY` 事务中检查目标数据库的 `current_database()`、主节点可写状态、
其他连接、用户对象数量、Auction schema identity 和 migration 历史，并以不可覆盖的私有证据
保存 `restore_preflight_passed`。该字段只表示空库、无其他连接和可写主节点等基础条件，不表示
backup archive、manifest 或恢复内容已经验证；命令不得停止连接、执行 `pg_restore`、创建或删除
数据库。

`auctionctl postgres-backup` 与 `postgres-backup-verify` 是 PostgreSQL 完整档的初始备份边界：
前者要求源库身份和 SQLx migration clean/current，在源库不写入的前提下调用部署提供的
`pg_dump --format=custom`，随后调用 `pg_restore --list` 做 archive 校验，并以不可覆盖的私有
manifest 保存数据库身份摘要、schema 版本和 archive SHA-256；数据库 URL 中的密码只通过短生命
周期的 0600 `.pgpass` 传给外部工具。后者只做本地 archive/manifest 配对、哈希和目录校验，不连接
数据库。

`auctionctl postgres-restore` 是 PostgreSQL 完整档的单事务恢复边界。它要求已验证的配对
archive/manifest、精确目标数据库确认、可写主节点、零外部连接和零用户对象；只读预检后先关闭
工具自身 SQLx 连接池，再次校验 artifact 未变化，才调用
`pg_restore --exit-on-error --single-transaction --no-owner --no-privileges`。提交后必须证明 Auction
schema identity 和 manifest 中的 clean migration 版本一致，才能原子发布不可覆盖的私有 receipt。
该命令不会停止外部进程、终止连接、创建/删除数据库、清理 schema、destructive rebuild 或执行
retention mutation；空库准备、维护窗口和重新放量仍属于部署 owner。

`auctionctl postgres-rebuild` 与 `auction-storage::database::rebuild` 共同拥有 PostgreSQL 18 完整档
的独立破坏性重建边界。CLI 默认只读，并要求当前 clean Auction schema、精确库名、零外部连接、
verified backup 或明确的一次性环境豁免；实际 mutation 还要求 `--execute`、逐字二次确认和预先
可写的不可覆盖 receipt。存储适配器只在 `postgres` 维护库持有集群 advisory lock，拒绝维护库、
自定义 database ACL/role setting、prepared transaction、subscription 和活动 logical slot，再以
经过 quoting 审计的 DDL 保留 owner、UTF8、locale provider、tablespace 和 connection limit。
`empty_for_restore` 后置条件是零用户对象，`fresh_schema` 后置条件是当前 clean migration；失败回执
显式区分数据库是否已经删除/重建。该命令不会由启动、重启、普通迁移或 systemd unit 调用，也不
替代部署 owner 停止进程、阻止自动重连和保管异地恢复点的职责。

## 缓存边界

缓存默认关闭。需要 Redis 时，对目标进程启用 `cache-redis` Cargo feature，在主配置中把
`cache.backend` 设为 `redis`，并仅在敏感 TOML 中提供 `redis_url`。实现直接复用 Redis
客户端的异步连接管理，不额外引入通用连接池或进程内缓存库。

缓存只能保存可重建的查询结果或派生视图，并必须带有限 TTL。当前首个消费者是 Public 规则
读模型：按语言缓存通过应用校验的 JSON 快照，`cache.public_rules_ttl_seconds` 默认 30 秒；
Admin 规则发布成功后按语言删除对应 key。读取 miss、快照损坏、Redis 不可用和失效失败都
回退到 PostgreSQL/SQLite 规则表，失效失败由 TTL 收敛，不能回滚已提交规则事务。竞价事务、
资格判断、命令幂等、锁、会话真值和审计不能依赖缓存。由此，未编译 Redis feature、运行时
选择 `disabled` 或 Redis 临时故障都不改变业务正确性。

## 配置与敏感信息

每个主配置只包含 `[listener]` 监听策略、日志过滤器、缓存策略和拍卖策略模板等普通字段，并通过 `secrets_file` 引用
第二个 TOML。Admin 主配置还可以通过 `runtime_config_file` 引用独立的非敏感 runtime TOML；
它承载监听、安全策略、会话时限、Worker control 路径和可选 dashboard world/mailbox 投影等可审计运行参数，不承载数据库 URL、
Redis URL、会话签名密钥、Admin actor/secret 或外部服务 Cookie。敏感值只存在于敏感文件；
不兼容旧的单文件字段，也不提供环境变量覆盖。相对路径以主配置所在目录解析，部署时敏感
文件和 runtime 文件都应置于受信任目录并限制运行账号读取，runtime 更新使用原子替换。

Admin A26/A27 的 runtime 更新属于 PostgreSQL 完整档：`auction-application` 拥有严格文档
模型、fingerprint fence、恢复服务和命令端口，`auction-config/src/runtime/` 拥有受权限
约束的 TOML 文件锁与原子替换，`auction-storage` 拥有
`runtime_config_admin_commands`、审计外键和 pending/applied/unknown 账本。更新后只写文件
和 durable intent，不在当前进程内热改 listener/security/session；成功响应必须带
`restart_required`。Admin 对外就绪前会执行同一恢复服务；可证明的 pending/applied 状态自动
收口，`unknown` 或文件冲突会保留人工核对告警，其他恢复错误阻断启动。SQLite Lite 读取
启动时 runtime 文档但不提供这项完整更新能力，HTTP 返回 `501 feature_not_supported`。Public/Admin
监听器装配已支持 TCP 与 Linux Unix socket：Unix 模式在绑定前校验受信任 runtime 目录、目录级
单实例锁、stale socket 身份、owner/group 和 `0660` 权限；当前进程退出时只清理自己创建且身份
未变化的 socket。

三进程部署预检会把独立 TOML 重新组合成跨进程拓扑：Admin 与 Worker 必须指向同一 Worker
control socket，并使用相同的装备、用户登录和外部凭据数据密钥标识/材料；Public/Admin 的实际
TCP 或 Unix listener 不能冲突。`auctionctl deployment` 的 Admin readiness 直接跟随 runtime
选择的 TCP/Unix listener，不再探测被 runtime 覆盖掉的主配置地址。该比较只给出字段类别，
不会输出密钥材料。

拍卖策略模板位于主 TOML 的 `[auction]` 区块。Admin 创建拍卖时，应用服务会把已校验模板
交给 PostgreSQL 完整档事务；事务将同一来源写入 schema v4 `policy_snapshot` 以及遗留规则列，
避免管理端配置与实际规则分叉。竞价和取消事务会在锁内从该快照恢复 PPM、代理限制、final
phase 及时间规则；MoogleMail 入站默认起拍价继承同一经济模板。

### Lite 能力边界

| 能力 | PostgreSQL 完整档 | SQLite Lite 档 |
| --- | --- | --- |
| 已实现命令的数据正确性 | 完整领域不变量 | 同一不变量，不允许弱化 |
| schema 与 migration | 完整生产 schema，追加式迁移 | 仅功能子集，追加式迁移，可接受维护窗口 |
| 写入并发 | Serializable + 明确行锁；队列路径可用 Read Committed + 显式锁 | WAL 下多读并发、写入串行、低并发 |
| 进程拓扑 | 独立进程与多副本 | 单主机、每个角色一个实例 |
| Worker | 持久化游标、可扩展并发 | 若启用则游标仍持久化，但实例和并发均为一 |
| 高可用与扩展 | 可建设故障转移、只读副本和横向扩展 | 不支持多副本、故障转移或网络共享数据库 |

当前仓库仍处于分阶段迁移，这张表定义的是实现上限和验收边界，不表示所有旧版业务功能已经完成。
以后新增持久化能力时，必须先标明它属于“完整档”“Lite 档”或“两者”，再决定是否需要
SQLite migration 和 repository 实现。

当前已落地的实现切片包括：纯领域 `CompetitionPlan`、代理转换与结算值对象，应用层
`TransactionPolicy`、`CommandLedger`、竞价/结算事务端口、`place_bid` 与
`settle_auction` 用例，以及 PostgreSQL `000001` 至 `000015` schema migration。
PostgreSQL 竞价、生命周期和结算适配器已把命令账本、锁定的权威状态、事实、状态版本、审计、履约
初始状态和 realtime 事件放进对应事务，并对关键 Serializable 冲突最多重试三次；旧名
`bid_proxies` 仅由兼容视图投影。`000014` 在 PostgreSQL 完整档恢复拍卖身份不可变、前序
顺序和 draft/intake target 一致性的数据库级防线；`000015` 恢复 operations alert 的
signal identity、三次 Create 重试、lease/fencing、终态错误码及成功入队后 cooldown 防线；
operations alert 的 scan、dispatch 和 reconcile 也已从共享 Worker tick 拆为三个独立一分钟循环，
分别使用有界 deadline、错误隔离和旧版默认阈值/窗口/冷却/重试策略；
MoogleMail 每个 world 的独立 poller 已恢复旧版标准五字段 cron，支持描述符、`@every`、进程
本地时区和 `TZ=` / `CRON_TZ=` IANA 时区前缀；成功游标和失败退避都从同一调度计算下一时刻，
共享 HTTP transport 同时恢复旧版一秒最小请求间隔、15 秒请求超时和可配置安全 endpoint；
PostgreSQL fulfillment/recovery 同样恢复独立的 `60s + [0,5s]` cadence、100 条批次、8 并发、
60 秒 lease/持久化重试和 30 秒单项总时限，Recovery 保持五阶段顺序且各 owner 错误相互隔离；
Auction 生命周期/结算主循环也恢复该一分钟 cadence、100 条分类型批次、PostgreSQL 8 并发和
30 秒单项总时限，SQLite Lite 的有效并发由配置层固定为 1；
SQLite `000001` 至 `000007`、迁移入口和
`BEGIN IMMEDIATE` 竞价写适配器已接入，Lite Worker 并发也由配置层固定为 1。HTTP 已注册
Public 20 条与 Admin 37 条契约路径，并完成稳定响应、世界/拍卖/lot 只读查询端口和
PostgreSQL/SQLite 数据库适配；配置数据库后 `/api/v1/worlds`、`/api/v1/rules`、`/api/v1/blacklist`、`/api/v1/auctions`、
`/api/v1/auctions/current`、Public lot 列表/详情、竞价日志、价格历史和普通竞价命令返回权威目录或当前公开
projection，
拍卖列表保留页码与 keyset cursor 语义，lot 列表保留 preview/live 互斥投影、分类统计和旧分页；已结算拍卖仍可在公开安全投影内读取 lot，供历史轮次页面使用，
形状，竞价日志保留 world/lot 筛选、完整降序 keyset 边界和 cursor 页未知 total 语义，价格历史
保留旧筛选、成交条目与编号页统计；Public 补偿池与单场账本在 PostgreSQL 完整档使用结算
账本的同一 Serializable 快照、守恒校验和 keyset 游标，SQLite Lite 对该完整档能力明确返回不支持。
Admin A04 的 URL 装备来源会在启动时冻结 runtime 的 host/path allowlist、响应上限、共享请求间隔和
全请求超时；该业务策略叠加在固定 HTTPS origin、无重定向、精确 world 路径、响应类型和流式有界
读取之上，关闭 allowlist 也不会放宽这些 transport 防线。

PostgreSQL 使用结算账本边界，SQLite Lite 使用 settled/sold 公开状态边界。Admin A03 拍卖列表 GET 和 A06 lot 列表在 PostgreSQL 完整档分别从同一 Serializable 快照读取生命周期/lot 数量与拍卖/lot/装备/投稿来源安全投影；A15 代理历史从同一快照读取命令账本中的代理计划并映射为安全事件，保留页码与 opaque cursor 两种分页；三者均保留 Admin 会话边界，SQLite Lite 因缺少完整 Admin 生命周期字段明确返回不支持。A03 创建拍卖 POST 已接入 PostgreSQL 完整档的命令账本、world/intake target 锁、策略快照、审计和创建事件；SQLite Lite 明确返回不支持。A04 装备录入已接入 `AdminIntakePort`、固定 HTTPS 装备来源/规范 JSON 适配器、AES-256-GCM key 信封、PostgreSQL Serializable world/target/玩家/装备锁，以及装备、密文、投稿、lot、版本、审计和 realtime 事件的同一事务；`/api/v1/admin/worlds/{world_id}/auction-intake/items` 与草稿 lot 管理页已接入，SQLite Lite 因没有 `equipment_secrets` 和完整录入事务明确返回 `501 feature_not_supported`。Public session、Admin session、账户读取、普通竞价以及代理创建/降低命令的
command ledger/竞价事务也已接入配置、应用端口与会话/竞价表；Admin session 已覆盖 Basic
登录交换、双 Cookie、CSRF 和持久化轮换/刷新/撤销；账户读取返回代理历史/配额/最近 bid 和 capability，代理命令返回旧版
`{ authorization: ... }` 完整命令投影，quota、转换历史和终止原因从提交后的安全账户快照补齐，授权状态版本不一致时失败关闭；
代理重入的 `usage_number` 在 lot 锁内按用户历史最大值递增，避免重入与命令重放混淆，
不暴露原始命令账本 payload。Admin 生命周期的 preview/open/cancel/final-phase 已通过统一应用端口接入，
命令请求携带 `expected_state_version`，取消动作还必须携带原因；存储事务在锁内校验版本并写入取消时间、原因和操作者，随后与命令账本、拍卖状态版本、追加审计记录和已注册 lifecycle realtime 事件在同一数据库事务内提交；Admin A25 规则 GET/POST/PUT 已接入规则 owner，PostgreSQL 使用独立 Serializable revision CAS、`rules_admin_commands` 幂等账本和追加审计，SQLite 只提供规则读取；A05 论坛预览已接入本地 HTML/文本解析端口与管理端契约，限制单次输入为 2 MiB，并只返回 catalog、posts、warnings 等安全预览数据；它不发起远程论坛请求、不写数据库，也不把本地解析结果误报成远程发布成功。A33 Admin 审计已接入两套数据库的追加式安全只读投影、严格精确筛选、keyset/数字分页和 metadata JSON object 校验；数字页的 total 与记录在同一读取事务内取得，管理端不接触原始 SQL 行。Public
`allow_manual_early_open` 在 Admin 启动时冻结为应用策略，HTTP capability 使用同一来源；计划时间
之前的人工开放会在存储事务锁定拍卖并读取 `scheduled_open_at` 后失败关闭，计划时间已到的开放和
Worker due 命令不受影响。命令账本重放先于当前策略检查，已经提交的结果不会因重启后配置变化而
失去幂等性。

`/api/v1/events` 已从 PostgreSQL/SQLite 持久化事件表提供带 sequence 游标的 Catch-up，`/api/v1/ws`
只提供有界的 Public session 服务端推送，恢复仍以 Catch-up/权威快照为准；
Admin A07 结算报告已接入 PostgreSQL 完整档的一致只读快照，SQLite 对该完整档能力明确返回不支持；
结算后的履约 claim/fencing、`pending → prepared → enqueued` 状态推进和 delivery outbox
的持久化领取/发送中 fencing 已由 Worker 阶段接入；Admin delivery 列表、release、retry 和
reconcile 也已接入 PostgreSQL 安全投影、fencing version 与独立命令账本。具体 MoogleMail
适配器已在 `auction-integrations/src/clients/mooglemail/` 完成固定 URL/响应边界、分页
cursor、allowlist-first 详情读取、正文与附件资产严格解析、装备 key 双来源交叉校验以及
Bot/测试 Cookie 切换；写入协议层又补齐 compose/detail typed target、表单闭集、单次 POST、
`write_unknown` 不确定性分类和发送成功页证明。return/take 另外在动作前冻结邮件身份与
资产摘要：sender 必须与 delivery 收件人一致，take 的装备 ID、Credits、COD 和附件计数必须与
outbox 预期值精确一致。POST 后 return 只接受结构正确且目标 `mid` 完全消失的收件箱，take
只接受同一邮件身份、资产归零且正文追加唯一 Credits/COD/附件回执的页面；任一证据缺失都进入
`manual_reconcile`。send 在任何写入前还必须证明 compose 是无资产/正文残留的干净状态，否则以
`preflight_unproven` 人工核对；远端明确拒绝时，只从排除 script/style/noscript/textarea 后的可见文字
识别“收件人不存在”和“无法支付邮资”；前者保留可选退信 `mid` 和 `take_required` 证据，
并在退信资产取回前进入人工对账；后者只在从未暂存资产时归为确定失败，否则进入人工对账。PostgreSQL
Delivery Worker 在 `claimed → sending` 前还会按 payload world 查询 scoped/shared 凭据可用性；
无凭据时以 lease/fencing 释放回 `pending`。进入 `sending` 后的凭据竞态、网络和协议依赖失败必须
进入人工对账，只有纯本地配置/负载或明确协议拒绝可以成为普通失败。Delivery 使用独立的
poll/jitter/batch/concurrency、lease/renew、完整请求和 finalize timeout 配置；每轮完成后按
`poll + [0, jitter]` 再唤醒，单轮持久化错误只记录并延后重试，不终止其它 Worker owner。Worker 只在并发槽空闲后
领取下一条，多步远端写按配置周期续租。续租失效后只允许使用原 token/fencing 尝试写入结果未知；
若 fence 已失效，数据库会在 lease 与额外 sending grace 均到期后隔离为人工对账。
Delivery 的创建/准备/激活、Worker claim/发送/释放/终态、过期 sending 隔离和 Admin 人工动作
都与状态变化在同一 PostgreSQL 事务中追加审计；两阶段激活会持久化独立 activation command
及请求摘要，后续幂等重放必须匹配该身份，不能把另一条激活命令误认为成功。
`000008_mooglemail_polling` 与 `auction-storage/src/database/mail_poll/` 负责持久化状态、
lease/fencing、失败退避以及 `inbound_mail`/`mail_commands`/cursor 的同事务幂等提交。
Worker 在 `[worker.mooglemail]` 显式启用后会按顶层 world 与 `additional_worlds` 为每个已启用世界装配
独立的持久化邮箱状态、claim、读取 gateway、业务 processor、拍卖入站事务、
delivery outbox 和 `MoogleMailDeliveryTransport`。每个 world 同时拥有独立调度任务和手动唤醒器，
读取与单封业务处理分别执行有界超时重试；批次处理期间周期续租 claim，续租 fence 失效后取消
陈旧 future，不能推进 cursor。当前首条业务链已覆盖拍卖邮件的 Credits/COD
拒绝、无附件通知、装备来源校验、起拍价指令解析、拍卖草稿/lot 入站、take/return 动作和静态
通知发送；通知模板由 `[worker.mooglemail.mail_templates]` 提供旧版 14 键的结构化覆盖，启动
时校验占位符并冻结快照；用户注册/重绑已经通过 Users owner 的 PostgreSQL Serializable 事务接入，结果通知
只保存安全结果，注册 key 以独立数据密钥加密，outbox 只保存用户 UUID 引用，发送前才解密。
POST 后无法证明的结果进入 `manual_reconcile`，不会盲目重试。结算 intent 与 MoogleMail
send/return transport 已接入：邮件来源的未售/撤回执行原邮件退回，后台来源执行装备交接通知，
主世界/异世界付款及退款通知使用数据库快照中的收件人和装备外部 ID。结算 send 的 delivery
成功不会直接满足履约；Sent 分页/exact ID 查找、详情剩余资产矩阵、Balance 页面一致性校验
和 Worker 自动对账已经接入，满足时追加不可变 proof，发送成功页没有 exact remote ID 则进入
`manual_reconcile`。论坛主题同步和回帖竞价导入已由 PostgreSQL Worker 接入：主题发布采用
`auction_forum_sync` 的 claim/fencing 与 `write_unknown` 人工对账，回帖必须经过登记身份、lot
映射和 `auction_forum_bid_commands` 幂等账本，再调用现有竞价端口。Forum 在独立顺序循环中先
发布后读取回帖，两阶段错误分别记录，发布失败不会跳过回帖扫描，也不会终止其它 Worker owner；
主题只在即将处理时逐条领取，避免待执行队列提前消耗租约；SQLite Lite 明确不提供该
完整档。Recovery health 只读取已配置世界的 Balance；未启用世界的结算履约会安全降级为人工对账。

现有 Auction-BF PostgreSQL 数据不会由普通 `migrate` 自动识别或猜测升级。旧库接管由
`auction-storage::database::legacy_v28` 独占：只接受 PostgreSQL 18.x 上 migration 28、dirty=false、
精确 canonical 表集合、零其他 session 且竞价投影可无损恢复的源库。`auctionctl` 先创建并离线验证
专用 custom-format 备份，再经逐字确认执行 Serializable bridge；bridge 将等价覆盖的 SQLx
`1..=5` 与 `7` 写入当前 checksum 基线，随后只执行剩余前向迁移。普通 restore manifest 与旧版
manifest 分离，接管成功回执只在 schema identity 和全部内嵌迁移均通过后发布。
接管后的前向 `000014` 会用当前定义重建旧版生命周期约束触发器，并移除旧版只在 live
状态隐式入队的论坛触发器；`000015` 会归一化旧版 operations alert 批次并约束迁移后的
delivery 引用，合法的历史终态孤儿不会被伪造为 delivery。论坛任务发现、领取和 fencing
继续只由 Worker 持有。
Public 进程已补齐持久化 stream relay 和进程内有界 Hub：relay 读取已提交事件、在本地 sink
接收后推进 `(consumer_key, stream_key)` 游标，Hub 只负责当前进程的 WebSocket 分发并淘汰慢
连接。该实现恢复了旧版 Public 的低延迟本地广播边界；旧版没有外部广播 sink，未来若新增
跨进程 sink 必须另行冻结协议，不能把它登记为旧版缺口。
Admin A08 recovery 报告已接入 PostgreSQL 完整档的一致安全投影，SQLite 明确返回不支持；A14 recovery 确认已接入 PostgreSQL 完整档的 Serializable 版本 fence、证据重校验、命令账本、退款激活、guarded relist、审计和预览目录事件；结算事务已为每个获胜 lot 在同一事务中生成 `settlement_recoveries`，SQLite 明确返回不支持。Recovery Worker 已接入 recovery case 的 due 推进、付款超时提醒 action、持久化 claim/fencing、MoogleMail outbox 关联和 delivery 结果证明，并完成严格退回邮件证据绑定、evidence mismatch 阻断、装备领取/载具取消/装备恢复 Take、目标世界和资产摘要 fence、Users owner 幂等买家黑名单 proof；recovery action delivery 也已使用 `pending → prepared → enqueued` 两阶段 identity 和可重放 activation，并在 prepare/activation 前复核 Users owner 的 active 收件人投影；以及 `relist_pending → refunding/completed → completed` 的持久化 resolution 调度；recovery health 按配置的启用世界独立周期读取 Balance，保留最后成功观测并在同一 PostgreSQL 更新中重算逾期、重挂、失败和人工复核计数。旧版五类 action 的静态闭集已经对齐，剩余是外部回执行为验证；现有 501 只表达 SQLite Lite 或未装配依赖的能力边界。Worker 已接入 `scheduled_rounds` 的持久化 due claim、
租约、尝试次数、生命周期 preview/open 调用和指数退避失败记录；PostgreSQL 完整档还接入结算
workflow 的持久化领取、稳定结算命令、完成 fence 和失败退避，SQLite Lite 明确不启用该
workflow；结算履约还接入了 delivery/prepare/activation 身份和 outbox 的成功、失败、
人工对账终态边界；实时消费 runner 已接入 PostgreSQL/SQLite 的持久化游标、失败记录和
显式 replay 边界；Admin A21-A24 已接入 lag/failure 安全投影和事务内 replay、命令账本与审计，
外部广播 sink 属于尚未冻结的新增扩展；Public 本地 relay/Hub 已接入同一持久化事件边界。公开端首页看板与 lot 详情已经接入同源 Public
API；公开端看板收到已注册拍卖/竞价事件后重新读取权威快照，Admin 拍卖列表提供带 CSRF、命令 ID
和结果反馈的生命周期操作；公开端规则、黑名单、补偿池/账本、论坛镜像、健康和实时 catch-up 页面已消费对应安全 DTO；论坛远程发布/回执仍由 Worker 外部适配器负责，MoogleMail 入站的首条
拍卖/用户注册重绑业务链、结算 delivery transport 和 Sent/Balance 回执对账已接入；论坛远程
同步与回帖导入已由 Worker 接入；MM `notify` 已进入 MoogleMail delivery outbox。对照
Auction-BF 的 `forumingest`，论坛 `start/cancel` 和 PM-only 通知在旧版本身就是 warning-only，
没有连接业务 owner，当前 Rust 保持该兼容边界；若未来要让它们真正改变状态，需要另行冻结
owner、幂等账本和回执契约。Admin 结算页面已消费该安全 settlement/recovery 契约，提供已结算拍卖
选择、lot 金额表、资金池摘要和带版本 fence 的 recovery 确认；Admin outbox 页面复用已冻结的
realtime failure/lag/replay 安全投影，不等同于外部广播 sink。SQLite 仍明确返回不支持。
Public/Admin 的统一 HTTP boundary 已补齐请求 ID/命令审计关联、W3C trace context、访问日志、
panic recovery、安全响应头、普通请求超时、5 秒 HTTP/1 请求头读取超时、可信代理地址解析和
4096 项有界限流器。静态分派也只向 14 + 21 条显式浏览器路由提供 SPA 入口，未知 API/health
返回 JSON 404，未知浏览器导航返回页面 404，并按旧版恢复 Public/Admin/静态资源缓存策略。
Web DTO 单一 owner、首批缓存读模型和静态资源制品级隔离，以及次级部署/所有权边界均已完成。
当前除真实浏览器、外部回执和部署切换证据外，不再把这些项目列为代码缺口；在真实行为证据
完成前仍不能宣称完整等价。Admin runtime 的人工提前开放与装备来源策略、三进程 SIGINT/SIGTERM、
有界排空和真实状态心跳也已进入实际运行 owner。

Admin A02 dashboard 已从两套数据库读取 `service_heartbeats` 的 health/process 安全投影，
并在 Admin session 保护下按 owner 返回安全 section；只有 owner 未装配或读取失败时才显式返回
`unknown`。Public、Admin、Worker 装配根的周期心跳每轮复用数据库/schema readiness，读取当前
进程的审计写入失败饱和计数，并接入各自 tracing owner 的安全投影；数据库、schema 或审计异常
会令 readiness 失败，tracing degraded 会令 heartbeat status 降级。SIGINT/SIGTERM 下会停止周期
任务并发布携带当前审计/tracing 状态的 `stopping`。Worker control 已通过
安全 runtime 目录、实例锁、固定 group/0660、inode 清理和显式 UID/GID peer credential allowlist
提供 live/ready/status、同 registry 的 `/metrics` 和
MoogleMail 唤醒，Admin dashboard 只显示安全投影；启用并配置 `dashboard_mailboxes` 时还按 Admin
runtime 中的 world/mailbox 投影校验轮询器数量和邮箱标识，避免已配置世界缺少 poller 仍显示正常；
无数据库启动骨架启用 control 时也只报告进程存活和零个 poller，不伪造持久化就绪；operations
alert、credential 按 kind/world 的 active/disabled/invalid 聚合，以及 scope 更新时间、最近 Admin
变更时间和 Worker 重启状态；凭据 section 还按旧版要求分别验证 MoogleMail `main`/`isekai`
world 的 active、invalid 和更新时间，还有 mail、realtime、
delivery、fulfillment、recovery 的安全汇总也已接入 dashboard，其中 mail 为全量 poller、
realtime 为 lag/failure water、delivery 为 pending/failed/manual-reconcile 水位及年龄字段；
外部广播状态不属于旧版 dashboard 契约；未来若新增跨进程 sink，必须用独立 section 表达，
Public Hub 的当前连接水位不能替代其健康指标。

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
`501 feature_not_supported`。A20 `POST /api/v1/admin/mail/polls:trigger` 通过 Worker control
使用单个 `command_id` 全局唤醒已装配的 MoogleMail poller，返回 `202 accepted` 及接受的
poller 数量；手动唤醒只让本轮跳过下一次调度时间，实际远程 GET 仍由 Worker 的持久化 lease、
失败退避和入站幂等提交负责，不会把唤醒动作伪装成远程读取成功。

Admin A28/A30/A31 用户管理属于“两者”能力：`auction-application` 的 `admin_users` 端口拥有
用户投影、创建/改名/游戏身份修改、key 撤销/重签、黑名单和改名回滚命令；
`auction-storage/src/database/users/admin/` 负责版本 fence、命令账本、密文 key 信封；其中
`mod.rs` 负责跨后端分派与共享校验，`postgres.rs`/`sqlite.rs` 分别拥有对应 SQL 实现。
改名/黑名单追加历史与审计。PostgreSQL 使用 Serializable + advisory command lock，SQLite
使用 `BEGIN IMMEDIATE`；两者都不保存 key 明文。`auction-http` 接入 A28/A30/A31 的 session、
CSRF 和稳定 DTO，`auction-web-admin` 的 `/users` 页面已接入列表、创建、改名、游戏身份修改、
撤销/重签 key、黑名单和改名回滚操作。A29 改名历史读取继续由 `admin_renames` 端口拥有；MoogleMail 注册/换绑本身仍
只在 PostgreSQL 完整档提供，不能据此宣称两套数据库能力完全等价。

Admin A32 凭据管理属于 PostgreSQL 完整档：`auction-application` 的
`admin_credentials` 端口负责凭据种类、world/shared scope、secret 校验、命令摘要和版本
冲突；`auction-integrations/src/credentials/` 使用独立 `credential_data_key` 生成带类型域的
AES-256-GCM 信封。`auction-storage/src/database/credentials/` 在同一 Serializable 事务内
完成安全投影、命令账本、审计以及导入/改名/启停/删除，删除后仍保留命令与审计证据；凭据 dashboard
同时按 scope 返回更新时间，并以最近 Admin 变更与 Worker 启动心跳判断重启要求；Worker
通过 active 凭据读取端口按 world 取池并在内存解密；没有 active 行时保留 degraded Worker，
实际操作返回 credential unavailable；已有 active 行但密钥不匹配或密文损坏时失败关闭。远端
确认 session 失效时按稳定顺序轮换、幂等回写 `invalid` 并从当前池移除，写入开始后禁止盲重试。Admin/Web 只接触安全投影，secret
不进入响应或页面；SQLite Lite 明确返回能力不支持，`.keyenv` 只作为 Worker 兼容回退。

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
