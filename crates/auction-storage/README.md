# auction-storage

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

拥有 PostgreSQL 的 SQL、连接池、查询、事务、锁与迁移，实现应用窄端口。业务决定留在领域/应用层，
外部请求留在集成层；SQL 模型与行映射不越过存储边界。表与能力归属见[对象契约](../../docs/contracts/schema.md)。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [database/store.rs](src/database/store.rs)、[pool](src/database/pool) | 连接、readiness 与连接池观测 |
| [competition](src/database/competition)、[commands](src/database/commands) | 竞价、生命周期、管理事务与命令装配 |
| [settlement](src/database/settlement)、[settlement_policy](src/database/settlement_policy) | 结算事实与冻结策略解码 |
| [scheduler](src/database/scheduler)、[slow_proxy_due.rs](src/database/slow_proxy_due.rs) | 持久轮次、领取与慢速代理候选 |
| [realtime_consumer](src/database/realtime_consumer) | 独占消费、连续游标与失败恢复 |
| [migrations](src/database/migrations)、[initialization](src/database/initialization) | 追加迁移、兼容执行与首次空库初始化 |
| [legacy_v28](src/database/legacy_v28) | canonical v28 格式防腐与接管 |
| [deployment_identity](src/deployment_identity) | 脱敏数据库目标身份与部署校验 |

## 用户身份与消费 owner

[users](src/database/users) 在写锁内复核注册信封绑定的 UUID。论坛数字账号是稳定身份；`username` 是可变显示名和
MM 收件人，不能互换。[sessions](src/database/sessions) 保存会话摘要：Admin 启动撤销旧 Admin 会话，Public key
撤销/重签与旧会话永久撤销同事务提交；只读 inspect 不刷新过期时间。

凭据与 realtime 消费 owner 持有专用物理 session；操作复用原持锁连接，失效后不能从池补连接继续。
guard 退出关闭物理连接，不带锁归池。凭据网络调用不跨越数据库事务；部署身份不包含连接原文、账号或密码。

## 竞价事务和后端入口

竞价按 world → auction → lot 加锁：只读 world/auction 使用共享行锁，lot、活动代理、参与者资格和账本
保留排他锁，生命周期使用对应排他锁。取得锁后读取数据库时间，同一计划的代理、领先时间、截止、审计及接受事实共用该时刻。
命令账本、状态、审计和 realtime/outbox 同事务提交；Final 与轮次完成标记也原子写入。

`competition/postgres/` 按 bidding、lifecycle、admin 拆分。旧竞价账本重放仍在一致快照中取得冻结策略、授权版本和转换历史，
不重新执行当前状态门禁。竞价及关键生命周期使用 Serializable、明确行锁，最多三次尝试（含首次）；队列领取才使用
Read Committed 加锁。只重试已分类的序列化冲突/死锁，未知提交结果不能盲目重放，详见[不变量](../../docs/contracts/invariants.md)。

## 持久调度与慢速代理

scheduler 保存 due、租约、尝试次数和失败状态；完成/失败须匹配 owner、精确租约及版本。Admin 改期清除旧租约并推进版本，
陈旧任务不能收口。已存在轮次按冻结事实读取，配置变更不改写旧命令或时间。
自动 Draft 不提前物化运行期结束时间；后继 SQL 规则列与快照同取新模板，不复制或改写前驱经济策略。

slow proxy 扫描在 LIMIT 前排除已到截止的 lot；执行由竞价事务复核 generation、due、授权与资格。
调度字段与确定性命令持久化，不另建内存游标或 Redis 锁。

## 报表与恢复

| 职责 | 入口 |
| --- | --- |
| 公开目录、lot、账户、竞价日志 | `auctions/`、`lots/`、`account/`、`bid_log/` |
| 已提交结算事实与报告 | `price_history/`、`compensation/`、`settlement_report/` |
| 结算与履约 | `settlement_scheduler/`、`settlement_commands/`、`fulfillment_scheduler/` |
| 确认、提醒、退款与重挂 | `recovery_report/`、`recovery_commands/`、`recovery_scheduler/` |
| 邮件轮询、入站事实、outbox 与审查 | `mail_poll/`、`mail_intake/`、`mail_delivery/`、`admin_mail/` |
| 外部发送状态、fencing 与人工动作 | `delivery_scheduler/`、`delivery_admin/` |
| 论坛、凭据与告警 | `forum/`、`credentials/`、`operations_alert/` |
| 规则、审计与 readiness 水位 | `rules/`、`admin_audit/`、`health/`、`audit_health/` |

编号页 total 与当前页来自一致快照。禁用 world 的活动投影不能公开；已 settled 历史按契约读取。
[auctions/countdown.rs](src/database/auctions/countdown.rs) 用单条 SQL 读取同场公开时间及数据库时钟；
不加载装备、凭据或论坛正文，不公开草稿、取消场次、被活动场次遮蔽的预览或预览计划截止；无需新增表和迁移。
Public 拍卖、竞价日志、成交历史与补偿批次的游标查询仅对额外探测行使用饱和加法；不截断请求数量，保留既有快照和 has_more 语义。大页读取成本见[分页契约](../../docs/contracts/routes.md#public-分页世界与金额)。
Admin 读取不为补全投影临时写入。邮件展示和诊断 sidecar 均非权威数据，不参与幂等、完成证明或 cursor。

Delivery 进入 sending 前须证明命令已提交；未知发送结果进入 manual_reconcile，不自动重寄。
主动 Send 要求新增、唯一且完整匹配的 Sent 证明；流拍/撤拍逐件 Send 原装备，历史冻结 Return 不改写。
论坛未知 POST 先精确只读对账，再按持久人工授权重试；真实处理楼层独立于内容版本和 post ID。
`forum/catalog.rs` 拥有 `auction_forum_catalog_pages` 续页身份/正文；迁移 `000040` 增加主记录目录页数。
发布和刷新在主记录 claim/revision CAS 的同一 PostgreSQL 事务内更新所有续页，领取检查页数、连续楼层及互异身份。
`catalog_layout` 冻结历史单页/新分页方案；旧主题保持原楼层并继续处理出价，仍受原正文长度上限约束。
仅从未尝试、无远端身份的队列项及新主题启用分页，已尝试或未知写入不得借升级更换布局。页数冻结后不在扫描时移动系统日志。
`forum/reply/queries.rs` 从既有命令账本投影 `last_error_code` 与有界详情，不回填或改写历史拒绝。
新拒绝分类由 jobs owner 提供，仍经原 pending → rejected 条件更新收口；无需新增列或迁移。
论坛领取可按 Worker 首轮水位重新领取旧凭据失败和未知写入，原失败码进入 claim；不领取 `write_authorization_required`，
不伪造远端证明，updated_at 超过水位后只能走正常 due/lease 门禁。
Delivery Admin 重试由 `recovery_commands/commands/delivery_retry.rs` 在原 Serializable 命令事务恢复匹配的 Recovery
父子失败投影并推进 fencing；不生成成功证明、不清除其他动作阻断。证明规则见[不变量](../../docs/contracts/invariants.md)。

## 连接池可观测性

`DatabaseStore::pool_metrics_source` 暴露借出、空闲、总数、上限及真实 acquisition 累计指标。
普通查询、事务与显式 acquire 均经过 `TrackedPool`，不额外查询 SQL，也不暴露数据库名或连接串；外部原生 SQLx 池不计入累计值。
readiness 检查数据库、内嵌迁移及审计写入失败计数，异常失败关闭。

## 只读盘点与重建

[retention_inventory](src/database/retention_inventory) 在只读快照按版本化政策盘点关系、依赖和 realtime 水位，不授予删除资格。
[recovery_preflight](src/database/recovery_preflight) 只核对恢复目标，不运行 pg_restore 或覆盖对象。
[bootstrap](src/database/bootstrap) 在维护库身份、权限和集群锁下创建缺失库，不提供 DROP 或 migration；
[rebuild](src/database/rebuild) 是独立破坏性边界，要求精确目标、受限目录，并保留受支持的数据库属性。
备份、人工确认、停止进程、外部工具与 receipt 归 `auctionctl`；步骤见[数据库恢复与保留](../../ops/README.md#数据库恢复与保留)。

## 后端选择

配置层将 `[database]` 或兼容的 `postgres://` / `postgresql://` URL 交给本 crate；不在存储层解析 TOML。
默认不编译驱动，运行须显式启用 `postgres` feature，使用 SQLx PostgreSQL/rustls。

## 迁移

[PostgreSQL 迁移](../../migrations/postgres/README.md)只追加；普通启动不隐式迁移，执行门禁见[迁移导航](../../migrations/README.md)。
`initialization/` 只供确认后的合并首次空库初始化：同一专用连接持 SQLx 迁移锁，拒绝已有用户对象与其他客户端；
不建库、不删对象、不升级已有库。普通迁移与初始化共用 `000005`/`000015` 受限历史别名执行视图，不重复加锁或修改文件/checksum。

`recovery_preflight/catalog.rs` 拥有空库对象盘点；subscription 仅计当前库，default ACL 等残留仍阻断。
`migrations/history.rs` 核对完整 SQLx 历史前缀及 checksum，旧 archive 恢复不能只比较最大版本。
canonical v28 必须走[专用接管](../../ops/deploy/LEGACY_V28_ADOPTION.md)。

## 连续领先时间

`leader_since` 按全部接受事实判断连续领先是否中断，普通出价后被原领先者快速代理反超也重新计时。
`000037` 只重建可由追加式 bid 证明的未结算 live lot 投影；无证据不猜测，不改已结算金额、终态 lot 或历史审计。
冻结结算策略解码不应用新模板门禁，后继创建另由应用/领域校验。

## 维护与验证

```bash
cargo check --locked -p auction-storage --no-default-features --features postgres
```

仅静态编译，不执行迁移或行为测试。完整门禁见[开发指南](../../docs/development.md)，运行证据见[验证状态](../../docs/status.md)。
