# 数据库对象与所有权契约

[文档导航](../README.md) · [PostgreSQL SQL](../../migrations/postgres) · [查询、事务与映射](../../crates/auction-storage/src/database)

按逻辑 owner 登记持久化对象与历史迁移范围；表存在不等于对应产品流程已验收。

## 本页导航

- [基线与 owner 规则](#基线与-owner-规则)
- [基础身份、配置与审计](#基础身份配置与审计)
- [拍卖与竞价核心](#拍卖与竞价核心)
- [结算、恢复与交付](#结算恢复与交付)
- [邮件、论坛与外部集成](#邮件论坛与外部集成)
- [实时事件与运维告警](#实时事件与运维告警)
- [共有约束](#共有约束)

## 基线与 owner 规则

当前运行 SQL 来自 `migrations/postgres/` 和 `auction-storage` 的查询/映射，只支持 PostgreSQL。
SQL owner 统一为 `auction-storage`；下表的逻辑 owner 决定业务事实含义，不另行拥有 SQL。
PostgreSQL 迁移为 `000001`—`000043`；已发布文件不修改或删除。
历史 SQL 的编号、对象和修复边界仍可追溯；实现与运行证据见[状态说明](../status.md)。

### 追加迁移与历史事实

所有修复只追加迁移，不改写已发布 SQL。以下边界影响升级与历史解释：

| 对象/迁移 | 可自动修复的范围 | 必须保留的历史边界 |
| --- | --- | --- |
| 连续领先时间：PostgreSQL `000013`/`000037` | 由追加式 bid 事实证明的 `leader_since`；后者覆盖未结算 live lot 同命令被快速代理反超 | 不猜测无证据时间，不改结算金额、bid 或审计 |
| 流拍寄回：PostgreSQL `000038` | ready 义务下从未领取、未冻结、无 delivery 的初始 pending execution 从 Return 转 Send，并追加审计 | 已尝试、冻结、未知或完成记录保持原身份；旧 Return 进入人工核对，禁止自动重寄 |
| Final 完成：PostgreSQL `000012`/`000039` | 持久化 Final fence；后者仅补齐唯一匹配成功命令、拍卖身份/状态/版本及权威 Final 时间的旧轮次 | 缺失或歧义证据不补写，不重放生命周期或外部动作 |
| 排期来源：PostgreSQL `000039` | nullable `scheduled_rounds.schedule_snapshot`，类型为 JSONB，冻结 IANA 时区和三个本地时间 | 有来源按下一本地周转换 UTC；历史/无时区手工轮次保持 NULL 与原 UTC 周期，不从当前配置反推 |

### 迁移语义与兼容边界

当前结构按下列语义解释，迁移编号用于定位 SQL，不代表另一套业务版本：

- 拍卖身份、前序顺序及唯一 Draft/Preview 录入目标由完整档延迟约束保护（`000014`/`000032`）。
  Preview 继续接收并物化装备，Open 或取消 Preview 才建立后继 Draft 并移动目标；升级时同世界
  Preview 与后继 Draft 并存会被拒绝，不自动选择保留哪一期。
- `bid_proxies` 是只读兼容视图；代理写入只进入 `bid_proxy_authorizations` 和历史表。
  `final_fast` 降额保持 Fast 模式、原状态及空终止原因，只能严格降额（PostgreSQL `000034`）。
- live/sold lot 必须有有效结束时间。PostgreSQL `000019` 仅为已先更新同 lot 投影、逐字段可证明的
  v28 INSERT 补齐 bid 截止事实，不干预当前显式写入。Isekai carrier 的派发 world 由 `000017` 冻结。
- 分类编号覆盖 One-Handed、Two-Handed、Staff、Shield、Cloth、Light、Heavy；每类从 1 递增。
  内部 `lot_number` 保持全局顺序，已开始发布的旧论坛主题冻结原全局编号方案（PostgreSQL `000024`）。
- 论坛 `showuser` 数字 ID 为规范十进制，同步到 `users.forum_account_id` 与
  `forum_bidder_identities.external_account_id`。一个 Users 主体跨 world/provider 只能沿用同一账号键；
  延迟约束保证提交时一致。`users.username` 是 MM 收件人/显示名称的唯一权威，`display_name` 只是论坛快照，
  `game_player_id` 只是名称兼容镜像。首次论坛竞价可建立 revoked-key `forum_only` 主体，后续 MM 注册
  按当前名称激活为 `mm_bound`，不创建影子名称或提前签发 active Key。
- 所有固定目录页及系统日志的远端 post ID 必须真实对账；历史 published 行没有身份时保留空值。
  `000040` 增加冻结的 `catalog_layout`、`catalog_page_count`（历史默认 1）及 `auction_forum_catalog_pages` 续页镜像。
  历史已尝试/发布/未知写入使用 `legacy_single`，保留楼层并继续导入；仅从未尝试且无远端身份的队列项及新主题用 `paged_v1`，每页最多 150 件。
  发布/刷新与主记录 claim/revision CAS 同事务写入，领取复核页数、连续楼层和互异身份。历史主题不自动改楼。
  `auction_forum_bid_commands` 的未知楼层保持 NULL，连续处理水位从系统日志楼层（目录页数加一）起步，只由完整主题扫描推进，
  不由 thread/post ID 或内容版本推算。账号与名称变化追加到 `forum_identity_history`（PostgreSQL `000025`—`000031`）。
- `mail_cookie`/`forum_cookie` 各自全局最多一条 active；`000030` 遇重复行拒绝迁移，必须先通过 Admin
  幂等停用并保留审计，不静默删除或选择凭据。告警状态与重试形状见[实时事件与运维告警](#实时事件与运维告警)。

canonical PostgreSQL v28 转换由 `legacy_v28` bridge 持有：精确格式预检和外部备份证据通过后，
补齐生命周期、领先投影、逐 bid 截止、代理兼容表/视图与索引，为等价覆盖的 SQLx `1..=5`、`7`
写入当前 checksum，再执行其余追加迁移；不修改来源 migration 或把来源版本号当作 SQLx 版本。

## 基础身份、配置与审计

| 表 | 逻辑 owner | 关键职责 |
| --- | --- | --- |
| worlds | auction-application：world | 世界边界、数据库权威启用状态与 `policy_revision` CAS |
| users | auction-application：users | 可变 MM/显示名称、稳定唯一论坛账号 ID、活动状态和版本；历史 game_player_id 仅作兼容镜像 |
| user_blacklist | auction-application：users | 公开资格与封禁原因 |
| user_renames | auction-application：users | 用户名变更追加历史 |
| public_sessions | auction-application：sessions | Public 会话权威状态；滑动闲置有效期受绝对有效期限制；长连接只读复核撤销、active 账号/凭据与双重过期，不刷新活动时间 |
| admin_sessions | auction-application：sessions | Admin 会话与 CSRF 关联状态；token_hash 为绑定 actor/secret 的 HMAC-SHA256 摘要，Admin 启动永久撤销所有未撤销行；保留滑动有效期、绝对上限与原撤销时间，不新增凭据校验值或列 |
| rules_documents | auction-application：rules | 版本化规则文档；PostgreSQL 首次接管可由 auctionctl 经同一应用端口导入受控目录 |
| audit_records | auction-application：audit | 追加式审计事实，不允许编辑历史 |
| service_heartbeats | auction-observability | 进程/服务心跳与过期判断 |
| credential_keys | auction-application：credentials | 加密后的外部凭据索引；邮件与论坛 Cookie 各自全局最多一条 active 记录 |
| user_login_key_envelopes | auction-application：users | 用户登录 key 密文信封；只保存密文、nonce、key id 和指纹 |
| user_mail_commands | auction-application：users | MoogleMail 注册/重绑命令幂等账本与结果 |
| user_game_identity_history | auction-application：users | MoogleMail 玩家名称换绑追加历史；game_player 字段仅作兼容镜像 |
| forum_identity_history | auction-storage：forum reply + audit | 论坛稳定数字账号或可变名称变更的追加式新旧快照，不覆盖历史证据 |
| user_blacklist_history | auction-application：users | 封禁变更追加历史 |
| user_admin_commands | auction-application：users | Admin 用户命令幂等账本与安全投影重放 |
| credential_admin_commands | auction-application：credentials | 凭据管理命令幂等账本 |
| runtime_config_admin_commands | auction-application：runtime-config | runtime 配置更新、指纹和 pending/applied/unknown 结果 |
| rules_admin_commands | auction-application：rules | 规则文档管理命令幂等账本 |

Worker 心跳在既有 `details` JSON 对象内发布布尔 `forum_sync_enabled`，值来自本进程启动时的
`[worker.forum].enabled`；observability/storage health owner 继续独占心跳写入。PostgreSQL 完整档
的 Public forum 读取 owner 仅聚合该字段：忽略过期和停止实例，任一 true 表示启用，全部明确 false
才表示关闭，缺失、非布尔或无有效实例保留未知。该展示投影不控制论坛任务或竞价，不改变表结构，
也不将心跳 details 原文公开给浏览器。

A32 使用 `credential_keys` 与 `credential_admin_commands`；后者到凭据行的引用为
`ON DELETE SET NULL`（`000009_credential_admin_compat`）。`AdminCredentialEnvelope` 校验 nonce、
key id 和指纹，账本只保存安全投影/删除结果。Worker 用独立数据密钥读取唯一 active Cookie，
Cookie 明文不进入账本。

## 拍卖与竞价核心

| 表 | 逻辑 owner | 关键职责 |
| --- | --- | --- |
| auctions | auction-domain + auction-application：auction | 拍卖生命周期、版本、时间点、前后轮关系和取消原因/操作者；完整档额外保护聚合身份、前序顺序与 Draft/Preview 唯一当前录入场次 |
| equipment | auction-domain + auction-application：auction | 标的事实与规范化装备属性 |
| auction_preview_items | auction-application：auction | 预览阶段的累积物化展示项；Preview 期间新录入装备按新 revision 追加 |
| lots | auction-domain + auction-application：auction | 拍品状态、金额、领先者、锁定版本、live/sold 有效结束时间 shape，以及内部全局顺序与分类内对外编号 |
| auction_commands | auction-application：command-ledger | 拍卖、竞价、世界启停与 realtime replay 等共用命令账本；其他 owner 使用下列专用账本 |
| bids | auction-domain + auction-application：auction | 追加式竞价事实 |
| lot_watches | auction-domain + auction-application：lot-watch；auction-storage：lot_watch | 按用户/lot 保存 watched、mm 渠道及正 i64 版本，取消保留墓碑；同值新命令仍推进 fence |
| lot_watch_leadership | auction-domain + auction-application：lot-watch；auction-storage：lot_watch | 逐条接受事实推进连续领先周期，同人加价不重开周期 |
| lot_watch_notifications | auction-application：lot-watch；auction-storage：lot_watch | 跨渠道用户/lot/周期唯一来源；冻结 channel、账号 preference_version、收件人及 payload/digest；MM 独占 delivery_id，PM 无 MM delivery；取消/重新领先永久失效 |
| lot_watch_pm_deliveries | auction-application：lot-watch-pm；auction-storage：lot_watch_pm | 独立 PM 租约/fence 队列，来源唯一；sending 后只允许原 sender 只读对账，缺论坛收件人 suppressed，不授权重发 |
| bid_proxies | auction-storage：兼容视图 | 历史代理字段的只读兼容投影，非权威状态 |
| bid_proxy_authorizations | auction-domain + auction-application：auction | Fast/Slow 代理授权状态机 |
| bid_proxy_authorization_transitions | auction-domain + auction-application：auction | 代理授权追加式迁移记录 |
| bid_proxy_outbid_uses | auction-domain + auction-application：auction | 代理出价次数和上限 |
| bid_proxy_attempts | auction-domain + auction-application：auction | 代理尝试历史与失败分类 |
| scheduled_rounds | auction-application + auction-jobs：auction-lifecycle | 持久化预览/开拍/Final phase 调度游标、租约、完成 fence、尝试次数和失败状态；Open 时为后继 Draft 建立下一轮 |
| auction_intake_targets | auction-application：auction-intake | 世界级拍品录入目标，并与该世界唯一 Draft 或 Preview 当前录入场次保持延迟一致性 |
| auction_submissions | auction-application：auction-intake | `mail/admin` 来源闭集及拍品来源关联；Admin URL/JSON 适配器种类只进入来源引用和命令摘要 |
| equipment_secrets | auction-application：keymanagement | 标的敏感来源密文 |
| auction_schema_identity | auction-storage：migration | 迁移/schema 身份和兼容检查 |

`000042` 的 `users.lot_watch_channel/lot_watch_channel_version` 由 application lot-watch 偏好 owner 定义，
SQL 仍由 auction-storage 独占，默认 mm/1，不改变身份版本。偏好命令复用 auction_commands，
只锁 user、先重放后 CAS，同渠道新命令也推进版本。新通知冻结偏好；历史 MM payload/digest 不重写。
PM 来源允许 recipient NULL 但只生成 recipient_missing 的 suppressed 队列事实，不伪造 UID、不回退 MM。
PM 任务 sending_at 与 sender 一旦绑定不可清空/替换；sending 后不可回 pending/claimed/suppressed，
由 `guard_lot_watch_pm_sending` 触发器保护。队列状态闭集为 pending/claimed/sending/succeeded/suppressed/manual_reconcile；
notification_id 主键及 `(notification_id,channel)` FK 保证一来源一 PM 任务，channel 固定 pm。
lease_token/lease_expires_at/lease_kind/worker_id 同空同非空；lease_kind 区分 send/reconcile，所有更新带 fence/token/状态/有效租约谓词。
succeeded 必须有原 sender、sending_at、数字 Sent external_reference 和完成时间；无原始 HTML/令牌/密钥字段。
attempt_count/reconcile_attempt_count 非负，fencing_version 正 i64；due/expiry/reconcile 索引支持每次至多一项领取。
来源的 MM 分支严格要求 delivery_id/recipient；PM 分支严格禁止 MM delivery_id、要求正 preference_version，
recipient 为空或符合 users 的 1–32 位规范十进制 ID。缺 UID 来源 payload 的 recipient_forum_account_id 为 JSON null，
不反序列化为可发送 ForumPmMessage；队列仅 suppressed/recipient_missing。已发送来源失效不抹除发送结果。

新增装备标记及超价通知对象由 application 的 lot-watch 逻辑 owner 定义，SQL 由 auction-storage 独占；
与 `lots`、`users` 和既有 `deliveries` 精确绑定。标记取消保留版本墓碑，通知按用户/lot/领先周期去重，
不能以浏览器缓存、论坛 notify 账本或任意模板替代来源授权。`000041` 仅补 active/final_fast 代理的标记及审计，
并以现有领先者/版本初始化周期，不补发历史提醒；新标记命令复用 `auction_commands`。
不修改 `000001`—`000040` 或既有竞价事实。delivery SQL 仍由 `mail_delivery`/`delivery_scheduler` 独占：
领取共用 watch proof，sending 在 lot → delivery 锁序下复核；失效且未开始发送的任务转为明确 failed 并追加审计，
不覆盖 sending、Sent 证明或 manual_reconcile。

`auctions.policy_snapshot` 保存创建时冻结的 schema v4 结算/恢复及竞价策略，与
`default_start_price`、软截止、取消截止、slow 间隔等规则列同事务写入，配置不回溯修改。
竞价和取消在锁内读取快照中的 PPM、代理开关/默认模式/次数上限及 final phase 参数；
`lots` 的 `leader_proxy_last`、慢速调度和状态版本同样是数据库权威状态。

应用竞价计划时，`bid_proxy_authorizations.created_at/updated_at` 与
`bid_proxy_authorization_transitions.created_at` 的新写入显式绑定计划统一的锁后数据库时间，
与命令结果内的转换时间一致；不依赖列默认值另行取时。既有表、迁移及历史记录保持不变。

管理命令复用既有对象，不另建状态表：

| 命令 | 完整档事务写入边界 |
| --- | --- |
| A40/A41 世界目录/启停 | `worlds` 先锁，仅从启用变为关闭时检查 `auctions`/`scheduled_rounds` blocker，状态、`auction_commands`、`audit_records` 同事务提交 |
| A38 提前结算 | `auction_early_settlement` 写通用账本，理由写审计，更新 `auctions`/`lots` 截止并追加 realtime；提交后由 settlement scheduler 建 workflow/义务 |
| A13 draft lot 修改 | 复用 `auctions`、`lots`、通用账本、审计与 realtime；撤回另写 `settlement_obligations`/`settlement_obligation_fulfillments` |

## 结算、恢复与交付

| 表 | 逻辑 owner | 关键职责 |
| --- | --- | --- |
| auction_settlements | auction-domain + auction-application：settlement | 拍卖结算汇总 |
| lot_settlements | auction-domain + auction-application：settlement | 拍品级买卖双方结算 |
| compensation_ledger | auction-domain + auction-application：settlement | 补偿池追加账本 |
| settlement_pools | auction-domain + auction-application：settlement | 结算池和策略快照 |
| settlement_obligations | auction-domain + auction-application：settlement | 买家/卖家履约义务 |
| settlement_obligation_fulfillments | auction-application：settlement | 履约交付证明与状态 |
| settlement_recoveries | auction-application：settlement-recovery | 付款/交付异常恢复聚合 |
| settlement_recovery_actions | auction-application：settlement-recovery | 恢复动作及 durable claim |
| settlement_recovery_commands | auction-application：settlement-recovery | 恢复确认命令幂等账本 |
| settlement_recovery_health | auction-application + auction-storage + auction-jobs | 恢复阻塞和健康投影；Worker 刷新，Admin 只读；战斗页暂时占用以 `busy` 投影并保留最后成功余额 |
| deliveries | auction-application + auction-jobs：delivery | 外部投递 outbox、claim 和重试 |
| delivery_admin_commands | auction-application：delivery | Admin 交付命令幂等账本 |

## 邮件、论坛与外部集成

| 表 | 逻辑 owner | 关键职责 |
| --- | --- | --- |
| inbound_mail | auction-storage：mail projection | 入站邮件事实索引及 Admin 审查安全投影；`sender`/`subject` 为可空标量，`message_display` 为受限脱敏快照 |
| mail_commands | auction-application + auction-jobs：mail intake | 入站完成命令账本，绑定 `inbound_mail`、32 字节内容摘要与处理结果；发送 outbox 由 `deliveries` 拥有 |
| mail_poll_state | auction-storage：mail state | MoogleMail 轮询游标、租约、失败退避和状态 |
| mail_admin_commands | auction-storage：mail admin command ledger | Admin 邮件控制命令幂等账本 |
| auction_forum_sync | auction-storage：forum sync/reply；auction-integrations + auction-jobs：forum | 论坛主题发布状态、租约、fencing、冻结 lot 编号/分页方案、目录页数，以及首帖/系统日志身份、内容版本、逐主题调度、真实连续处理楼层、最近成功时间和有界安全错误 |
| auction_forum_catalog_pages | auction-storage：forum/catalog；auction-integrations + auction-jobs：forum | 目录续页的连续楼层、真实 post ID 与正文证明；与主记录同事务发布及 CAS 更新，不是竞价权威事实 |
| forum_sync_admin_commands | auction-application + auction-storage + auctionctl：forum recovery | 论坛未知写入重新授权的幂等命令账本、目标确认结果和追加审计关联 |
| forum_bidder_identities | auction-storage：forum reply + Users owner | world/provider 参与映射、单用户账号归属、名称快照及 forum-only/MM-bound 生命周期；稳定账号权威值同步到 users.forum_account_id |
| auction_forum_reply_cursors | auction-storage：forum reply（历史兼容） | 保留的历史世界级回帖游标；当前无新写入者，逐主题调度由 `auction_forum_sync.content_next_scan_at` 拥有 |
| auction_forum_bid_commands | auction-storage：forum reply；auction-jobs：forum reply | 论坛竞价导入命令账本、可证明的真实楼层号与重试事实 |

用户注册/重绑的 `user_mail_commands` 归上文 Users owner，SQL 位于 `database/users/mail/postgres/`；
它不是由邮件传输维护的第二份发送账本，故不在本分组重复登记。

A16-A19 使用 `auction-storage` 的邮件表做 PostgreSQL 完整档状态与审查投影；A20 的全局
Worker control 唤醒不写入额外触发表；
`000008_mooglemail_polling` 追加 `mail_poll_state` 的 lease/fencing 与失败字段，Worker 在同一
事务中提交 `inbound_mail`、`mail_commands` 和 cursor。应用层定义命令与查询端口，
`auction-jobs`/`auction-integrations` 负责远程读取；基础通知和结算 delivery 已进入 outbox
transport；结算 send 的 Sent/Balance 严格解析、exact remote ID 查找和自动 proof 对账已接入
PostgreSQL Worker，无法证明远端 ID 时进入 `manual_reconcile`。论坛主题写入未知时先按精确标题
只读对账；未命中必须由 `forum_sync_admin_commands` 的持久授权打开下一次 POST。所有固定目录页与系统日志的
post ID 未持久化时，用户回帖不得导入；固定楼层更新以前一内容版本 CAS 收口，每个主题另以
独立 lease/fencing 串行刷新，并在完成时把 `content_next_scan_at` 推进到下一次论坛轮询周期；
`processed_through_post_number` 只由完整主题读取按远端楼层顺序推进，待重试命令使水位停在其前一楼。
`auction_forum_bid_commands.last_error_code` 保存 jobs 分类的具体拒绝码，`last_error_detail` 保留本地有界诊断；
应用读模型同时投影二者，Worker 按[安全英文展示规则](invariants.md#i-08-外部写入发生在本地提交之后)解释。
旧通用拒绝不回填、不重新执行；错误展示复用既有列，不改变自然键、尝试次数或终态。
`000043` 为 forum owner 的 `auction_forum_sync` 追加 `terminal_refresh_enabled`（默认 true）与
`terminal_synced_version`（nullable 正版本）。升级时已 settled 的既有主题仅设 enabled=false，完成版本仍为 NULL；
其余主题及新主题纳入，不回填历史正文或伪造成功。终态领取仅限 enabled=true 且完成版本为空的 settled 主题。
最终完成先共享锁定 auctions，核对已渲染的 settled state_version，再以原内容租约/fencing/revision CAS 同事务确认所有页；
版本变化、pending、部分进度或 write_unknown 不得写终态完成版本。普通 live 扫描即使期间结算，也必须重新渲染并证明全部固定页。
结算明细只从同一 Repeatable Read 快照的 auction_settlements/lot_settlements 与 lots 读取，不新增经济事实。

目录分页的 `000040` 只增加非权威镜像，不改竞价/审计事实；查询使用运行时绑定 SQL，无新增 SQLx 宏离线元数据。

MoogleMail 异常原始数据不进入数据库 schema：Worker 按独立运维契约写入本地加密证据文件。
证据文件不是入站事实、命令账本、幂等结果或游标的替代物，也不需要 migration；普通数据库投影
仍不得保存正文、Cookie、请求 payload、token 或装备 key 明文。

`migrations/postgres/000035_mail_intake_display_snapshot` 仅属于 PostgreSQL 完整档：向 `inbound_mail` 追加
nullable `sender`、`subject` 和 `message_display`。sender/subject 必须成对出现并受 256/512 字节边界约束；
展示快照固定 schema/redaction version，正文最多 64 KiB、JSON 最多 128 KiB、附件 ID 最多 1000 条，且只含
正文脱敏结果、附件外部 ID、Credits 和 COD。装备 key 与认证邮件正文不会进入快照。它是非权威展示投影，不能
参与入站幂等或 cursor；普通 replay 不会为历史 NULL 自动补写。

主动 ComposeSend 的 Sent proof 使用既有 delivery/outcome metadata 与外部引用字段，不新增表、列或 migration；
`sent_verified`、`external_reference_proven`、`proof` 和 `mooglemail:world:send:mid` 仍属于成功写回的安全
结果投影。Sent baseline/scan 失败只保留 `manual_reconcile`，不伪造新的数据库事实。

`migrations/postgres/000036_mail_intake_equipment_diagnostic` 仅为 PostgreSQL 完整档的 `inbound_mail` 增加
nullable `equipment_diagnostic` sidecar。它只在装备邮件被拒绝且首个装备读取失败时保存非权威安全投影，不能进入
`decision_metadata`、command digest、completion proof 或 cursor；成功读取只丢弃已加密准备内容。

## 实时事件与运维告警

| 表 | 逻辑 owner | 关键职责 |
| --- | --- | --- |
| realtime_streams | auction-storage：realtime；auction-application：Public relay；auction-jobs：consumer | 持久化 stream 目录；PostgreSQL 完整档包含 `auction:discovery` 派生流 |
| realtime_events | auction-storage：realtime；auction-application：Public relay；auction-jobs：consumer | 追加式事件、stream sequence、replay 关系、`auction.created` 发现别名及 Admin 时间倒序读取索引 |
| realtime_consumer_offsets | auction-application：Public relay；auction-jobs：外部 consumer | 按 consumer/stream 保存的连续 durable cursor；Public relay 与外部 consumer 使用不同 consumer key；以会话级 advisory lock 保证同 key 单 owner |
| realtime_failures | auction-application：Public relay；auction-jobs + auction-observability：外部 realtime | 事件消费隔离、失败事实和显式 replay 边界 |
| operations_alert_runtime_state | auction-application + auction-storage：alert health；auction-jobs：运行状态与扫描水位 | Worker 配置启用状态的非敏感单例投影，以及与 signal 观察事务同提交的成功扫描水位；两类时间独立单调，不互相覆盖 |
| operations_alert_signals | auction-application + auction-storage：alert signal；auction-jobs：扫描 | 告警信号、稳定 identity、版本、通知时间、cooldown 与已证明退信的独立提醒 |
| operations_alert_batches | auction-storage：alert batch；auction-jobs：claim/dispatch | 告警批次、租约/fencing、三次 Create 重试、稳定终态和 delivery 对账 |
| operations_alert_batch_items | auction-storage：alert batch | 告警批次成员、版本、冻结摘要和 signal 反查索引 |

告警目标变更时，旧待重试批次在领取事务内以 `dependency_mismatch` 终止，然后允许为新目标建批。
确定从未激活的关联 delivery 以 `failed` 收口，保留零 attempt/fencing 和空 claimed/sending 时间；
`reason_code=operations_alert_batch_terminal` 且 `outcome_unknown=false` 明确记录未发生外部发送。
错误码仅为 `dependency_mismatch` 或 `retry_exhausted`，收口与 `delivery.fail` 审计同事务提交。
每轮最多收口 100 个耗尽租约批次和 100 个符合相同未激活形状的历史孤儿，不改写发送过的 delivery。

## 共有约束

1. `migrations/postgres/` 是当前运行 schema 的迁移来源；新增 migration 必须评审列、约束、索引及 owner，
   不能从仓库外复制表定义。已发布 SQL 不得修改或删除。
2. auction_commands.command_id 是 UUID 主键；command_kind、aggregate_id、request_hash、
   result_code、result_payload、created_at 都是账本事实。request_hash 固定 32 字节，
   result_payload 必须是 JSON object。
3. bids.command_id、bid_proxy_authorizations.creation_command_id 及其他 command 外键不得改成可随意
   删除的弱引用。历史事实和幂等账本的删除顺序必须失败关闭。
4. realtime_events 只追加；修复事件通过 replay_of_event_id 引用历史事件，禁止编辑原事件。
   PostgreSQL 的 origin bid fact 唯一性只约束原始事件，不能阻止同一事实的运维 replay。
5. 当前写入必须满足 PostgreSQL 隔离与锁契约；历史迁移范围标记不能代替当前能力或并发保证。
6. `lots.lot_number` 是正数、场次内唯一且不可变的内部顺序；`category_key/category_number`
   组合为不可变的对外编号：`category_number` 为正数且在场次与分类内唯一，分类键必须与 equipment category 一致，
   撤回或重挂不会复用已经分配的编号。
7. 迁移提交前必须为每张表补充 Rust 查询 owner、适用的 SQLx 离线元数据策略及持久化必要性依据。
