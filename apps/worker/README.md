# auction-worker

持久化生命周期与集成 Worker 进程入口。

源码布局：`src/main.rs` 只负责 Tokio 入口；`src/bootstrap/mod.rs` 负责配置、缓存和运行模式
选择，`src/bootstrap/persistent.rs` 负责持久化 Worker 装配，`forum.rs`、`operations_alert.rs`、
`fulfillment.rs`、`control.rs` 和 `skeleton.rs` 分别负责各自的集成、履约、本地控制和无数据库启动 owner。
MoogleMail 装配位于 `src/bootstrap/mail/`：`mod.rs` 只负责组合，`credentials.rs`、`processor.rs`、
`runtime.rs` 和 `mapping.rs` 分别负责凭据恢复、业务/delivery、world poll runtime 和安全 envelope 映射。

Worker **不监听业务写入 HTTP 表面**，只从配置读取数据库与缓存设置，然后运行
`auction_jobs::run_persistent_with_settlement_until_shutdown()`；没有数据库时才保留不执行任务的启动骨架。
Worker 的主配置 `[tracing]` 可启用有界 OTLP/HTTP exporter，headers 只从敏感 TOML 的 Worker
专属字段读取。进程通过 `auction-process` 统一监听 SIGINT/SIGTERM；首次信号将 control 标记为
draining，通知各 owner 完成当前有界批次但不再领取新工作，并在 10 秒上限内停止 heartbeat、发布
停止状态；排空期间第二次信号会强制收口，最后按配置限时刷新 exporter。

```bash
AUCTION_CONFIG_FILE=ops/config/examples/worker.toml \
  cargo run -p auction-worker
```

Worker 配置没有 `listen_address`——该角色的默认监听地址为 `None`。
Worker 可以通过主配置的 `runtime_config_file` 读取 Admin 管理的共享业务文档；对 MoogleMail 只消费
main/isekai、发件人 allowlist、装备过滤和 14 个通知模板；Admin listener/security/session/
metrics/tracing 不会投影到 Worker。MoogleMail transport、凭据、mailbox identity 和全局
`enabled` 继续由 Worker 主配置拥有，runtime 更新后必须重启 Worker 才会生效。

共享文档的 `operations.economics/schedule/forum/alerts/worker/mail` 各自提供默认关闭的 `managed`；
接管后在启动时覆盖对应经济模板、周排期、论坛同步、运维告警、各独立任务循环及邮件调度/处理参数，再执行完整
启动校验。旧文件省略区块、或某组未接管时继续使用原公共/角色配置。经济模板和排期需同时重启
Admin/Worker，其余组重启 Worker；不会改写已有拍卖快照、任务游标、命令账本或租约状态。
邮件组支持 cron、读取批量/分页、HTTP 预算、租约续期和内联/持久化退避。管理页保存不会立即轮询，
调度仍服从已有的持久化 due、暂停状态和邮箱身份；参数范围见 runtime 示例。
mail gateway 复用已校验的 `fetch_timeout_seconds`，每次尝试由 Worker 传入绝对截止时刻；
同一预算覆盖限速和详情拉取，返回的完成前缀仍由 Worker 在业务处理及持久化证明后提交。

## 拍卖邮件起拍价

拍卖邮件正文未填写 `start` 时，每件装备默认以 50,000 Credits 起拍，正文可以留空、只写
说明或装备链接。可通过 `[worker.mooglemail].start_price` 显式覆盖邮件默认价，不继承公共
拍卖经济模板。单条 `start amount` 对邮件中的全部装备生效；逐件定价使用重复的
`<equipment-url> start amount` 三元组，三元组可以换行或放在同一行。普通空行、尾换行、
CRLF/CR、`<URL>` 包裹、URL 后的中英文句末标点和空白后开始的 `#` 注释都会被兼容处理。

`start` 位于行首、跟在装备链接后或带有金额时按定价输入校验；`starting`、`starter` 等普通
单词及句中的无价格说明不算指令。`start=10k`、`start:10k` 或 `start10k` 等误写仍会拒绝，
不能静默改用默认价。金额接受正整数、规范千分位以及能精确换算为整数 Credits 的 `k`/`m` 小数，例如
`1000`、`10,000`、`1.5k` 和 `0.5m`。零、负数、非整数换算和溢出继续失败关闭。逐件定价仍
必须恰好覆盖每个真实附件一次；未知装备 ID、ID/key 不匹配、重复或缺失指令不会被兼容逻辑
猜测或默认补齐。

## Cargo feature

- `cache-redis`：编译 Redis 缓存客户端。
- `postgres`：编译 PostgreSQL 完整存储和持久化 Worker 调度。
- `sqlite`：编译 SQLite Lite 存储和持久化 Worker 调度。

## 部署约束

Auction 生命周期、慢速代理和结算使用独立 `60s + [0,5s]` 主循环，默认每阶段批次 100、
PostgreSQL 并发 8、单项总时限 30 秒；SQLite Lite 的有效并发固定为 1 且不启用结算阶段。
每周轮次的 `[auction.schedule]` 默认关闭；启用后默认按 UTC 周四 22:00 开放预览、周五
00:00 正式开拍、周六 12:00 计划结束，星期、时间、时区和 world 均可通过 TOML 覆盖。配置中的
world 只是 allowlist：实际 bootstrap 集合还必须与 Admin runtime 中启用的 world 及数据库
`worlds.enabled=true` 取交集，并在每次 due 前重新读取数据库。业务 world 关闭后不创建新轮次，
但已持久化的 settlement、fulfillment、delivery 与 recovery 仍继续安全排空。
Fulfillment 与 Recovery 分别由
`[worker.fulfillment]` / `[worker.recovery]` 的独立 poll、jitter、batch、并发、lease 和单项超时
控制，Recovery 还维护独立 health 刷新周期，避免任一积压或远端余额读取放大普通拍卖任务。
MoogleMail 的每个 world 也各自拥有标准五字段 cron（默认 `*/5 * * * *`，即每 5 分钟）加
`[0,5s]` 抖动和
手动唤醒器；调度兼容旧版描述符、`@every` 与 `TZ=` / `CRON_TZ=` 前缀，读取与可重试业务处理默认各
三次、每次 30 秒；确定性业务/输入/配置错误按 `Permanent` 立即收口，只有 `Retryable` 和超时进入
有界重试。Worker 重启时会把旧配置遗留的过晚 `next_poll_at` 向当前 cron 收敛，但不会恢复已暂停邮箱、
越过活动 claim 或压缩尚未结束的失败退避。共享 transport 使用旧版一秒请求间隔与 15 秒请求超时，并在长批次期间续租；
Forum 使用独立循环按“主题发布 → 回帖导入”顺序执行，
任一阶段失败不会跳过另一阶段或终止其它 owner。
Lite 档（SQLite）不装配这两类完整档循环；其进程实例数和主并发必须为一，因为写入是串行事务，
运行手册需说明文件锁、锁等待和备份假设。

有数据库时，轮次 due、租约、尝试次数、完成标记和失败原因写入 `scheduled_rounds`；慢速代理
阶段直接扫描 `bid_proxy_authorizations.slow_due_at`，随后以旧版稳定身份进入竞价命令账本和事务
fence，不使用内存游标。PostgreSQL 完整档还把结算 workflow 的领取、稳定命令和完成 fence 写入 `auctions` 与
`auction_commands`，并把结算履约的 claim/fencing、delivery 准备/激活和 delivery outbox
终态写入数据库；SQLite Lite 明确不启用结算 workflow 与完整履约。PostgreSQL 完整档且启用
`[worker.mooglemail]` 时，Worker 还装配 MoogleMail 读取、拍卖及用户注册/重绑业务处理和
`DeliveryTransport`，执行静态通知、注册 key/结果通知、重绑结果通知以及 return/take；用户
登录 key 由独立的数据密钥加密保存，outbox 只保存用户 UUID 引用，发送前才在 Worker 内解密。
远端读取兼容当前 Item Shop 的 `Credits: amount` 与旧版 `Credits amount` 余额标签，以及
MoogleMail Inbox/Sent 固定英文表头、详情字段和发送完成标记的文本或受限 CSS 字形表示；两种
路径仍要求唯一结构、固定列序和逐字列名，余额也必须与脚本中的 `current_credits` 精确一致。
compose 的固定附件计数器同样兼容当前受限 CSS 字形，但仍需解码为规范的
`count / maximum items attached` 并与附件清单数量一致。
分页只兼容规范相对 URL 或 `https://hentaiverse.org/` 的同源绝对 URL，协议、host、端口、路径和
查询闭集仍逐项校验。
Sent 详情要求当前 `mail_state=2`；Inbox 含资产邮件要求可领取态 `1`，空附件邮件兼容打开详情前后
的 `1`/`2`，但不会放宽含资产邮件的领取状态。
共享 runtime 的 `[mail_templates]` 可按旧版 14 个通知键覆盖正文；字段、占位符和控制字符会在
启动时闭集校验，成功后冻结为本进程模板快照。旧 runtime 完全缺少业务区块时，才沿用
`[worker.mooglemail.mail_templates]`、直接 world/sender 和装备过滤兼容配置。
远端 POST 无法证明时进入人工对账，不自动重发；结算退信以及 Sent/Balance 回执对账已接入。
return 动作同时支持当前只读 `[RTS]` 回执与旧版 Inbox 缺席证明；当前回执必须把前后身份、固定
正文后缀、Credits/CoD/装备清单和动作消失逐项绑定，不能只凭标题前缀判定成功。
每个邮箱的持久化 claim 在批次执行期间按配置续租；token、owner、版本或续租 fence 失效后会
取消当前陈旧处理，后续只允许从权威 cursor 重放。
启用 `[worker.operations_alert]` 时，PostgreSQL Worker 会从 process、delivery、fulfillment 和
recovery owner 读取安全水位，按旧版 15 类 signal、退信子信号、阈值、fingerprint、cooldown 和 claim lease
生成有界通知批次；通知通过同一 MoogleMail delivery outbox 幂等入队，delivery 终态再回写
告警成功、失败或人工对账。scan、dispatch 和 reconcile 各自立即执行后按 `scan_interval_seconds +
[0, jitter_seconds]` 等待；取消会跳过等待且不影响 claim/fencing。普通 Create 失败会清空瞬时错误后持久化退避，identity 不一致立即
失败，第三次失败固定为 `retry_exhausted`；只有入队确认事务会推进 signal cooldown。
其中收件人不存在且明确要求取回资产的 delivery 会在原人工对账总量之外追加
`delivery_returned_mail` signal，摘要提示管理员核对后台交付页和 MM；该子信号沿用人工对账数量
阈值，且 operations alert 自身 delivery 不参与统计，避免退信提醒循环。
Recovery health 会按 `[worker.recovery]` 的独立周期读取两个 world
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
回帖导入：作者链接的 `showuser` 数字 ID 记录为稳定唯一的 `users.forum_account_id`，链接文本记录为
可变的 `users.username`，也是实际 MM 收件人和页面显示名称。已有用户直接复用；尚未注册 MM 的
作者会在 Users-owner Serializable 事务内建立 revoked-key 的 `forum_only` 临时主体及世界映射，不会
自动获得 active Key；后续 MM 注册按当前名称恢复并激活该用户。论坛只把普通 `bid` 交给现有竞价命令端口；
`proxy`/`proxy lower` 属于 Web 专属能力，论坛解析入口静默忽略且不写入固定 #2；仅含这些被忽略行的
帖子不会触发作者身份解析，历史已落账事实仍保留；
`start`、`cancel` 和 PM-only `notify` 保持 warning-only 并显示在固定 #2；MM `notify` 进入
MoogleMail delivery outbox。主题发布使用
持久化 claim/fencing，只为已经实际开放的 `live` 拍卖创建一次；装备真实 URL 在渲染前由
`equipment_data_key` 解密边界批量生成，明文 key 不进入快照或日志。远端写入不确定或本地证明写入失败时先按精确标题只读对账，命中只补本地
证明，未命中停在等待人工授权。只有 `auctionctl authorize-forum-write-retry` 经逐字目标确认写入
幂等授权和追加审计后才允许下一次 POST。主题固定 #1 拍卖目录、#2 编辑/错误记录；Worker 持久化
两个 exact post ID 后才处理 #3+，竞价结果提交后刷新 #1，并把编辑、解析、身份和确定拒绝更新到
#2；#2 同时显示能够连续证明的已处理楼层水位，仍待重试的命令会显示并阻止水位越过对应楼层。
固定帖刷新逐主题持有独立内容 lease/fencing，既有 #2 不是
系统标记时不会覆盖。回帖命令使用持久化账本和稳定命令 ID。
Forum 默认立即执行一轮，再等待 300 秒和最多五秒抖动；成功扫描把持久化下一次 due 推进一个完整
论坛周期。主题逐条领取，默认 claim 300 秒，发布和回帖单主题总时限 240 秒；主题读取默认最多
10 页、硬上限 20 页，超过上限明确失败而不截断出价。发布阶段失败后仍会继续本轮回帖导入。
论坛完整档不在 SQLite Lite 中伪装提供。当前 Worker 内存中存在论坛凭据时，首轮还会一次性释放早于
本进程启动水位的 `credential_unavailable` 主题任务；同一任务再次失败后不会在本进程内形成紧循环，
新导入凭据仍按 Admin 的 `restart_required` 提示重启 Worker 才进入新的恢复世代。
配置 `credential_data_key_id`/`credential_data_key` 后，Worker 会从 PostgreSQL 读取指定 world
或 shared scope 的唯一 active MoogleMail Cookie，在内存中解密；同一种外部 Cookie 由进程生命周期
advisory lock 保证只有一个 Worker owner。密钥不匹配、密文损坏或不是合法 UTF-8 时失败关闭；
暂时没有 active 凭据时保留 degraded Worker，实际轮询/写入再返回 credential unavailable。同一
world-aware Cookie 也供入站邮件的装备详情读取使用，避免把 HentaiVerse 登录页误判为不受支持的
装备来源。未配置该独立密钥时继续兼容 `.keyenv`，但 Worker 只使用 `hv-ck-key1`；
`hv-ck-key2` 仅供显式隔离验证，不参与运行时后备或自动切换。敏感配置仍是单 Cookie 入口。
启用 `[worker.mooglemail.diagnostics]` 后，业务拒绝、业务失败、处理超时以及 HTTP/解析异常会在普通安全日志
之外生成独立加密证据；证据保存不改变原有退回、投递、重试、游标或幂等结果。只保存异常实际取得的响应或提取
字段，正常邮件不保存；`body_unavailable` 只表示留证当时未取得完整响应，容量淘汰后的 UUID 导出返回
`not_found`。目录权限、容量淘汰和留证状态边界见
[`docs/operations/mail-diagnostics.md`](../../docs/operations/mail-diagnostics.md)。
Worker 在数据库可用时已有持久化启动、就绪、周期和正常退出后的停止心跳路径。
心跳的非敏感 `details.forum_sync_enabled` 固定为启动时完成 runtime 投影后的论坛启用值，供 Public
只读展示实际启用状态；关闭不会由历史帖子是否存在推断。升级此展示能力时须同时更新 Worker、
Public API 和公开端产物；修改开关后重启 Worker，页面会在下一轮状态读取时更新。周期心跳每轮读取
真实数据库/schema readiness、进程内审计写入失败计数和 Worker tracing 安全状态；SIGINT/SIGTERM
已可靠进入停止发布。各 owner 的内存定时器只负责唤醒，不能代替持久化游标。配置
`[worker.control].enabled = true` 后，数据库 Worker 和无数据库启动骨架都会
提供由安全 runtime 目录、实例锁、固定 group/0660 和显式 UID/GID peer allowlist 保护的本地
`live`、`ready`、`status` 控制面；后者只报告进程状态和零个 poller，不会伪造数据库或任务就绪。
