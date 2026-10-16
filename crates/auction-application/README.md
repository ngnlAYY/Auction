# auction-application

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

用例、窄端口和事务编排；具体 SQL、HTTP、配置与外部 I/O 由外层适配器实现。

## 本页导航

- [源码入口](#源码入口)
- [身份与凭据约束](#身份与凭据约束)
- [源码分组](#源码分组)
- [竞价与生命周期端口](#竞价与生命周期端口)
- [结算与恢复端口](#结算与恢复端口)
- [管理端口与服务](#管理端口与服务)
- [Public 读模型与装备入口](#public-读模型与装备入口)
- [论坛、审计与运维投影](#论坛审计与运维投影)
- [投递与邮件处理](#投递与邮件处理)
- [调度、告警与错误](#调度告警与错误)
- [读取、实时与会话](#读取实时与会话)
- [冻结策略与事务约束](#冻结策略与事务约束)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/ports](src/ports) | 命令、读模型、事务和外部能力端口 |
| [src/services](src/services) | 竞价、管理、会话、结算、恢复与邮件用例 |
| [src/errors](src/errors) | 错误分类与重试边界 |

## 身份与凭据约束

MoogleMail 注册/找回先读取用户 UUID 再签发绑定该身份的登录 key 信封；Users 写事务必须重新锁定并复核身份。
身份变化交给有界重试，新换绑必须由 active key 授权，已完成命令按账本重放。
`AdminCredentialRuntimeOwnership` 只暴露有界所有权检查，失效或取消后不恢复所有权，也不暴露数据库连接或凭据。

## 源码分组

`lib.rs` 只保留模块声明和公共导出。`ports/` 与 `services/` 按 `admin`、`auction`、`mail`、
`realtime`、`recovery`、`settlement` 等职责分组；接口表达领域意图，不传递 SQL 行或部署配置。

## 竞价与生命周期端口

[ports/core](src/ports/core) 定义事务、账本、仓储、竞价与时钟端口；
[services/auction/competition.rs](src/services/auction/competition.rs) 统一 Public 竞价和 Worker 慢速代理，
[services/auction/lifecycle](src/services/auction/lifecycle) 统一 Admin/Worker 生命周期。
取得权威锁后才读取命令时刻，同一计划共用该时间；Final 与调度完成标记原子提交。

基础 `AuctionService::create` 位于 [services/auction/commands.rs](src/services/auction/commands.rs)；
正式 Admin 创建由 [services/admin/auction.rs](src/services/admin/auction.rs) 的 `AdminAuctionCreateService` 拥有，
使用稳定命令、冻结策略和幂等重放，二者不能混作同一入口。
竞价成功响应在事务内冻结授权、quota、历史及终态；HTTP 不在提交后补读当前账户状态。
历史账本由 `competition/legacy_ledger.rs` 与 `slow_replay.rs` 严格校验，只读重放不重新竞价。

自动周排期来源由 [ports/scheduling/source.rs](src/ports/scheduling/source.rs) 冻结；后继按本地周及既有 DST 规则计算，
无来源的历史轮次保持 UTC 周期。人工提前开放/结算受默认关闭策略控制。
Admin/Worker 向生命周期服务注入启动时有效的 `AuctionPolicyTemplate`；新后继冻结该模板，不能复制前驱经济策略。
排期来源仍按既有轮次延续，旧快照和已接受命令不重写。

## 结算与恢复端口

[services/settlement](src/services/settlement) 拥有结算与履约用例；重放必须用冻结策略、资金池、lot 事实和义务验证原结果。
历史账本通过独立防腐入口重建只读结果，缺失证据不回退到写入。
[services/recovery](src/services/recovery) 统一 Admin/Worker 确认与版本门禁。
履约 intent、payload、稳定身份及摘要不得包含装备 secret；派发世界与装备来源世界分别建模。

## 管理端口与服务

[ports/admin](src/ports/admin) 与 [services/admin](src/services/admin) 按拍卖、lot、规则、Users、凭据、runtime 等 owner 分工。
命令携带幂等身份与版本/指纹门禁；数据库业务世界开关与 runtime 邮箱开关分开建模。
已实现能力见[能力矩阵](../../docs/architecture.md#部署档位与能力)。

Runtime 文件更新和恢复从开始到收口持有 `WorkflowGuard`；文件发布后只能向前恢复账本，
结果未知不得恢复旧文件。候选纯校验交给文件适配器，cron 语法仍由配置层拥有。
流程与重启语义见[运行配置契约](../../docs/contracts/routes.md#runtime-配置与文件事务)。

## Public 读模型与装备入口

[ports/auction/queries](src/ports/auction/queries) 定义拍卖、lot、竞价/价格历史及账户安全投影；
[ports/public](src/ports/public) 定义规则、黑名单、补偿与装备链接等读取端口。
[queries/countdown.rs](src/ports/auction/queries/countdown.rs) 定义论坛图片所需的公开时间快照；`AuctionReader` 读取时绑定公开可见性及数据库时钟，
不带装备或凭据，不把装饰性倒计时作为生命周期决定。
Public 指定列表的请求及结果页统一接受正 `i64` 数量、保留 offset 防护；不影响账户、Admin 和事件分页，范围见[分页契约](../../docs/contracts/routes.md#public-分页世界与金额)。
规则缓存只接收有效可重建快照，miss、损坏、过期或后端失败均回源。
完整装备 URL 仅能移交已授权的 HTTP DTO 或论坛渲染边界，不能进入日志、审计或缓存。
Admin 装备录入将来源读取、key 加密和持锁写入分成窄端口；来源/策略拒绝发生在写事务前。

## 论坛、审计与运维投影

[ports/forum](src/ports/forum) 区分公开镜像、外部同步与回帖命令；真实处理楼层不能由 post ID 或内容版本推算。
`ForumTopicPublication` 区分真实创建回执和找到既有主题；Admin 状态读取复用公开安全投影端口，不获得写入能力。
同步端口定义冻结的历史单页/新分页布局与新主题每帖 150 件的目录页大小、续页正文/身份与动态系统日志楼层；`ForumCatalogPage` 的调试正文始终脱敏。
`ForumReplyTopic` 持有有序续页证明，全部固定帖确认后才以同一版本推进处理水位。
`ForumCommandOutcome` 分开携带持久错误码及本地有界诊断；Worker 只能生成白名单英文提示，不能把诊断原文发往论坛。
未知论坛写入只能先只读对账，再按持久人工授权重试；固定帖正文比较统一由同步端口定义，
兼容范围与确认门禁见[外部写入不变量](../../docs/contracts/invariants.md#i-08-外部写入发生在本地提交之后)。
Admin 审计、邮件、实时和 dashboard 端口只返回有界安全投影；最近事件元数据不含 payload。
路由、分页及字段闭集见[HTTP 契约](../../docs/contracts/routes.md)。

## 投递与邮件处理

[ports/delivery](src/ports/delivery) 定义 claim、发送与结果端口；transport 可收紧进程内并发上限，
让背压发生在领取和进入 `sending` 之前。
[ports/mail](src/ports/mail) 与 [services/mail](src/services/mail) 拥有轮询、用户命令、装备入站及 delivery outbox 编排。
处理前检查持久化完成证明，原接受决定可恢复，不能因取件后资产变化改判退件。
`MailPollStore::fail_after_prefix` 要求同一 fenced 事务提交连续成功前缀、游标和失败退避；不支持的适配器明确拒绝。

仅 `Retryable` 错误与超时进入有界重试；业务拒绝与确定性错误直接收口。
原始邮件/响应只进入独立加密诊断；Admin 脱敏展示与装备失败 sidecar 均不参与命令摘要、完成证明或 cursor，
具体见[邮件诊断](../../ops/mail-diagnostics.md)。装备来源时间在每次异步读取完成后重新校验，不加宽限。

## 调度、告警与错误

[ports/scheduling](src/ports/scheduling) 只表达冻结轮次与下一 UTC 触发时刻；cron 解析在配置层，
定时器不能替代持久化 due、游标和租约。[ports/operations](src/ports/operations) 定义安全水位、告警评估与批次状态，
告警不改变拍卖或外部投递事实。[errors](src/errors) 保留领域分类和稳定可重试性。

## 读取、实时与会话

[services/sessions](src/services/sessions) 将 Public 认证版本与会话创建锁内复核衔接；key 撤销/重签永久废止旧会话。
长连接通过只读 `inspect` 检查用户、凭据与双重过期边界，不滑动刷新。
Admin 服务启动前撤销旧 Admin 会话，失败阻断启动；token 摘要绑定当前 actor/secret。
账号/口令校验由会话 owner 导出，首次设置与敏感配置复用相同边界，拒绝不可登录的配置。

[ports/realtime](src/ports/realtime) 定义事件、连续游标、消费所有权和重放边界；事件用于通知与恢复，
不能代替权威快照。细节见[实时契约](../../docs/contracts/events.md)。

## 冻结策略与事务约束

工作区依赖向内指向 `auction-domain`；不得读取进程配置、打开监听、执行 SQL 或直接调用外部服务。
可变业务命令使用稳定 `command_id` 与 32 字节 SHA-256 请求摘要；相同命令重放原结果，不同负载冲突。
`TransactionPolicy::serializable()` 最多尝试三次（含首次），`read_committed()` 一次，适配器不能混淆两者。

新拍卖及后继草稿校验新模板限制，历史读取按冻结策略；Admin 创建同时保持策略快照与兼容规则列一致。
完整锁顺序、写入集合及历史兼容边界见[领域不变量](../../docs/contracts/invariants.md)。

## 维护与验证

```bash
cargo check --locked -p auction-application
```

仅静态编译；默认检查与禁止默认运行的测试范围见[开发指南](../../docs/development.md)，
运行证据见[验证状态](../../docs/status.md)。修改公共行为时同步对应契约。
