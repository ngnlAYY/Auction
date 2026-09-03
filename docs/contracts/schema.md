# 数据库对象契约清单

## 基线与 owner 规则

本清单基于 Auction-BF 提交 f109146e246c6bbfefd7f4749ac2bba087bcfa0b 的
28 个 PostgreSQL up migration，逐项提取出 61 张表。来源目录为旧仓库
apps/backend/internal/database/migrations/。

这里区分两种 owner：

- SQL owner：所有 Rust migration、查询和数据库记录映射统一由 auction-storage 持有；
- 逻辑 owner：决定表中业务事实含义的目标领域/应用/适配器边界。它不是当前代码已经
  实现的证明，而是迁移时必须遵守的唯一事实 owner。

档位含义：

- 两者：目标是 PostgreSQL 完整档与 SQLite Lite 档都支持，但 Lite 仍受单主机、单写者和
  能力子集约束；
- 完整档：只承诺 PostgreSQL；SQLite 不得因为误建表而被误认为支持该能力。

本表先冻结所有旧对象的归属和候选档位；真正提交 Rust migration 前，必须把 Lite 表项与
SQLite 事务、锁等待、备份和 Worker 并发约束再次核对。

当前已提交 PostgreSQL `000001` 至 `000032`，覆盖核心竞价、结算/履约、生命周期、
代理历史、身份/外部命令、A32 凭据兼容修正、恢复/可观测性和论坛同步对象；SQLite 已提交
`000001` 至 `000010` 作为共有能力子集及生命周期/领先时间/lot 结束时间、分类编号与用户论坛账号
身份修正。旧系统的 `bid_proxies` 不再作为第二份
权威表维护，PostgreSQL 与 SQLite 均以只读兼容视图投影新授权模型；写入只进入
`bid_proxy_authorizations` 及其历史表。完整适配器、Lite 串行写入和其他业务表面仍在迁移中。

核心竞价切片额外把竞价规则快照字段放在 `auctions`，把 `leader_proxy_last`、慢速代理
调度字段和状态版本放在 `lots`；`auction-storage` 的 PostgreSQL 事务适配器只读取并更新
这些权威字段，不在内存中另存一份竞价状态。PostgreSQL `000012` 与 SQLite `000006` 为
持久化轮次补齐 Final phase 完成 fence；PostgreSQL `000013` 与 SQLite `000007` 只修正可由
追加式 bid 事实证明的 `leader_since`，无法证明时不猜测历史。PostgreSQL `000014` 先为早期
Rust schema 中可唯一映射的既有 draft 回填 intake target，再追加拍卖身份、前序顺序和
draft/target 跨行一致性触发器；这是完整档数据库防线，不要求 SQLite 复制 PostgreSQL 的延迟
约束触发器机制。
PostgreSQL `000015` 统一 operations alert signal identity、批次 lease/retry/terminal 形状与稳定
错误码；普通退避保持空错误码，第三次 Create 失败固定为 `failed/retry_exhausted`，signal cooldown
只在批次和 delivery 原子确认入队后推进。
PostgreSQL `000016` 为论坛主题写入结果未知后的人工再授权新增
`forum_sync_admin_commands`：精确标题只读对账未命中时，只有幂等运维命令和追加审计可以打开下一次
远端 POST；该账本不保存 Cookie、远端响应或任意敏感配置。
PostgreSQL `000017` 固定 Isekai carrier 的派发 world；`000018` 为 live/sold lot 恢复非空有效结束
时间约束；`000019` 只为已先更新同一 lot 投影的旧 v28 INSERT 补齐遗漏的 bid 有效结束时间，新版
显式写入和无法逐字段证明属于同一事实的行都不经过该兼容分支；`000020` 扩展 operations alert
kind/identity 闭集，为已证明的 MoogleMail 退信提供独立提醒，同时保留总人工对账 signal；
`000021` 保存 Worker 发布的非敏感启用状态和成功扫描水位，Admin 不从 signal 是否为空推断配置；
`000022` 解除配置状态发布时间与扫描水位之间不存在的先后约束，状态发布仍按自身时间单调校验，
成功扫描只推进自己的水位；`000023` 扩展 recovery 余额状态闭集，以 `busy` 表达 HV 账号仍有效
但因处于战斗页而暂时无法读取 Item Shop 的可恢复状态；PostgreSQL `000024` 与 SQLite `000009`
为 `lots` 增加不可变 `category_key/category_number`：`lot_number` 继续作为场次内全局顺序，
用户可见编号则在 One-Handed、Two-Handed、Staff、Shield、Cloth、Light、Heavy 各分类内从 1
独立递增。PostgreSQL 还为已开始发布的论坛主题冻结旧全局编号方案，避免迁移后改变既有帖子命令。
PostgreSQL `000025` 追加固定 #1 拍卖目录与 #2 系统日志的远端 post ID、系统日志正文、内容版本、
同步时间和独立内容刷新 claim/lease/fencing；历史 published 行保持空身份，必须由 Worker 读取远端
主题对账，不以 thread ID 或楼层位置伪造 post proof。
论坛 `showuser` 数字 ID 同时写入 `forum_bidder_identities.external_account_id` 和
`users.forum_account_id`，作为稳定、唯一且规范十进制的论坛账号键；论坛用户名与 MoogleMail
收件人共用可变的 `users.username`，映射表 `display_name` 只保存论坛侧快照及
`forum_only/mm_bound` 生命周期，不是第二份名称权威源。旧 `users.game_player_id` 只保留为
`username` 的数据库兼容镜像，不进入新业务 DTO。首次论坛竞价可以在 Users-owner Serializable
事务内建立 revoked-key `forum_only` 用户，不生成第二套影子用户名或 active Key；后续 MM 注册按
当前名称恢复并激活该主体。PostgreSQL `000026` 保证同一 world/provider 下一个 Users 主体只能绑定
一个论坛账号；`000027` 再把该稳定账号归属提升到 Users 主体，并用延迟约束保证 Users 与该主体全部
论坛映射在事务提交时一致；跨世界复用必须沿用相同数字键。
`000028` 为 `auction_forum_bid_commands` 记录可证明的真实楼层号，历史未知值保持空；`000029`
为固定帖刷新增加最近成功时间、最近失败时间和有界安全错误码，并建立追加式
`forum_identity_history` 保存论坛数字账号与名称变更的新旧快照。
`000030` 在建立索引前拒绝仍有同类重复 active 行的数据库，再用 partial unique index 保证
`mail_cookie` 与 `forum_cookie` 各自全局最多一条 active 记录。迁移不静默选择保留项；已有重复
凭据必须先通过 Admin 的幂等状态命令停用，使命令账本和追加审计保持完整。`000031` 为
`auction_forum_sync` 增加真实连续处理楼层；历史行从 #2 安全起步，必须由后续完整主题扫描推进，
不能从固定帖内容版本或远端 post ID 猜测回填。
PostgreSQL `000032` 把世界级录入目标从仅 `draft` 扩展为唯一的 `draft|preview` 当前场次：Preview
继续接收并物化新装备，Open 才原子创建后继 Draft、登记下一轮调度并迁移录入目标；若 Preview 被取消，
取消事务也必须建立后继 Draft，避免世界失去可用录入目标。迁移会拒绝旧实现留下的同世界 Preview 与
后继 Draft 并存窗口，必须在窗口外升级，不能猜测应折叠或保留哪一期。

Auction-BF v28 到当前 schema 的转换由专用 `legacy_v28` bridge 持有，不修改旧 migration，也不
把 golang-migrate 版本号直接映射成 SQLx 版本。它只在精确旧库预检和外部备份证据通过后补齐
Rust 需要的生命周期列、lot 领先投影、逐 bid 截止事实、代理兼容表/视图及索引，并为等价覆盖的
SQLx `1..=5`、`7` 写入当前 checksum；其余迁移仍按本目录顺序向前执行。

## 基础身份、配置与审计

| 表 | 旧迁移 | 逻辑 owner | 档位 | 关键职责 |
| --- | --- | --- | --- | --- |
| worlds | 000001_baseline | auction-application：world | 两者 | 世界边界与启用状态 |
| users | 000001_baseline + PostgreSQL 000027_user_forum_account_identity / SQLite 000010_user_forum_account_identity | auction-application：users | 两者 | 可变 MM/显示名称、稳定唯一论坛账号 ID、活动状态和版本；旧 game_player_id 仅作兼容镜像 |
| user_blacklist | 000001_baseline | auction-application：users | 两者 | 公开资格与封禁原因 |
| user_renames | 000004_identity_external_admin | auction-application：users | 两者 | 用户名变更追加历史 |
| public_sessions | 000001_baseline | auction-application：sessions | 两者 | Public 会话权威状态 |
| admin_sessions | 000001_baseline | auction-application：sessions | 两者 | Admin 会话与 CSRF 关联状态 |
| rules_documents | 000001_baseline | auction-application：rules | 两者 | 版本化规则文档；PostgreSQL 首次接管可由 auctionctl 经同一应用端口导入旧目录 |
| audit_records | 000001_baseline | auction-application：audit | 两者 | 追加式审计事实，不允许编辑历史 |
| service_heartbeats | 000001_baseline | auction-observability | 两者 | 进程/服务心跳与过期判断 |
| credential_keys | 000004_identity_external_admin + 000009_credential_admin_compat + 000030_single_active_external_cookie | auction-application：credentials | 完整档 | 加密后的外部凭据索引；邮件与论坛 Cookie 各自全局最多一条 active 记录 |
| user_login_key_envelopes | 000004_identity_external_admin / 000003_user_admin | auction-application：users | 两者 | 用户登录 key 密文信封；只保存密文、nonce、key id 和指纹 |
| user_mail_commands | 000002_worker_external_adapters | auction-application：users | 完整档 | MoogleMail 注册/重绑命令幂等账本与结果 |
| user_game_identity_history | 000002_worker_external_adapters | auction-application：users | 完整档 | 旧命名的 MoogleMail 玩家名称换绑追加历史；game_player 字段仅作兼容镜像 |
| forum_identity_history | 000029_forum_sync_health_identity | auction-storage：forum reply + audit | 完整档 | 论坛稳定数字账号或可变名称变更的追加式新旧快照，不覆盖历史证据 |
| user_blacklist_history | PostgreSQL 000004_identity_external_admin / SQLite 000001_lite_core | auction-application：users | 两者 | 封禁变更追加历史 |
| user_admin_commands | 000004_identity_external_admin / 000003_user_admin | auction-application：users | 两者 | Admin 用户命令幂等账本与安全投影重放 |
| credential_admin_commands | 000004_identity_external_admin + 000009_credential_admin_compat | auction-application：credentials | 完整档 | 凭据管理命令幂等账本 |
| runtime_config_admin_commands | 000004_identity_external_admin | auction-application：runtime-config | 完整档 | runtime 配置更新、指纹和 pending/applied/unknown 结果 |
| rules_admin_commands | 000003_rules_admin_commands | auction-application：rules | 完整档 | 规则文档管理命令幂等账本 |

A32 复用旧身份/外部命令迁移创建的 `credential_keys` 与
`credential_admin_commands`，由 `000009_credential_admin_compat` 补齐 `forum_cookie` 闭集，
并把命令账本到凭据行的引用改为 `ON DELETE SET NULL`。凭据密文本体由 Rust 的
`AdminCredentialEnvelope` 约束 nonce、key id 和指纹；`credential_admin_commands.result_payload`
只保存安全投影或删除结果，不能保存 Cookie 明文。该能力只承诺 PostgreSQL 完整档，Worker
通过独立数据密钥读取唯一 active Cookie，SQLite 不复制这些完整档表。

## 拍卖与竞价核心

| 表 | 旧迁移 | 逻辑 owner | 档位 | 关键职责 |
| --- | --- | --- | --- | --- |
| auctions | 000001_baseline + PostgreSQL 000003_lifecycle_proxy_history + 000014_lifecycle_database_invariants + 000032_preview_intake_handoff / SQLite 000004_auction_cancellation + 000005_lifecycle_metadata | auction-domain + auction-application：auction | 两者 | 拍卖生命周期、版本、时间点、前后轮关系和取消原因/操作者；完整档额外保护聚合身份、前序顺序与 Draft/Preview 唯一当前录入场次 |
| equipment | 000001_baseline | auction-domain + auction-application：auction | 两者 | 标的事实与规范化装备属性 |
| auction_preview_items | 000001_baseline | auction-application：auction | 两者 | 预览阶段的累积物化展示项；Preview 期间新录入装备按新 revision 追加 |
| lots | 000001_baseline + PostgreSQL 000018、000024 / SQLite 000008、000009 | auction-domain + auction-application：auction | 两者 | 拍品状态、金额、领先者、锁定版本、live/sold 有效结束时间 shape，以及内部全局顺序与分类内对外编号 |
| auction_commands | 000001_baseline | auction-application：command-ledger | 两者 | 所有可变命令的幂等账本 |
| bids | 000001_baseline | auction-domain + auction-application：auction | 两者 | 追加式竞价事实 |
| bid_proxies | 000001_baseline | auction-storage：兼容视图 | 两者 | 旧代理字段的只读兼容投影，非权威状态 |
| bid_proxy_authorizations | 000025_auction_bidding_state_model | auction-domain + auction-application：auction | 两者 | Fast/Slow 代理授权状态机 |
| bid_proxy_authorization_transitions | 000025_auction_bidding_state_model | auction-domain + auction-application：auction | 两者 | 代理授权追加式迁移记录 |
| bid_proxy_outbid_uses | 000012_auction_policy_snapshot | auction-domain + auction-application：auction | 两者 | 代理出价次数和上限 |
| bid_proxy_attempts | 000024_proxy_bid_attempt_history | auction-domain + auction-application：auction | 两者 | 代理尝试历史与失败分类 |
| scheduled_rounds | 000013_auction_scheduled_rounds + 000006_worker_scheduler + PostgreSQL 000012_scheduled_final_phase / SQLite 000006_scheduled_final_phase | auction-application + auction-jobs：auction-lifecycle | 两者 | 持久化预览/开拍/Final phase 调度游标、租约、完成 fence、尝试次数和失败状态；Open 时为后继 Draft 建立下一轮 |
| auction_intake_targets | 000020_auction_lifecycle_hardening + PostgreSQL 000014_lifecycle_database_invariants + 000032_preview_intake_handoff | auction-application：auction-intake | 完整档 | 世界级拍品录入目标，并与该世界唯一 Draft 或 Preview 当前录入场次保持延迟一致性 |
| auction_submissions | 000002_worker_external_adapters | auction-application：auction-intake | 完整档 | `mail/admin` owner 闭集的拍品提交事实；Admin 的 URL/JSON 适配器种类只进入来源引用和命令摘要 |
| equipment_secrets | 000002_worker_external_adapters | auction-application：keymanagement | 完整档 | 标的敏感来源密文 |
| auction_schema_identity | 000001_baseline | auction-storage：migration | 两者 | 迁移/schema 身份和兼容检查 |

当前 Rust 迁移中的 `auctions.policy_snapshot` 由 PostgreSQL `000002_settlement_core` 建立，
保存创建拍卖时冻结的 schema v4 结算/恢复及竞价策略。它与 `default_start_price`、软截止、
取消截止和 slow 间隔等遗留规则列在同一创建事务中写入；配置更新不会回溯改变已创建拍卖。
竞价和取消事务在锁内读取该快照，恢复 PPM、代理开关/默认模式/次数上限和 final phase 参数。

A38 不新增表或修改结算事实 schema：`auction_early_settlement` 继续写入通用
`auction_commands`，理由与动作上下文进入追加式 `audit_records`，截止时间只更新 `auctions`/`lots`
现有字段，并追加已登记的 realtime 事件。事务提交后仍由 PostgreSQL settlement scheduler 建立
workflow 并写入下方结算、义务与恢复表；SQLite Lite 没有该 owner，因此不声明 capability。

A13 不新增表：Admin draft lot 修改由 `auction-storage` 作为 SQL owner 复用 `auctions`、`lots`、
`auction_commands`、`audit_records`、`realtime_streams`/`realtime_events`，撤回时再写入下方
已有的 `settlement_obligations` 与 `settlement_obligation_fulfillments`。应用层仍只通过窄端口
决定动作和事务顺序，不能在 HTTP 或页面层直接访问这些表。

## 结算、恢复与交付

| 表 | 旧迁移 | 逻辑 owner | 档位 | 关键职责 |
| --- | --- | --- | --- | --- |
| auction_settlements | 000001_baseline | auction-domain + auction-application：settlement | 完整档 | 拍卖结算汇总 |
| lot_settlements | 000001_baseline | auction-domain + auction-application：settlement | 完整档 | 拍品级买卖双方结算 |
| compensation_ledger | 000001_baseline | auction-domain + auction-application：settlement | 完整档 | 补偿池追加账本 |
| settlement_pools | 000014_settlement_policy_obligations | auction-domain + auction-application：settlement | 完整档 | 结算池和策略快照 |
| settlement_obligations | 000014_settlement_policy_obligations | auction-domain + auction-application：settlement | 完整档 | 买家/卖家履约义务 |
| settlement_obligation_fulfillments | 000015_settlement_fulfillment_delivery | auction-application：settlement | 完整档 | 履约交付证明与状态 |
| settlement_recoveries | 000005_recovery_observability_forum | auction-application：settlement-recovery | 完整档 | 付款/交付异常恢复聚合 |
| settlement_recovery_actions | 000005_recovery_observability_forum | auction-application：settlement-recovery | 完整档 | 恢复动作及 durable claim |
| settlement_recovery_commands | 000005_recovery_observability_forum | auction-application：settlement-recovery | 完整档 | 恢复确认命令幂等账本 |
| settlement_recovery_health | 000005_recovery_observability_forum + 000023_recovery_balance_busy | auction-application + auction-storage + auction-jobs | 完整档 | 恢复阻塞和健康投影；Worker 刷新，Admin 只读；战斗页暂时占用以 `busy` 投影并保留最后成功余额 |
| deliveries | 000001_baseline | auction-application + auction-jobs：delivery | 完整档 | 外部投递 outbox、claim 和重试 |
| delivery_admin_commands | 000005_delivery_admin_commands | auction-application：delivery | 完整档 | Admin 交付命令幂等账本 |

## 邮件、论坛与外部集成

| 表 | 旧迁移 | 逻辑 owner | 档位 | 关键职责 |
| --- | --- | --- | --- | --- |
| inbound_mail | 000001_baseline | auction-storage：mail projection | 完整档 | 入站邮件原始事实索引的安全读取来源 |
| mail_commands | 000001_baseline | auction-integrations + auction-jobs：mail | 完整档 | 邮件发送 outbox/命令 |
| mail_poll_state | 000001_baseline + 000008_mooglemail_polling | auction-storage：mail state | 完整档 | MoogleMail 轮询游标、租约、失败退避和状态 |
| user_mail_commands | 000002_worker_external_adapters | auction-integrations + auction-jobs：mail | 完整档 | 用户邮件外部写入幂等账本 |
| mail_admin_commands | 000004_mail_admin_commands | auction-storage：mail admin command ledger | 完整档 | Admin 邮件控制命令幂等账本 |
| auction_forum_sync | 000005_recovery_observability_forum + 000010_forum_sync_fencing + 000024_lot_category_numbering + 000025_forum_fixed_posts + 000029_forum_sync_health_identity + 000031_forum_processed_floor | auction-storage：forum sync/reply；auction-integrations + auction-jobs：forum | 完整档 | 论坛主题发布状态、租约、fencing、冻结 lot 编号，以及固定 #1/#2 的远端身份、内容版本、逐主题调度、真实连续处理楼层、最近成功时间和有界安全错误 |
| forum_sync_admin_commands | 000016_forum_sync_write_authorization | auction-application + auction-storage + auctionctl：forum recovery | 完整档 | 论坛未知写入重新授权的幂等命令账本、目标确认结果和追加审计关联 |
| forum_bidder_identities | 000005_recovery_observability_forum + 000026_forum_identity_owner_unique | auction-storage：forum reply + Users owner | 完整档 | world/provider 参与映射、单用户账号归属、名称快照及 forum-only/MM-bound 生命周期；稳定账号权威值同步到 users.forum_account_id |
| auction_forum_reply_cursors | 000005_recovery_observability_forum | auction-storage：forum reply（历史兼容） | 完整档 | 保留的旧世界级回帖游标；当前无新写入者，逐主题调度由 `auction_forum_sync.content_next_scan_at` 拥有 |
| auction_forum_bid_commands | 000005_recovery_observability_forum + 000028_forum_command_post_number | auction-storage：forum reply；auction-jobs：forum reply | 完整档 | 论坛竞价导入命令账本、可证明的真实楼层号与重试事实 |

A16-A19 使用 `auction-storage` 的邮件表做 PostgreSQL 完整档状态与审查投影；A20 的全局
Worker control 唤醒不写入额外触发表；
`000008_mooglemail_polling` 追加 `mail_poll_state` 的 lease/fencing 与失败字段，Worker 在同一
事务中提交 `inbound_mail`、`mail_commands` 和 cursor。应用层定义命令与查询端口，
`auction-jobs`/`auction-integrations` 负责远程读取；基础通知和结算 delivery 已进入 outbox
transport；结算 send 的 Sent/Balance 严格解析、exact remote ID 查找和自动 proof 对账已接入
PostgreSQL Worker，无法证明远端 ID 时进入 `manual_reconcile`。论坛主题写入未知时先按精确标题
只读对账；未命中必须由 `forum_sync_admin_commands` 的持久授权打开下一次 POST。固定 #1/#2 的
post ID 未持久化时，#3 及后续回帖不得导入；固定楼层更新以旧内容版本 CAS 收口，每个主题另以
独立 lease/fencing 串行刷新，并在完成时把 `content_next_scan_at` 推进到下一次论坛轮询周期；
`processed_through_post_number` 只由完整主题读取按远端楼层顺序推进，待重试命令使水位停在其前一楼；
SQLite Lite 不复制这些完整档表。

## 实时事件与运维告警

| 表 | 旧迁移 | 逻辑 owner | 档位 | 关键职责 |
| --- | --- | --- | --- | --- |
| realtime_streams | 000001_baseline + 000011_realtime_discovery_alias | auction-storage：realtime；auction-application：Public relay；auction-jobs：consumer | 两者 | 持久化 stream 目录；PostgreSQL 完整档包含 `auction:discovery` 派生流 |
| realtime_events | 000001_baseline + 000007_realtime_replay + 000011_realtime_discovery_alias | auction-storage：realtime；auction-application：Public relay；auction-jobs：consumer | 两者 | 追加式事件、stream sequence、replay 关系和 PostgreSQL `auction.created` 发现别名 |
| realtime_consumer_offsets | 000001_baseline | auction-application：Public relay；auction-jobs：外部 consumer | 两者 | 按 consumer/stream 保存的连续 durable cursor；Public relay 与外部 consumer 使用不同 consumer key；PostgreSQL 以会话级 advisory lock 保证同 key 单 owner，SQLite 在同主机以 0600 非阻塞 sidecar 文件锁拒绝重复 owner，非 Unix 平台无法证明互斥时失败关闭 |
| realtime_failures | 000001_baseline | auction-application：Public relay；auction-jobs + auction-observability：外部 realtime | 两者 | 事件消费隔离、失败事实和显式 replay 边界 |
| operations_alert_runtime_state | 000021_operations_alert_runtime_state + 000022_operations_alert_runtime_clock_split | auction-application + auction-storage：alert health；auction-jobs：运行状态与扫描水位 | 完整档 | Worker 配置启用状态的非敏感单例投影，以及与 signal 观察事务同提交的成功扫描水位；两类时间独立单调，不互相覆盖 |
| operations_alert_signals | 000005_recovery_observability_forum + 000015_operations_alert_state_invariants + 000020_operations_alert_returned_mail | auction-application + auction-storage：alert signal；auction-jobs：扫描 | 完整档 | 告警信号、稳定 identity、版本、通知时间、cooldown 与已证明退信的独立提醒 |
| operations_alert_batches | 000005_recovery_observability_forum + 000015_operations_alert_state_invariants | auction-storage：alert batch；auction-jobs：claim/dispatch | 完整档 | 告警批次、租约/fencing、三次 Create 重试、稳定终态和 delivery 对账 |
| operations_alert_batch_items | 000005_recovery_observability_forum + 000015_operations_alert_state_invariants | auction-storage：alert batch | 完整档 | 告警批次成员、版本、冻结摘要和 signal 反查索引 |

## 共有约束

1. 旧 migration 只作为行为和数据语义基线，不直接复制为 Rust 表；Rust migration 必须
   重新评审列、约束、索引、owner 和 Lite 能力。
2. auction_commands.command_id 是 UUID 主键；command_kind、aggregate_id、request_hash、
   result_code、result_payload、created_at 都是账本事实。request_hash 固定 32 字节，
   result_payload 必须是 JSON object。
3. bids.command_id、bid_proxies.creation_command_id 及其他 command 外键不得改成可随意
   删除的弱引用。历史事实和幂等账本的删除顺序必须失败关闭。
4. realtime_events 只追加；修复事件通过 replay_of_event_id 引用旧事件，禁止编辑原事件。
   PostgreSQL 的 origin bid fact 唯一性只约束原始事件，不能阻止同一事实的运维 replay。
5. 任何标记为两者的对象都必须同时满足 PostgreSQL 并发语义和 SQLite Lite 的串行写事务、
   WAL/本地文件系统、锁等待及单 Worker 实例约束。
6. `lots.lot_number` 是正数、场次内唯一且不可变的内部顺序；`category_key/category_number`
   是正数、场次与分类内唯一且不可变的对外编号。分类键必须与对应 equipment category 一致，
   撤回或重挂不会复用已经分配的编号。
7. 迁移提交前必须为每张表补充 Rust 查询 owner、共有查询的 SQLx 离线元数据策略，以及
   PostgreSQL/SQLite 是否真的需要该对象的证据。
