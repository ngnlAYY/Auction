# auction-storage

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

PostgreSQL 完整档和 SQLite Lite 的 SQL、连接池、查询、事务、锁及迁移实现。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [用户身份与消费 owner](#用户身份与消费-owner)
- [竞价事务和后端入口](#竞价事务和后端入口)
- [持久调度与慢速代理](#持久调度与慢速代理)
- [报表与恢复](#报表与恢复)
- [连接池可观测性](#连接池可观测性)
- [只读盘点与重建](#只读盘点与重建)
- [后端选择](#后端选择)
- [Cargo feature](#cargo-feature)
- [边界约束](#边界约束)
- [SQLite 连接选项](#sqlite-连接选项)
- [迁移](#迁移)
- [连续领先时间](#连续领先时间)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/database/store.rs](src/database/store.rs)、[pool](src/database/pool) | 后端分派、readiness 与连接池观测 |
| [src/database/competition](src/database/competition) | PostgreSQL 竞价、生命周期与管理事务 |
| [src/database/sqlite](src/database/sqlite) | Lite 查询与显式串行写事务 |
| [src/database/settlement](src/database/settlement)、[settlement_policy](src/database/settlement_policy) | 结算事实与冻结策略解码 |
| [src/database/scheduler](src/database/scheduler) | 持久化轮次、领取与结果门禁 |
| [src/database/realtime_consumer](src/database/realtime_consumer) | 独占消费、连续游标与失败恢复 |
| [src/database/migrations](src/database/migrations)、[initialization](src/database/initialization) | 追加迁移、兼容执行与首次空库初始化 |
| [src/database/legacy_v28](src/database/legacy_v28) | canonical v28 格式防腐与接管 |
| [src/deployment_identity](src/deployment_identity) | 脱敏数据库目标身份与严格部署校验 |

## 职责与实现边界

实现 `auction-application` 的窄端口；SQL 模型与行映射不得越过存储边界。
业务决定留在领域/应用层，外部请求留在集成层。表与能力归属统一见[对象契约](../../docs/contracts/schema.md)，
本页保留存储实现及后端差异。

## 用户身份与消费 owner

[database/users](src/database/users) 拥有 Users 写入与身份一致性；注册信封绑定的 UUID 必须在写锁内复核。
论坛数字账号是稳定身份，`username` 是可变显示名和 MM 收件人，两者不能互换。
[database/sessions](src/database/sessions) 在两后端保存会话摘要：Admin 启动撤销旧 Admin 会话，
Public key 撤销/重签与旧会话永久撤销同事务提交；只读 inspect 不刷新过期时间。

PostgreSQL 凭据与 realtime 消费 owner 持有专用物理 session；操作复用原持锁连接，失效后不得从池补连接继续。
guard 退出时关闭物理连接，不带锁归池。凭据网络调用不跨越数据库事务。

SQLite 文件库从实际 `main` 路径确认身份，以同目录 `0600` sidecar 非阻塞锁保护 owner；锁文件退出不删除。
文件须单链接、目录链受信、所在目录非 group/other 可写且可创建锁；Linux `statx` mount 身份证明拒绝单文件 bind mount，
无法证明时失败关闭。内存库按有效名称在进程内隔离/共享，特殊 URI、自定义 VFS 或 immutable 包装不提供独占保证。
部署身份只暴露端点/文件等安全目标，不包含连接原文、账号或密码。

## 竞价事务和后端入口

PostgreSQL 竞价按 world → auction → lot 加锁；只读 world/auction 使用共享行锁，
lot、活动代理、参与者资格和账本保留排他锁，生命周期使用对应排他锁。
SQLite 竞价、生命周期、身份、调度等写事务由 SQLx 守卫跟踪 `BEGIN IMMEDIATE`，错误或取消后回滚再归池。

取得锁后读取数据库时间，同一计划的代理、领先时间、截止、审计及接受事实共用该时刻。
命令账本、状态、审计和 realtime/outbox 在同一事务提交；Final 与轮次完成标记也原子写入。
两后端分别实现 SQL 和映射，不以无锁读取、降级隔离或 `AnyPool` 换取统一接口。

`competition/postgres/` 与 `sqlite/competition/transaction/` 内按 bidding、lifecycle 拆分；
PostgreSQL 管理写入另归 `competition/postgres/admin/`，命令装配归 [database/commands](src/database/commands)。
旧竞价账本只读重放时仍在一致快照中取得冻结策略、授权版本和转换历史，不重新执行当前状态门禁。

## 持久调度与慢速代理

[database/scheduler](src/database/scheduler) 保存 due、租约、尝试次数和失败状态；完成/失败必须匹配 owner、精确租约及版本。
Admin 改期清除旧租约并推进版本，陈旧任务不能收口。已存在轮次按冻结事实读取，配置变更不改写旧命令或时间。

[slow_proxy_due.rs](src/database/slow_proxy_due.rs) 只做有界候选扫描，在 LIMIT 前排除已到截止的 lot；
执行仍由竞价事务复核 generation、due、授权与资格。慢速代理使用 lot 上的持久化调度字段和确定性命令，
不另建内存游标或 Redis 锁。

## 报表与恢复

| 持久化职责 | 入口与边界 |
| --- | --- |
| 公开目录、lot、账户、竞价日志 | `auctions/`、`lots/`、`account/`、`bid_log/` 分别实现两后端安全投影 |
| 价格历史、补偿与结算报告 | `price_history/`、`compensation/`、`settlement_report/` 读取 PostgreSQL 已提交结算事实；Lite 明确不支持 |
| 结算 workflow 与履约 | `settlement_scheduler/`、`settlement_commands/`、`fulfillment_scheduler/` 保存 claim、计划、义务和执行证明 |
| 恢复 | `recovery_report/`、`recovery_commands/`、`recovery_scheduler/` 以证据、版本和租约保护确认、提醒、退款与重挂 |
| 邮件 | `mail_poll/`、`mail_intake/`、`mail_delivery/`、`admin_mail/` 分别拥有轮询、入站事实、outbox 与审查投影 |
| 外部投递 | `delivery_scheduler/`、`delivery_admin/` 拥有发送状态、fencing、人工动作和同事务审计 |
| 论坛、凭据与告警 | `forum/`、`credentials/`、`operations_alert/` 保存各自进度、安全投影与唯一所有权 |
| 规则、审计与健康 | `rules/`、`admin_audit/`、`health/`、`audit_health/` 维护版本、追加记录与 readiness 水位 |

编号页的 total 与当前页来自一致快照。禁用 world 的活动投影不能公开；已 settled 历史按契约允许读取。
Lite 没有结算事实链，不能把手工写入的 settled/sold 状态当作成交历史。

结算/恢复、外部邮件/论坛、凭据管理和告警均为完整档；SQLite 只实现已登记的子集，缺能力明确返回不支持。
Admin 读取不能为补全投影临时写入；邮件展示与诊断 sidecar 均为非权威数据，不参与幂等、完成证明或 cursor。

Delivery 从 claimed 进入 sending 前须证明所属命令已提交；未知发送结果进入 manual_reconcile，不自动重寄。
主动 Send 成功要求新增、唯一且完整匹配的 Sent 证明；流拍/撤拍逐件 Send 原装备，历史冻结 Return 不自动改写。
论坛未知 POST 先精确只读对账，再按持久人工授权重试；真实处理楼层独立于内容版本和 post ID。
详细写入与证明要求见[领域不变量](../../docs/contracts/invariants.md)及[邮件诊断](../../docs/operations/mail-diagnostics.md)。

## 连接池可观测性

`DatabaseStore::pool_metrics_source` 只暴露借出、空闲、总数、上限及真实 acquisition 累计指标。
普通查询、事务与显式 acquire 均经过 `TrackedPool`，不额外查询 SQL，也不暴露数据库名、路径或连接串。
外部原生 SQLx 池不计入这组累计数据。readiness 检查数据库、内嵌迁移及审计写入失败计数，异常失败关闭。

## 只读盘点与重建

[retention_inventory](src/database/retention_inventory) 在只读快照中按版本化政策盘点关系、依赖与 realtime 水位，
不授予归档/删除资格；[recovery_preflight](src/database/recovery_preflight) 只核对恢复目标，不运行 pg_restore 或覆盖对象。

[bootstrap](src/database/bootstrap) 在维护库身份、权限和集群锁下创建缺失数据库，不提供 DROP 或 migration；
[rebuild](src/database/rebuild) 是独立破坏性边界，要求精确目标与受限目录条件，保留受支持的数据库属性。
备份、人工确认、停止进程、外部工具与 receipt 归 `auctionctl`，不能由本 crate 推断授权。
操作步骤见[数据库恢复与保留](../../ops/deploy/README.md#数据库恢复与保留)。

## 后端选择

配置层将 `[database]` 或兼容 `database_url` 转为 URL，本 crate 不解析 TOML。

| scheme | 档位 |
| --- | --- |
| `postgres://` / `postgresql://` | 完整档，后端锁与并发事务 |
| `sqlite:` | Lite，单主机、每角色最多一个实例、Worker 并发一 |

完整能力矩阵见[架构](../../docs/architecture.md#部署档位与能力)。

## Cargo feature

默认不编译数据库驱动；`postgres` 启用 SQLx PostgreSQL/rustls，`sqlite` 启用 SQLite。
发布只选实际后端；配置选择未编译的驱动时明确失败。

## 边界约束

竞价及关键生命周期使用 PostgreSQL Serializable、最多三次尝试（含首次）与明确行锁；
队列领取等路径才使用 Read Committed 加锁。只能重试已分类的序列化冲突/死锁，未知提交结果不能盲目重放。
SQLite 使用串行写事务，已实现能力同样遵守幂等、原子性与追加审计，不因 Lite 降低正确性。

## SQLite 连接选项

文件库显式开启 WAL、外键、缺失时建文件、5 秒 busy timeout，池上限为 5；写入仍只有一个。
内存库统一使用单连接且不启用 WAL，避免私有内存库的多连接隔离问题；命名共享内存的 owner 规则另行校验。
SQLite 文件必须位于本地受信文件系统，不能通过网络共享扩展；备份同时遵守 WAL 和单节点约束。

## 迁移

[PostgreSQL](../../migrations/postgres/README.md) 与 [SQLite](../../migrations/sqlite/README.md) 独立维护只追加迁移，
共有对象同名同义，不要求序号或能力对等。普通启动不隐式迁移；CLI 停机门禁、迁移失败与升级步骤见[迁移导航](../../migrations/README.md)。

`initialization/` 只供明确授权的合并首次空 PostgreSQL 库初始化：同一专用连接持 SQLx 迁移锁，拒绝已有用户对象与其他客户端，
不建库、不删对象、不升级已有库。与普通迁移共用 `000005`/`000015` 的受限历史别名执行视图，不重复加锁或改写原文件/checksum。
canonical v28 必须走[专用接管](../../ops/deploy/LEGACY_V28_ADOPTION.md)，不能冒充普通追加迁移。

## 连续领先时间

`leader_since` 按本次全部接受事实判断连续领先是否中断，普通出价后被原领先者快速代理反超也会重新计时。
PostgreSQL `000037` 与 SQLite `000012` 只重建可由追加式 bid 证明的未结算 live lot 投影；无法证明不猜测，
已结算金额、终态 lot 和历史审计不改写。冻结结算策略解码不应用新模板门禁，后继创建另由应用/领域层校验。

## 维护与验证

```bash
cargo check --locked -p auction-storage --no-default-features --features postgres
cargo check --locked -p auction-storage --no-default-features --features sqlite
```

仅静态编译，不执行迁移或行为测试。完整检查见[开发指南](../../docs/development.md)，
源码能力与运行证据见[验证状态](../../docs/status.md)。新增持久化能力先更新架构档位与对象 owner。
