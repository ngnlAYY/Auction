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

当前已提交 PostgreSQL `000001` 至 `000007`，覆盖核心竞价、结算/履约、生命周期、
代理历史、身份/外部命令、恢复/可观测性和论坛同步对象；SQLite 已提交
`000001_lite_core.up.sql` 作为共有能力子集。旧系统的 `bid_proxies` 不再作为第二份
权威表维护，PostgreSQL 与 SQLite 均以只读兼容视图投影新授权模型；写入只进入
`bid_proxy_authorizations` 及其历史表。完整适配器、Lite 串行写入和其他业务表面仍在迁移中。

核心竞价切片额外把竞价规则快照字段放在 `auctions`，把 `leader_proxy_last`、慢速代理
调度字段和状态版本放在 `lots`；`auction-storage` 的 PostgreSQL 事务适配器只读取并更新
这些权威字段，不在内存中另存一份竞价状态。

## 基础身份、配置与审计

| 表 | 旧迁移 | 逻辑 owner | 档位 | 关键职责 |
| --- | --- | --- | --- | --- |
| worlds | 000001_baseline | auction-application：world | 两者 | 世界边界与启用状态 |
| users | 000001_baseline | auction-application：users | 两者 | 用户身份、活动状态和版本 |
| user_blacklist | 000001_baseline | auction-application：users | 两者 | 公开资格与封禁原因 |
| user_renames | 000004_identity_external_admin | auction-application：users | 两者 | 用户名变更追加历史 |
| public_sessions | 000001_baseline | auction-application：sessions | 两者 | Public 会话权威状态 |
| admin_sessions | 000001_baseline | auction-application：sessions | 两者 | Admin 会话与 CSRF 关联状态 |
| rules_documents | 000001_baseline | auction-application：rules | 两者 | 版本化规则文档 |
| audit_records | 000001_baseline | auction-application：audit | 两者 | 追加式审计事实，不允许编辑历史 |
| service_heartbeats | 000001_baseline | auction-observability | 两者 | 进程/服务心跳与过期判断 |
| credential_keys | 000001_baseline | auction-application：keymanagement | 完整档 | 加密后的外部凭据索引 |
| user_login_key_envelopes | 000002_worker_external_adapters | auction-application：sessions | 完整档 | 用户登录凭据密文封装 |
| user_game_identity_history | 000002_worker_external_adapters | auction-application：users | 完整档 | 游戏身份变更历史 |
| user_blacklist_history | 000019_user_blacklist_history | auction-application：users | 两者 | 封禁变更追加历史 |
| user_admin_commands | 000008_user_admin | auction-application：users | 完整档 | Admin 用户命令幂等账本 |
| credential_admin_commands | 000007_credential_admin | auction-application：keymanagement | 完整档 | 凭据管理命令幂等账本 |
| runtime_config_admin_commands | 000004_identity_external_admin | auction-application：runtime-config | 完整档 | runtime 配置更新、指纹和 pending/applied/unknown 结果 |
| rules_admin_commands | 000003_rules_admin_commands | auction-application：rules | 完整档 | 规则文档管理命令幂等账本 |

## 拍卖与竞价核心

| 表 | 旧迁移 | 逻辑 owner | 档位 | 关键职责 |
| --- | --- | --- | --- | --- |
| auctions | 000001_baseline | auction-domain + auction-application：auction | 两者 | 拍卖生命周期、版本和时间点 |
| equipment | 000001_baseline | auction-domain + auction-application：auction | 两者 | 标的事实与规范化装备属性 |
| auction_preview_items | 000001_baseline | auction-application：auction | 两者 | 预览阶段的物化展示项 |
| lots | 000001_baseline | auction-domain + auction-application：auction | 两者 | 拍品状态、金额、领先者和锁定版本 |
| auction_commands | 000001_baseline | auction-application：command-ledger | 两者 | 所有可变命令的幂等账本 |
| bids | 000001_baseline | auction-domain + auction-application：auction | 两者 | 追加式竞价事实 |
| bid_proxies | 000001_baseline | auction-storage：兼容视图 | 两者 | 旧代理字段的只读兼容投影，非权威状态 |
| bid_proxy_authorizations | 000025_auction_bidding_state_model | auction-domain + auction-application：auction | 两者 | Fast/Slow 代理授权状态机 |
| bid_proxy_authorization_transitions | 000025_auction_bidding_state_model | auction-domain + auction-application：auction | 两者 | 代理授权追加式迁移记录 |
| bid_proxy_outbid_uses | 000012_auction_policy_snapshot | auction-domain + auction-application：auction | 两者 | 代理出价次数和上限 |
| bid_proxy_attempts | 000024_proxy_bid_attempt_history | auction-domain + auction-application：auction | 两者 | 代理尝试历史与失败分类 |
| scheduled_rounds | 000013_auction_scheduled_rounds + 000006_worker_scheduler | auction-application + auction-jobs：auction-lifecycle | 两者 | 持久化预览/开拍调度游标、租约、尝试次数和失败状态 |
| auction_intake_targets | 000020_auction_lifecycle_hardening | auction-application：auction-intake | 完整档 | 世界级拍品录入目标 |
| auction_submissions | 000002_worker_external_adapters | auction-application：auction-intake | 完整档 | 邮件/后台拍品提交事实 |
| equipment_secrets | 000002_worker_external_adapters | auction-application：keymanagement | 完整档 | 标的敏感来源密文 |
| auction_schema_identity | 000001_baseline | auction-storage：migration | 两者 | 迁移/schema 身份和兼容检查 |

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
| settlement_recoveries | 000016_settlement_recovery_operations | auction-application：settlement-recovery | 完整档 | 付款/交付异常恢复聚合 |
| settlement_recovery_actions | 000016_settlement_recovery_operations | auction-application：settlement-recovery | 完整档 | 恢复动作及 durable claim |
| settlement_recovery_commands | 000016_settlement_recovery_operations | auction-application：settlement-recovery | 完整档 | 恢复确认命令幂等账本 |
| settlement_recovery_health | 000016_settlement_recovery_operations | auction-observability | 完整档 | 恢复阻塞和健康投影 |
| deliveries | 000001_baseline | auction-application + auction-jobs：delivery | 完整档 | 外部投递 outbox、claim 和重试 |
| delivery_admin_commands | 000005_delivery_admin_commands | auction-application：delivery | 完整档 | Admin 交付命令幂等账本 |

## 邮件、论坛与外部集成

| 表 | 旧迁移 | 逻辑 owner | 档位 | 关键职责 |
| --- | --- | --- | --- | --- |
| inbound_mail | 000001_baseline | auction-storage：mail projection | 完整档 | 入站邮件原始事实索引的安全读取来源 |
| mail_commands | 000001_baseline | auction-integrations + auction-jobs：mail | 完整档 | 邮件发送 outbox/命令 |
| mail_poll_state | 000001_baseline | auction-storage：mail state | 完整档 | MoogleMail 轮询游标和状态 |
| user_mail_commands | 000002_worker_external_adapters | auction-integrations + auction-jobs：mail | 完整档 | 用户邮件外部写入幂等账本 |
| mail_admin_commands | 000004_mail_admin_commands | auction-storage：mail admin command ledger | 完整档 | Admin 邮件控制命令幂等账本 |
| auction_forum_sync | 000026_auction_forum_sync | auction-integrations + auction-jobs：forum | 完整档 | 论坛主题发布同步状态 |
| forum_bidder_identities | 000028_forum_bidder_identities | auction-integrations：forum | 完整档 | 论坛用户名到用户身份映射 |
| auction_forum_reply_cursors | 000028_forum_bidder_identities | auction-jobs：forum | 完整档 | 论坛回帖扫描游标 |
| auction_forum_bid_commands | 000028_forum_bidder_identities | auction-integrations + auction-jobs：forum | 完整档 | 论坛竞价导入命令 outbox |

A16-A19 只使用 `auction-storage` 的三个邮件表做 PostgreSQL 完整档状态/审查投影；应用层
定义命令与查询端口，Worker/MoogleMail 的远程轮询触发、发送 outbox 和回执仍分别由
`auction-jobs`/`auction-integrations` 的后续迁移负责。SQLite Lite 不复制这些完整档表。

## 实时事件与运维告警

| 表 | 旧迁移 | 逻辑 owner | 档位 | 关键职责 |
| --- | --- | --- | --- | --- |
| realtime_streams | 000001_baseline | auction-contracts + auction-jobs：realtime | 两者 | 持久化 stream 目录 |
| realtime_events | 000001_baseline + 000007_realtime_replay | auction-contracts + auction-jobs：realtime | 两者 | 追加式事件、stream sequence 和 replay 关系 |
| realtime_consumer_offsets | 000001_baseline | auction-jobs：realtime | 两者 | Worker 按 consumer/stream 保存的连续 durable cursor |
| realtime_failures | 000001_baseline | auction-jobs + auction-observability：realtime | 两者 | 事件消费隔离、失败事实和显式 replay 边界 |
| operations_alert_signals | 000017_observability_alerting | auction-observability + auction-jobs | 完整档 | 告警信号和下次尝试时间 |
| operations_alert_batches | 000017_observability_alerting | auction-observability + auction-jobs | 完整档 | 告警批次、租约和重试 |
| operations_alert_batch_items | 000017_observability_alerting | auction-observability + auction-jobs | 完整档 | 告警批次成员与指纹 |

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
