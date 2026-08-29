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

当前已提交 PostgreSQL `000001` 至 `000019`，覆盖核心竞价、结算/履约、生命周期、
代理历史、身份/外部命令、A32 凭据兼容修正、恢复/可观测性和论坛同步对象；SQLite 已提交
`000001` 至 `000008` 作为共有能力子集及生命周期/领先时间/lot 结束时间修正。旧系统的 `bid_proxies` 不再作为第二份
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
显式写入和无法逐字段证明属于同一事实的行都不经过该兼容分支。

Auction-BF v28 到当前 schema 的转换由专用 `legacy_v28` bridge 持有，不修改旧 migration，也不
把 golang-migrate 版本号直接映射成 SQLx 版本。它只在精确旧库预检和外部备份证据通过后补齐
Rust 需要的生命周期列、lot 领先投影、逐 bid 截止事实、代理兼容表/视图及索引，并为等价覆盖的
SQLx `1..=5`、`7` 写入当前 checksum；其余迁移仍按本目录顺序向前执行。

## 基础身份、配置与审计

| 表 | 旧迁移 | 逻辑 owner | 档位 | 关键职责 |
| --- | --- | --- | --- | --- |
| worlds | 000001_baseline | auction-application：world | 两者 | 世界边界与启用状态 |
| users | 000001_baseline | auction-application：users | 两者 | 用户身份、活动状态和版本 |
| user_blacklist | 000001_baseline | auction-application：users | 两者 | 公开资格与封禁原因 |
| user_renames | 000004_identity_external_admin | auction-application：users | 两者 | 用户名变更追加历史 |
| public_sessions | 000001_baseline | auction-application：sessions | 两者 | Public 会话权威状态 |
| admin_sessions | 000001_baseline | auction-application：sessions | 两者 | Admin 会话与 CSRF 关联状态 |
| rules_documents | 000001_baseline | auction-application：rules | 两者 | 版本化规则文档；PostgreSQL 首次接管可由 auctionctl 经同一应用端口导入旧目录 |
| audit_records | 000001_baseline | auction-application：audit | 两者 | 追加式审计事实，不允许编辑历史 |
| service_heartbeats | 000001_baseline | auction-observability | 两者 | 进程/服务心跳与过期判断 |
| credential_keys | 000004_identity_external_admin + 000009_credential_admin_compat | auction-application：credentials | 完整档 | 加密后的外部凭据索引 |
| user_login_key_envelopes | 000004_identity_external_admin / 000003_user_admin | auction-application：users | 两者 | 用户登录 key 密文信封；只保存密文、nonce、key id 和指纹 |
| user_mail_commands | 000002_worker_external_adapters | auction-application：users | 完整档 | MoogleMail 注册/重绑命令幂等账本与结果 |
| user_game_identity_history | 000002_worker_external_adapters | auction-application：users | 完整档 | MoogleMail 换绑后的游戏身份追加历史 |
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
通过独立数据密钥读取 active 池，SQLite 不复制这些完整档表。

## 拍卖与竞价核心

| 表 | 旧迁移 | 逻辑 owner | 档位 | 关键职责 |
| --- | --- | --- | --- | --- |
| auctions | 000001_baseline + PostgreSQL 000003_lifecycle_proxy_history + 000014_lifecycle_database_invariants / SQLite 000004_auction_cancellation + 000005_lifecycle_metadata | auction-domain + auction-application：auction | 两者 | 拍卖生命周期、版本、时间点、前后轮关系和取消原因/操作者；完整档额外保护聚合身份与前序顺序 |
| equipment | 000001_baseline | auction-domain + auction-application：auction | 两者 | 标的事实与规范化装备属性 |
| auction_preview_items | 000001_baseline | auction-application：auction | 两者 | 预览阶段的物化展示项 |
| lots | 000001_baseline + PostgreSQL 000018 / SQLite 000008 | auction-domain + auction-application：auction | 两者 | 拍品状态、金额、领先者、锁定版本及 live/sold 有效结束时间 shape |
| auction_commands | 000001_baseline | auction-application：command-ledger | 两者 | 所有可变命令的幂等账本 |
| bids | 000001_baseline | auction-domain + auction-application：auction | 两者 | 追加式竞价事实 |
| bid_proxies | 000001_baseline | auction-storage：兼容视图 | 两者 | 旧代理字段的只读兼容投影，非权威状态 |
| bid_proxy_authorizations | 000025_auction_bidding_state_model | auction-domain + auction-application：auction | 两者 | Fast/Slow 代理授权状态机 |
| bid_proxy_authorization_transitions | 000025_auction_bidding_state_model | auction-domain + auction-application：auction | 两者 | 代理授权追加式迁移记录 |
| bid_proxy_outbid_uses | 000012_auction_policy_snapshot | auction-domain + auction-application：auction | 两者 | 代理出价次数和上限 |
| bid_proxy_attempts | 000024_proxy_bid_attempt_history | auction-domain + auction-application：auction | 两者 | 代理尝试历史与失败分类 |
| scheduled_rounds | 000013_auction_scheduled_rounds + 000006_worker_scheduler + PostgreSQL 000012_scheduled_final_phase / SQLite 000006_scheduled_final_phase | auction-application + auction-jobs：auction-lifecycle | 两者 | 持久化预览/开拍/Final phase 调度游标、租约、完成 fence、尝试次数和失败状态 |
| auction_intake_targets | 000020_auction_lifecycle_hardening + PostgreSQL 000014_lifecycle_database_invariants | auction-application：auction-intake | 完整档 | 世界级拍品录入目标，并与该世界唯一 draft 保持延迟一致性 |
| auction_submissions | 000002_worker_external_adapters | auction-application：auction-intake | 完整档 | `mail/admin` owner 闭集的拍品提交事实；Admin 的 URL/JSON 适配器种类只进入来源引用和命令摘要 |
| equipment_secrets | 000002_worker_external_adapters | auction-application：keymanagement | 完整档 | 标的敏感来源密文 |
| auction_schema_identity | 000001_baseline | auction-storage：migration | 两者 | 迁移/schema 身份和兼容检查 |

当前 Rust 迁移中的 `auctions.policy_snapshot` 由 PostgreSQL `000002_settlement_core` 建立，
保存创建拍卖时冻结的 schema v4 结算/恢复及竞价策略。它与 `default_start_price`、软截止、
取消截止和 slow 间隔等遗留规则列在同一创建事务中写入；配置更新不会回溯改变已创建拍卖。
竞价和取消事务在锁内读取该快照，恢复 PPM、代理开关/默认模式/次数上限和 final phase 参数。

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
| settlement_recovery_health | 000005_recovery_observability_forum | auction-application + auction-storage + auction-jobs | 完整档 | 恢复阻塞和健康投影；Worker 刷新，Admin 只读 |
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
| auction_forum_sync | 000005_recovery_observability_forum + 000010_forum_sync_fencing | auction-storage：forum sync；auction-integrations + auction-jobs：forum | 完整档 | 论坛主题发布状态、租约和 fencing 证明 |
| forum_sync_admin_commands | 000016_forum_sync_write_authorization | auction-application + auction-storage + auctionctl：forum recovery | 完整档 | 论坛未知写入重新授权的幂等命令账本、目标确认结果和追加审计关联 |
| forum_bidder_identities | 000005_recovery_observability_forum | auction-storage：forum reply；Admin 凭据/用户 owner 提供登记来源 | 完整档 | 论坛外部身份到用户身份映射 |
| auction_forum_reply_cursors | 000005_recovery_observability_forum | auction-storage：forum reply | 完整档 | 论坛回帖扫描游标 |
| auction_forum_bid_commands | 000005_recovery_observability_forum | auction-storage：forum reply；auction-jobs：forum reply | 完整档 | 论坛竞价导入命令账本与重试事实 |

A16-A19 使用 `auction-storage` 的邮件表做 PostgreSQL 完整档状态与审查投影；A20 的全局
Worker control 唤醒不写入额外触发表；
`000008_mooglemail_polling` 追加 `mail_poll_state` 的 lease/fencing 与失败字段，Worker 在同一
事务中提交 `inbound_mail`、`mail_commands` 和 cursor。应用层定义命令与查询端口，
`auction-jobs`/`auction-integrations` 负责远程读取；基础通知和结算 delivery 已进入 outbox
transport；结算 send 的 Sent/Balance 严格解析、exact remote ID 查找和自动 proof 对账已接入
PostgreSQL Worker，无法证明远端 ID 时进入 `manual_reconcile`。论坛主题写入未知时先按精确标题
只读对账；未命中必须由 `forum_sync_admin_commands` 的持久授权打开下一次 POST。SQLite Lite
不复制这些完整档表。

## 实时事件与运维告警

| 表 | 旧迁移 | 逻辑 owner | 档位 | 关键职责 |
| --- | --- | --- | --- | --- |
| realtime_streams | 000001_baseline + 000011_realtime_discovery_alias | auction-storage：realtime；auction-application：Public relay；auction-jobs：consumer | 两者 | 持久化 stream 目录；PostgreSQL 完整档包含 `auction:discovery` 派生流 |
| realtime_events | 000001_baseline + 000007_realtime_replay + 000011_realtime_discovery_alias | auction-storage：realtime；auction-application：Public relay；auction-jobs：consumer | 两者 | 追加式事件、stream sequence、replay 关系和 PostgreSQL `auction.created` 发现别名 |
| realtime_consumer_offsets | 000001_baseline | auction-application：Public relay；auction-jobs：外部 consumer | 两者 | 按 consumer/stream 保存的连续 durable cursor；Public relay 与外部 consumer 使用不同 consumer key |
| realtime_failures | 000001_baseline | auction-application：Public relay；auction-jobs + auction-observability：外部 realtime | 两者 | 事件消费隔离、失败事实和显式 replay 边界 |
| operations_alert_signals | 000005_recovery_observability_forum + 000015_operations_alert_state_invariants | auction-application + auction-storage：alert signal；auction-jobs：扫描 | 完整档 | 告警信号、稳定 identity、版本、通知时间和 cooldown |
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
6. 迁移提交前必须为每张表补充 Rust 查询 owner、共有查询的 SQLx 离线元数据策略，以及
   PostgreSQL/SQLite 是否真的需要该对象的证据。
