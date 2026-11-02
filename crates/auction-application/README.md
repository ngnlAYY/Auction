# auction-application

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [业务规则](../../docs/business-rules.md)

用例层：定义窄端口（trait）、命令与安全投影，编排领域规则与事务边界。它不认识 PostgreSQL、HTTP、MoogleMail 或 E-Hentai 的具体实现——这些由 `auction-storage`、`auction-http`、`auction-integrations`、`auction-jobs` 作为适配器实现端口，`apps/` 中各程序按角色在装配根组合所需能力。

依赖：`auction-domain` 与少量通用库（serde、sha2/hmac、chrono-tz、url 等），不依赖 tokio 运行时。

## 结构

| 路径 | 内容 |
| --- | --- |
| [`crates/auction-application/src/ports/`](src/ports) | 端口与值对象：每个功能 owner 一个子模块，由 [`crates/auction-application/src/lib.rs`](src/lib.rs) 统一导出 |
| [`crates/auction-application/src/services/`](src/services) | 用例服务：组合端口完成一个业务动作 |
| [`crates/auction-application/src/errors/`](src/errors) | `ApplicationError` 及各 owner 的低基数错误 |

### 端口分组

| 分组 | 主要内容 |
| --- | --- |
| [`crates/auction-application/src/ports/core/`](src/ports/core) | `Clock`、`CommandLedger`/`RequestHash`/`StoredCommand`（幂等账本）、事务策略、仓储错误、稳定编码（小写十六进制、旧版 Go JSON 兼容摘要） |
| [`crates/auction-application/src/ports/auction/`](src/ports/auction) | 公开查询、竞价命令、生命周期、慢速代理到期、冻结的策略模板 |
| [`crates/auction-application/src/ports/scheduling/`](src/ports/scheduling) | 周排期、赛季早鸟日历、IANA 时区来源、Worker 排期 bootstrap |
| [`crates/auction-application/src/ports/public/`](src/ports/public) | 规则、黑名单、补偿池、装备链接、会话与登录时长偏好、装备标记与提醒偏好、读模型缓存 |
| [`crates/auction-application/src/ports/admin/`](src/ports/admin) | 拍卖创建/排期/列表、lot 修改、装备录入、论坛预览、邮件状态、规则、runtime 配置、凭据、用户、世界启停、审计、改名历史、结算目录 |
| [`crates/auction-application/src/ports/mail/`](src/ports/mail) | 轮询持久化、业务处理、注册/换绑、诊断证据、装备读取诊断、邮箱维护、展示投影 |
| [`crates/auction-application/src/ports/delivery/`](src/ports/delivery) | 投递 outbox Worker、发送节奏分组、Send/Take 期望冻结、Admin 投递操作 |
| [`crates/auction-application/src/ports/settlement/`](src/ports/settlement) | 结算命令、Worker 调度、履约、联合远端证据、资金、内部履约、报表 |
| [`crates/auction-application/src/ports/recovery/`](src/ports/recovery) | 付款超时恢复的调度、命令、健康与报表 |
| [`crates/auction-application/src/ports/forum/`](src/ports/forum) | 论坛主题同步、回帖导入、Public 镜像、装备标色规则、论坛私信、目标资料 |
| [`crates/auction-application/src/ports/lot_watch_pm/`](src/ports/lot_watch_pm) | 装备超价提醒私信的独立队列与人工对账 |
| [`crates/auction-application/src/ports/realtime/`](src/ports/realtime) | Public 回放、Worker 消费、Admin 运维 |
| [`crates/auction-application/src/ports/operations/`](src/ports/operations) | 运维告警、进程 readiness、Worker control 状态 |

### 主要服务

| 服务 | 职责 |
| --- | --- |
| `CompetitionService` | 出价、创建/下调代理、慢速代理到期：锁内取快照 → 领域规划 → 原子写入事实、审计与 realtime |
| `AuctionLifecycleService` | Admin 与 Worker 共用的预览/开拍/Final/取消/提前结算；开拍时按排期日历计算后继场次 |
| `SettlementService` | 认领到期拍卖、计算 `SettlementPlan`、生成义务，结算事实一次提交；兼容重放旧版两阶段账本 |
| `SettlementFulfillmentService` | 履约 prepare → activate → reconcile；Bot 参与时走内部履约分支 |
| `MailProcessorService` | 按邮件主题路由：`auction` 投稿、`register`、`rebind`、结算退回邮件、其他邮件归档或退回 |
| `MailboxMaintenanceService` | 周期观察 compose 草稿、受控丢弃与 Sent 观察 |
| `RecoveryHealthService` / `RecoveryCommandService` | 余额与积压健康、Admin 恢复确认 |
| `RealtimeRelay` | Public 进程把持久化事件流转发到 WebSocket Hub |
| `PublicSessionService` / `AdminSessionService` | 会话签发与校验 |
| Admin 系列服务 | 凭据、用户、规则、runtime 配置、录入、世界、排期等后台命令 |
| `SettlementEquipmentLossPort` | 显式人工装备损失 preview/resolve；稳定命令绑定 actor/reason、UID、recovery/payment/delivery 版本和冻结卖家金额，安全投影不输出原载荷；外部买方补偿声明不等于完成回执，详见[数据库契约](../../docs/contracts/database.md#结算资金恢复与投递) |
| `DeliverySentConfirmationPort` | 正式 CLI 的人工 Sent preview/confirm；v1 观察绑定冻结正文摘要、身份、分钟时窗、金额与实际资产，已收取资产和旧尝试歧义分别显式接受；独立 operator proof 只恢复正常对账，详见[数据库契约](../../docs/contracts/database.md#结算资金恢复与投递) |

## 横切约定

- **幂等账本**：竞价、生命周期及其他要求幂等重放的业务命令携带稳定 `command_id` 与 `RequestHash`（命令种类 + 聚合 ID + 请求 DTO 的 SHA-256）。同 ID 同摘要重放原结果，同 ID 不同摘要返回冲突。通用请求 JSON 直接流入摘要，不保留完整临时缓冲；邮件附件直接借用逐项序列化，避免再分配完整摘要投影列表，序列化字段顺序与字节编码保持不变。兼容旧系统的摘要统一经 `RequestHash::from_legacy_go_json` 等入口计算；登录、会话校验等端口使用各自的会话契约。
- **事务边界**：一个用例的状态变化、命令结果、追加式审计与 realtime 事件在同一事务提交；端口要求的隔离级别与重试（40001/40P01 有界重试）由存储实现负责。
- **竞价策略**：新建场次模板冻结统一加价与代理上限出价的 v2 策略；既有场次继续原版本。普通出价的事实 ID 数量与领域反击结果一致；慢速命令账本也接受有完整终态证据的耗尽结果。快照形状见[数据库契约](../../docs/contracts/database.md#拍卖与竞价)，金额规则见[业务规则](../../docs/business-rules.md#代理竞价)。
- **时间**：通过 `Clock` 注入；领域规划不读取系统时间。排期按本地日历偏移，偏移量或日期溢出返回 `InvalidSchedule`，不因构造时长而 panic。
- **赛季**：赛季前场次冻结 v2 恢复策略与 24 小时付款期限；配置按本地分钟先校验，具体场次物化和 Admin 改期共用 UTC 付款边界，要求实际计划结束加 24 小时严格早于换季日本地 00:00，防止 DST 前跳或改期突破预算。读取已有冻结来源仍沿用原解析语义，已冻结策略与命令结果不随新模板改写。早鸟查询逐场物化并在首个命中后停止，当前无效候选仍返回错误。同封投稿按冻结的世界与收件时间只选择一次品质列表，附件仍按原顺序逐件读取和验证。
- **取消与未知提交**：外写只能在持久 sending fence 后开始；内部证据的提交 guard、发送 fence 的取消 guard 在未确认提交或回滚时同步撤销原 owner。未知 Send 保留人工核对，Sent 未命中不能授权重发；已经接受的投稿优先恢复持久事实，不能因重试时远端附件变化改判退回。
- **Sent 收件人证明**：`mail_recipient_is_list_projection(list, full)` 只用于列表候选筛选，借用非空前缀并忽略 ASCII 大小写，允许列表末尾 U+FFFD、`…` 或 `...` 截断标记；空值和纯标记不匹配。`MoogleMailboxSentState.recipient` 必须来自详情完整收件人；冻结发送、历史结算和通知在记录证据或恢复前都逐字核对该完整身份（仅忽略 ASCII 大小写），不能以共享前缀完成证明。原正文、凭据、金额、附件和唯一性门禁保留；自动恢复时间窗从原 `sending_at` 所在分钟的起点（`sent_list_minute_start`）到 `outcome_recorded_at`；同分钟更早的同内容邮件由详情唯一匹配、MID 绑定唯一和存储侧整分钟歧义检查排除。
- **敏感值**：Cookie、装备 key、登录 key 等经专用值对象受控暴露，并对明文调试输出与丢弃清理设定边界；普通查询投影不含秘密。显式的凭据命令、Admin key 签发结果与投递端口可携带专用秘密类型，不能把这些类型直接映射为普通查询 DTO。
- **错误**：`ApplicationError` 区分能力缺失（`CapabilityUnavailable`，HTTP 层通常映射为 `501 feature_not_supported`）、领域错误、仓储错误与各 owner 的闭集错误；错误文本不回显输入中的秘密。

## 会话

- **Public**：玩家用登录 key 登录，原始 token 只在登录响应中出现一次，存储端只保存摘要。登录时长由账号偏好决定（默认永久，可设固定期限），活动不延长期限；每个用户的并发会话数受 `max_public_sessions` 限制。
- **Admin**：HTTP Basic 风格的 actor/secret 登录，会话与 CSRF token 以 HMAC 摘要保存，空闲与绝对过期滑动计算。服务启动时永久撤销既有 Admin 会话——重启 Admin 后须重新登录，配置凭据改回旧值也不能复活旧 token。

## 维护约定

- 新功能先定义端口与值对象，再在适配器 crate 中实现；不要在本 crate 引入 I/O 依赖。
- 新增命令必须定义稳定的命令种类字符串与请求摘要输入，一经发布不可修改。
- 投影与 DTO 的映射放在 `auction-http`，不要让端口类型直接成为对外协议。
