# auction-worker

持久化生命周期与集成 Worker 进程入口。

源码布局：src/main.rs 只负责 Tokio 入口，src/bootstrap/mod.rs 负责配置、缓存、安全心跳和
Worker 生命周期装配。

Worker **不监听业务写入 HTTP 表面**，只从配置读取数据库与缓存设置，然后运行
`auction_jobs::run_persistent_with_settlement_until_shutdown()`；没有数据库时才保留不执行任务的启动骨架。
Worker 的主配置 `[tracing]` 可启用有界 OTLP/HTTP exporter，headers 只从敏感 TOML 的 Worker
专属字段读取；进程停止时会在限定时间内刷新并关闭 exporter。

```bash
AUCTION_CONFIG_FILE=ops/config/examples/worker.toml \
  cargo run -p auction-worker
```

Worker 配置没有 `listen_address`——该角色的默认监听地址为 `None`。

## Cargo feature

- `cache-redis`：编译 Redis 缓存客户端。
- `postgres`：编译 PostgreSQL 完整存储和持久化 Worker 调度。
- `sqlite`：编译 SQLite Lite 存储和持久化 Worker 调度。

## 部署约束

Auction 生命周期/结算使用独立 `60s + [0,5s]` 主循环，默认每类批次 100、PostgreSQL 并发 8、
单项总时限 30 秒；SQLite Lite 的有效并发固定为 1。Fulfillment 与 Recovery 分别由
`[worker.fulfillment]` / `[worker.recovery]` 的独立 poll、jitter、batch、并发、lease 和单项超时
控制，Recovery 还维护独立 health 刷新周期，避免任一积压或远端余额读取放大普通拍卖任务。
MoogleMail 的每个 world 也各自拥有标准五字段 cron（默认 `* * * * *`）加 `[0,5s]` 抖动和
手动唤醒器；调度兼容旧版描述符、`@every` 与 `TZ=` / `CRON_TZ=` 前缀，读取与业务处理默认各
三次、每次 30 秒；共享 transport 使用旧版一秒请求间隔与 15 秒请求超时，并在长批次期间续租；
Forum 使用独立循环按“主题发布 → 回帖导入”顺序执行，
任一阶段失败不会跳过另一阶段或终止其它 owner。
Lite 档（SQLite）不装配这两类完整档循环；其进程实例数和主并发必须为一，因为写入是串行事务，
运行手册需说明文件锁、锁等待和备份假设。

有数据库时，轮次 due、租约、尝试次数、完成标记和失败原因写入 `scheduled_rounds`；
PostgreSQL 完整档还把结算 workflow 的领取、稳定命令和完成 fence 写入 `auctions` 与
`auction_commands`，并把结算履约的 claim/fencing、delivery 准备/激活和 delivery outbox
终态写入数据库；SQLite Lite 明确不启用结算 workflow 与完整履约。PostgreSQL 完整档且启用
`[worker.mooglemail]` 时，Worker 还装配 MoogleMail 读取、拍卖及用户注册/重绑业务处理和
`DeliveryTransport`，执行静态通知、注册 key/结果通知、重绑结果通知以及 return/take；用户
登录 key 由独立的数据密钥加密保存，outbox 只保存用户 UUID 引用，发送前才在 Worker 内解密。
`[worker.mooglemail.mail_templates]` 可按旧版 14 个通知键覆盖正文；字段、占位符和控制字符
会在启动时闭集校验，成功后冻结为本进程模板快照。
远端 POST 无法证明时进入人工对账，不自动重发；结算退信以及 Sent/Balance 回执对账已接入。
每个邮箱的持久化 claim 在批次执行期间按配置续租；token、owner、版本或续租 fence 失效后会
取消当前陈旧处理，后续只允许从权威 cursor 重放。
启用 `[worker.operations_alert]` 时，PostgreSQL Worker 会从 process、delivery、fulfillment 和
recovery owner 读取安全水位，按旧版 15 类 signal、阈值、fingerprint、cooldown 和 claim lease
生成有界通知批次；通知通过同一 MoogleMail delivery outbox 幂等入队，delivery 终态再回写
告警成功、失败或人工对账。普通 Create 失败会清空瞬时错误后持久化退避，identity 不一致立即
失败，第三次失败固定为 `retry_exhausted`；只有入队确认事务会推进 signal cooldown。Recovery
health 会按 `[worker.recovery]` 的独立周期读取两个 world
的 Balance，低余额、过期证据和来源不可用都会进入安全投影，读取失败不会覆盖最后成功余额。
告警 delivery 在批次确认 `enqueued` 前不会被普通发送循环领取，
避免发送竞态；SQLite Lite 明确不提供该能力。
PostgreSQL recovery case 的 due 推进独立于 MoogleMail transport；当前已覆盖付款满足后的阶段推进、
付款超时与稳定 `payment_reminder` action。退回邮件会先按 obligation UUID 和 Credits/COD/附件摘要
严格绑定 recovery；匹配后 Worker 按 ordinal 领取 recovery Take，关联来源邮件、目标世界和预期资产
摘要，并按远端动作证明收口。COD 相关 Take 成功后，Worker 通过 Users owner 的幂等命令写入买家
黑名单，再以 `users_recovery_buyer_blacklisted` proof 完成 action。启用 MoogleMail transport 后，
邮件 action 才会关联 delivery outbox 并按发送证明、退避和人工复核边界收口；Worker 还会按
持久化 due 和 1-86400 秒可配置退避推进 `relist_pending` 的 guarded relist，并在 buyer refund
fulfillment 满足后把 `refunding` case 收口为 `completed`；这些 resolution 写入与 worker 审计均
在 PostgreSQL Serializable 事务内完成。
PostgreSQL 完整档启用 `[worker.forum]` 后，Worker 还会装配 E-Hentai 主题发布、主题读取和
回帖导入：身份必须先在 `forum_bidder_identities` 登记，`bid`、`proxy` 和 `proxy lower`
通过现有竞价命令端口执行；`start`、`cancel` 和 PM-only `notify` 在旧版 `forumingest` 中就是
warning-only，当前不越过该兼容边界；MM `notify` 进入 MoogleMail delivery outbox。主题发布使用
持久化 claim/fencing；远端写入不确定时进入人工对账，回帖命令使用持久化账本和稳定命令 ID。
Forum 默认立即执行一轮，再等待一分钟和最多五秒抖动；主题逐条领取，默认 claim 300 秒，发布
和回帖单主题总时限 93 秒，发布阶段失败后仍会继续本轮回帖导入。
论坛完整档不在 SQLite Lite 中伪装提供。
配置 `credential_data_key_id`/`credential_data_key` 后，Worker 会从 PostgreSQL 读取指定 world
与 shared scope 的 active MoogleMail 凭据池，在内存中逐个解密并按顺序切换；密钥不匹配、密文
损坏或不是合法 UTF-8 时失败关闭；暂时没有 active 凭据时保留 degraded Worker，实际轮询/写入
再返回 credential unavailable。未配置该独立密钥时继续兼容 `.keyenv` 的
`hv-ck-key1` Bot 与 `hv-ck-key2` 测试号，以及敏感配置的单 Cookie 入口。
Worker 在数据库可用时发布持久化启动/就绪/周期/停止心跳；各 owner 的内存定时器只负责唤醒，不能代替
持久化游标。配置 `[worker.control].enabled = true` 后，数据库 Worker 和无数据库启动骨架都会
提供由安全 runtime 目录、实例锁、固定 group/0660 和显式 UID/GID peer allowlist 保护的本地
`live`、`ready`、`status` 控制面；后者只报告进程状态和零个 poller，不会伪造数据库或任务就绪。
