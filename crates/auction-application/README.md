# auction-application

[docs/README.md](../../docs/README.md) · [docs/architecture.md](../../docs/architecture.md) · [crates/auction-domain/README.md](../auction-domain/README.md)

用例、窄端口与事务编排层。只依赖 `auction-domain` 与基础库，不读进程配置、不监听、不执行 SQL 或外部调用；存储、HTTP、集成与 Worker 适配器实现本 crate 的端口。

## 结构

| 路径 | 内容 |
| --- | --- |
| [crates/auction-application/src/ports](src/ports) | 端口与值对象：每个功能 owner 一个子模块，由 [crates/auction-application/src/lib.rs](src/lib.rs) 统一导出 |
| [crates/auction-application/src/services](src/services) | 用例服务：组合端口完成一个业务动作 |
| [crates/auction-application/src/errors](src/errors) | `ApplicationError` 及各 owner 的低基数错误 |

### 上下文公开面

`ports/` 与 `services/` 都按限界上下文分目录，`lib.rs` 把两者合并成与上下文同名的公开模块（如 `admin`、`auction`、`core`、`mail`、`settlement`）：只有端口或只有用例的上下文直接导出原模块，两侧都有的由 `merged_contexts!` 合并。公开项只在各上下文 `mod.rs` 的 `pub use` 清单登记一次。装配根和适配器必须以 `<上下文>::<类型>` 路径引用，不得走 `ports::`/`services::` 扁平路径。只有端口的上下文（`lot_watch`、`observability` 等）在 `ports/` 内按需再归组子模块，`lib.rs` 仍以原名字导出（`lot_watch_pm`、`sli` 等），对外路径不变；`src` 下的 `#[path]` 由计数棘轮 `all:#[path]` 阻断。

### 端口分组

| 分组 | 主要内容 |
| --- | --- |
| [crates/auction-application/src/ports/core](src/ports/core) | `Clock`、`RequestHash`/`StoredCommand`（幂等账本）、事务策略与按场次写队列、仓储错误、稳定编码、有界分页窗口、CLI 操作者身份 |
| [crates/auction-application/src/ports/auction](src/ports/auction) | 公开查询、竞价命令、生命周期、慢速代理到期、冻结的策略模板 |
| [crates/auction-application/src/ports/public](src/ports/public) · [admin](src/ports/admin) | 公开端规则/黑名单/补偿池/会话；Admin 拍卖与 lot 管理、装备录入、邮件状态、规则、runtime 配置、凭据、用户、世界、审计 |
| [crates/auction-application/src/ports/mail](src/ports/mail) · [delivery](src/ports/delivery) | 邮件轮询/处理/维护与诊断；投递 outbox、发送节奏、Admin 投递证据与动作守卫 |
| [crates/auction-application/src/ports/settlement](src/ports/settlement) · [recovery](src/ports/recovery) · [forum](src/ports/forum) | 结算命令与履约、资金证据、报表；付款超时恢复；论坛同步、回帖导入、私信 |
| [crates/auction-application/src/ports/observability](src/ports/observability) | SLI 快照、影子运行共享契约、Sent 发送基线、通知与外发指标闭集 |
| [crates/auction-application/src/ports/operations](src/ports/operations) · [realtime](src/ports/realtime) · [lot_watch](src/ports/lot_watch) | 运维告警与 readiness、Worker control 端口；实时回放/消费；提醒私信与关注列表提醒 |

### 主要服务

| 服务 | 职责 |
| --- | --- |
| `CompetitionService` / `AuctionLifecycleService` | 出价、代理与生命周期：锁内快照 → 领域规划 → 原子写入事实、审计与 realtime |
| `SettlementService` / `SettlementFulfillmentService` | 认领到期场次、计算结算计划与义务；履约 prepare → activate → reconcile |
| `MailProcessorService` / `MailboxMaintenanceService` | 按主题路由邮件；周期观察草稿、受控丢弃与 Sent 观察 |
| `RealtimeRelay` / 会话服务 | 持久事件转发到 WebSocket Hub；Public/Admin 会话签发与校验 |
| Admin 系列服务 | 凭据、用户、规则、runtime 配置、录入、世界与排期命令；通用前置条件在 `services/admin/common.rs` 的 `valid_command_envelope` |
| `SettlementEquipmentLossPort` / `DeliverySentConfirmationPort` | 人工装备损失与人工 Sent 的 preview/confirm；冻结命令、身份与时窗，契约见[docs/contracts/database.md#结算资金恢复与投递](../../docs/contracts/database.md#结算资金恢复与投递) |

## 横切约定

- **谓词与分页**：文本与正十进制校验来自 `auction_domain::text`；有界分页窗口用 `ports::core::valid_page_window`。应用层组合谓词做命令级校验，不复制长度或字符规则。
- **幂等账本**：业务命令携带稳定 `command_id` 与 `RequestHash`（命令种类 + 聚合 ID + 请求 DTO 的 SHA-256）。同 ID 同摘要重放原结果，同 ID 不同摘要冲突；兼容旧系统的摘要经 `RequestHash::from_legacy_go_json` 等入口。
- **事务边界**：状态变化、命令结果、追加式审计与 realtime 事件同事务提交；隔离级别与 40001/40P01 有界重试由存储实现。会写 `lots`/`auctions` 的场次级命令在进入 Serializable 事务前经 `run_in_auction_lot_queue`（仅知 lot）或 `run_in_auction_queue`（已知场次）排队，预算用尽返回 `QueueWaitTimeout`。设计见[docs/contracts/database.md#按场次写队列竞价--生命周期--admin](../../docs/contracts/database.md#按场次写队列竞价--生命周期--admin)。
- **竞价与赛季策略**：新建场次模板冻结统一加价、代理上限与恢复策略；既有场次沿用冻结版本。金额规则见[docs/business-rules.md#代理竞价](../../docs/business-rules.md#代理竞价)。
- **时间**：通过 `Clock` 注入，领域规划不读系统时间；排期按本地日历偏移，溢出返回错误而不 panic。
- **取消与未知提交**：外部写入只在持久 sending fence 后开始；未确认提交时同步撤销 guard。未知 Send 保留人工核对，Sent 未命中不能授权重发。
- **敏感值**：Cookie、装备 key、登录 key 经专用值对象受控暴露；普通查询投影不含秘密，不能把秘密类型映射为查询 DTO。
- **错误**：`ApplicationError` 区分能力缺失（HTTP 通常映射 `501 feature_not_supported`）、领域、仓储与各 owner 闭集错误；错误文本不回显秘密。诊断用 `safe_kind()` 的闭集分类以[safe_kind.rs](src/errors/safe_kind.rs) 的实现为唯一事实源，不参与重试或业务判定；`FundingError`/`InternalEvidenceError` 的分类同理。

## 错误诊断

类型化安全错误由 jobs/application 与 integrations/HTTP 共同覆盖；诊断入口 `diagnostics::record_error` 只接收 `SafeErrorKind`、固定级别、kind/phase 与 `DiagnosticContext`，不读取 Display/Debug/source 或负载文本。500、内部不变量与持久化处置失败用 ERROR，依赖、超时与未确认结果用 WARN；未持久化处置用 `DiagnosticDisposition::Unpersisted`。完整规则见[crates/auction-observability/README.md#未持久化错误](../auction-observability/README.md#未持久化错误)、[crates/auction-integrations/README.md#安全错误记录](../auction-integrations/README.md#安全错误记录)与[ops/README.md#安全错误与未持久化指标](../../ops/README.md#安全错误与未持久化指标)。

COD 实收的 reconcile 位置分类以 `services/settlement/fulfillment/collected.rs` 的固定 `safe_kind` 与 `stage` 为唯一事实源；收据拒绝码与前置存储阶段见[crates/auction-storage/README.md#cod-实收资金化拒绝诊断](../auction-storage/README.md#cod-实收资金化拒绝诊断)。

## 会话与开关

- **Public**：登录 key 登录，原始 token 只在响应中出现一次；会话数受 `max_public_sessions`（默认 10）限制，满额时在同一事务内注销最久未活动会话而不是拒绝登录。
- **Admin**：actor/secret 登录，会话与 CSRF token 以 HMAC 摘要保存，空闲与绝对过期滑动计算；启动时撤销既有 Admin 会话。
- **运行时开关**：`RuntimeSwitchName`/`RuntimeSwitchCommand` 绑定类型与角色，共用稳定摘要与 CAS/到期规则；`RuntimeSwitches` 保存不可变快照与 TOML 基线。清单与回退见[docs/contracts/database.md#运行时行为开关](../../docs/contracts/database.md#运行时行为开关)与[ops/README.md#运行时开关](../../ops/README.md#运行时开关)。

## 维护约定

- 新功能先定义端口与值对象，再在适配器 crate 实现；本 crate 不引入 I/O 依赖。
- 新增命令必须定义稳定的命令种类字符串与请求摘要输入，一经发布不可修改。
- 投影与 DTO 映射放 `auction-http`；端口类型不直接成为对外协议。
- `ExternalWriteLifecycle` 是控制进程内取消的窄端口，不决定持久租约、完成状态或重发资格；登记不授权外写，仍须确认围栏提交。
