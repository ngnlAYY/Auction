# auction-application

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

用例、窄端口和事务编排；具体 SQL、HTTP、配置与外部 I/O 由外层适配器实现。本 crate 不读进程配置、不开监听、
不执行 SQL 或直接调用外部服务，依赖方向向内指向 `auction-domain`。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/ports](crates/auction-application/src/ports) | 命令、读模型、事务和外部能力端口 |
| [src/services](crates/auction-application/src/services) | 竞价、管理、会话、结算、恢复与邮件用例 |
| [src/errors](crates/auction-application/src/errors) | 错误分类与重试边界 |
| [src/draft_soft_close](crates/auction-application/src/draft_soft_close) | 显式未发布草稿软截止修正规则、检查结果和窄端口 |

`lib.rs` 只保留模块声明和公共导出；`ports/` 与 `services/` 按 `admin`、`auction`、`mail`、`realtime`、`recovery`、
`settlement` 等职责分组，接口表达领域意图，不传递 SQL 行或部署配置。

## 身份与凭据约束

MoogleMail 注册/找回先读取用户 UUID 再签发绑定该身份的登录 key 信封，Users 写事务必须重新锁定并复核身份；
身份变化交给有界重试，新换绑必须由 active key 授权，已完成命令按账本重放。注册/换绑使用本地处理时钟，不以偏快的
邮件接收时间抬高用户创建时间；装备投稿的既有接收时间门禁不变。`AdminCredentialRuntimeOwnership` 暴露有界所有权
检查、可选全局 `mail_cookie` generation 与永久撤销，失效或取消后不恢复所有权，也不暴露数据库连接或凭据。
[credentials/identity.rs](crates/auction-application/src/ports/admin/credentials/identity.rs) 保留预期完整名称与旧
`VerifiedMailPlayerIdentity` 的兼容形状；[forum/profile.rs](crates/auction-application/src/ports/forum/profile.rs) 只按
HV UID 读取论坛目标资料，预期名称为 `None` 仍自动远端核验。
[settlement/evidence](crates/auction-application/src/ports/settlement/evidence) 拥有完整 UID/凭据版本/generation/world
绑定、精确 EID 正向存在及 Main cash observation；`prepare_internal_evidence_with_sent` 在同一上下文追加精确 Sent 领取
观察，纳入完整摘要、时间门禁与同 session 锁序。opaque prepared 保留同 session/独占 guard，提交作用域未确定结束就
取消或销毁会 sticky 撤销 owner；请求 purpose 区分结算 claim、投递 claim、资金运维命令，同一绝对预算最多 30 秒。

## 竞价与生命周期端口

[ports/core](crates/auction-application/src/ports/core) 定义事务、账本、仓储、竞价与时钟端口；
[services/auction/competition.rs](crates/auction-application/src/services/auction/competition.rs) 统一 Public 竞价和
Worker 慢速代理，[services/auction/lifecycle](crates/auction-application/src/services/auction/lifecycle) 统一
Admin/Worker 生命周期；取得权威锁后才读取命令时刻，同一计划共用该时间，Final 与调度完成标记原子提交。
基础 `AuctionService::create` 位于 [services/auction/commands.rs](crates/auction-application/src/services/auction/commands.rs)，
正式 Admin 创建由 [services/admin/auction.rs](crates/auction-application/src/services/admin/auction.rs) 的
`AdminAuctionCreateService` 拥有（稳定命令、冻结策略、幂等重放），二者不能混作同一入口。竞价成功响应在事务内冻结
授权、quota、历史及终态，HTTP 不在提交后补读当前账户状态；历史账本由 `competition/legacy_ledger.rs` 与
`slow_replay.rs` 严格校验。自动周排期来源由
[ports/scheduling/source.rs](crates/auction-application/src/ports/scheduling/source.rs) 冻结，后继按本地周及既有 DST
规则计算，无来源的历史轮次保持 UTC 周期；Admin/Worker 向生命周期服务注入启动时有效的 `AuctionPolicyTemplate`，
新后继冻结该模板，旧快照和已接受命令不重写。`draft_soft_close` 只对显式指定、从未发布的合格草稿修正 60/60 为
900/900，默认检查只读，执行需稳定命令、版本及精确确认（门禁见[CLI](apps/auctionctl/README.md#草稿软截止显式修正)）。

装备标记由 [ports/public/watch.rs](crates/auction-application/src/ports/public/watch.rs) 与
[services/public/watch.rs](crates/auction-application/src/services/public/watch.rs) 拥有：稳定命令摘要包含会话用户，
Serializable 锁内重放/CAS，同值新命令仍推进 fence；超价通知身份按 lot/用户/领先周期确定，冻结正文只含公开标识和
已接受价格，外部发送不在竞价事务内执行。账号渠道由
[ports/public/watch_preference.rs](crates/auction-application/src/ports/public/watch_preference.rs) 与
[services/public/preferences](crates/auction-application/src/services/public/preferences) 拥有，只锁 user、账本先于 CAS、
旧 watch.channel=mm 不变。[services/public/watch_notification.rs](crates/auction-application/src/services/public/watch_notification.rs)
生成 PM 冻结内容/缺收件人事实；[ports/lot_watch_pm](crates/auction-application/src/ports/lot_watch_pm) 定义持久任务端口；
[ports/forum/pm.rs](crates/auction-application/src/ports/forum/pm.rs) 定义只读 prepare、一次性 prepared.send 与原 sender
Sent 对账，不能重寄未知写入。

## 结算与恢复端口

Recovery 每封退件由 storage 在同一事务追加唯一消费事实、增加计数并建立回收动作，重寄不释放原消费身份。
重复使用的 action 按来源 MID 区分新的 take prepare/activate，同轮重试保持原命令；
`MailDeliveryEnqueuer::find_recovery_preparation` 只读恢复旧版 prepare 已提交而 action 未关联的窗口，完整来源、
摘要和 payload 冲突失败关闭。旧 prepared/enqueued 不改写命令或外部结果。

[services/settlement](crates/auction-application/src/services/settlement) 拥有结算与履约用例，重放必须用冻结策略、
资金池、lot 事实和义务验证原结果；历史账本通过独立防腐入口重建只读结果，缺失证据不回退到写入。
履约激活继续沿用同一 claim 的 token/fencing，租约存在期间不写 `next_attempt_at`；紧接的
reconcile 释放 claim 时原子写入下次追踪时间。COD、carrier 与卖家货款对齐共享五分钟窗口，
其它装备寄送沿用普通重试间隔，保持发送 owner 与履约 owner 分离。
[services/recovery](crates/auction-application/src/services/recovery) 统一 Admin/Worker 确认与版本门禁。履约 intent、
payload、稳定身份及摘要不得包含装备 secret，派发世界与装备来源世界分别建模；`SettlementFulfillmentBotIdentityReader`
只读当前 active Cookie/world 的 `VerifiedMailPlayerIdentity`，无预期绑定也远端核验，无法证明时返回错误。

[ports/settlement/internal](crates/auction-application/src/ports/settlement/internal) 与履约服务的 `internal.rs`、
`collected.rs`、`refund.rs` 拥有新 pending 的真实身份分支、内部满足、外部 COD 资金化及原合法退款用途授权：
`with_internal_fulfillment` 注入数据库窄端口和同一 opaque evidence reader，缺装配/证据/资金明确持久延期；
`prepare_next` 返回独立 `InternallySatisfied`（无 delivery ID），原 prepare/activate/reconcile 与旧 claim 字段保留。
只处理未准备且无旧资产冲突的新 pending；原稳定命令先重放，guard 跨完整事务及有界重试，未知提交撤销 owner、不换 ID。
退款仅原 `equipment_recovered` 门激活的新 pending；外部 COD 使用同 session with_sent 完整快照，在原外部满足后、
直接依赖解锁前调用资金 owner。B 及实际 C 转原 seller S/受限 T，retained 不自动再投资；Isekai 付款前及 handoff 各自
重核同 UID 真实库存，现金仍在 Main。[存储原子事务](crates/auction-storage/README.md#内部履约原子事务设计)独立保存
`bot_internal_v1` 证明，不伪造 Sent。

### 资金对账与用途端口

[ports/settlement/funding.rs](crates/auction-application/src/ports/settlement/funding.rs) 与
[services/settlement/funding](crates/auction-application/src/services/settlement/funding) 拥有独立资金端口及对账用例：
preview 只读数据库、不读远端身份/余额、不输出自由额度；apply 要求稳定命令、actor、预览摘要/CAS、明确的自有与补偿
用途额度及逐字确认，再取得真实同 session 证据，提交 guard 保持到确定提交/回滚，未知结果撤销 owner 并按原命令核对。
`FundingCashGateProvider` 默认无能力，只有完整现金发送门接线后才能 apply；补偿仅资金化已冻结需求，旧无 UID 责任
只能隔离扣占；新 COD 另需当前同 session 精确收款与 UID 绑定，不得从旧 collected 或总余额生成第二份现金来源。

UID 账户另有显式 `record_only` 策略，由独立命令初始化缺失账户并纳入预览摘要及事务版本；默认 `strict_cash` 沿用上述现金对账。
仅记账模式保留真实身份、干净 compose、正向库存及精确 Sent 证据，省略账号余额采集和额度比较；按每笔冻结 B/C 建立唯一有限的
会计来源，允许所有人自行使用 Credits。账本仍保护原义务、用途、未知投递和 B+C=S+T，不把记账来源当作现金证明。

### 旧结算显式核对

[ports/settlement/legacy_repair](crates/auction-application/src/ports/settlement/legacy_repair) 为旧自寄提供单目标、
默认只读的修复端口。写入绑定稳定命令、完整预览摘要、原版本、操作者与原因，使用同一 opaque guard 下的真实
UID、干净 compose 和精确装备库存；流拍装备可按事实留存，明确自寄拒绝后的成交付款仅恢复内部准备。
原 delivery、发送时间、错误及全部指针快照形成只追加历史桥，后续内部付款必须沿用已核 UID，不降级外发。

[ports/settlement/funding/legacy.rs](crates/auction-application/src/ports/settlement/funding/legacy.rs) 为旧实收的
Bot 卖家款提供独立责任认领：同账号精确 Sent-collected、冻结金额和账户策略通过后，将已隔离责任转入原标的
卖家用途再留存。命令与目标均唯一，不重复发行旧 B，不把留存收入增加为自有可消费额度。
两条路径均由 `auctionctl legacy-settlement` 装配，预览不取得凭据 owner；执行遵守[停机修复流程](ops/README.md#旧-bot-结算的显式核对)。

## 管理、Public 读模型与运维投影

[ports/admin](crates/auction-application/src/ports/admin) 与 [services/admin](crates/auction-application/src/services/admin)
按拍卖、lot、规则、Users、凭据、runtime 等 owner 分工，命令携带幂等身份与版本/指纹门禁，数据库业务世界开关与
runtime 邮箱开关分开建模（能力见[能力矩阵](docs/design/architecture.md#部署档位与能力)）。Runtime 文件更新和恢复
从开始到收口持有 `WorkflowGuard`，文件发布后只能向前恢复账本，结果未知不得恢复旧文件（见
[运行配置契约](docs/contracts/routes.md#runtime-配置与文件事务)）。

装备标色规则的 `categories`、`equipment_types`、`qualities` 保存同一条规则的多选条件：同维任一匹配，
维度间同时满足；每维空数组沿用旧单值字段，两者均空则不过滤。同维新旧字段同时非空、重复或未规范化条目
均拒绝。空数组不序列化，旧文档指纹和命令摘要保持原义；标色不改变收件品质门槛、装备事实或价格。
新 `equipment_types` 数组中的十二种护甲材质族同时匹配其所属实际大类和完整装备种类的首词/部位边界，
其余选项精确匹配完整种类。旧 `equipment_type` 单值始终保持精确匹配，不继承材质族语义。
材质归类和后缀 `the ` 兼容比较复用 `auction-domain::equipment::taxonomy`，应用层只拥有规则命中语义，
不再维护分类副本；词典扩展不改变现有收件允许列表、绑定要求、等级门槛或旧标色单值含义。
[ports/forum/colors.rs](crates/auction-application/src/ports/forum/colors.rs) 的 `equipment_color_decision` 同时拥有
论坛渲染方案及公开页 `EquipmentGroup`：实际会上色为 `rare`，其余为 `fusion`；Peerless 固定彩虹属于稀有。
首条规则只命中单边词缀时，名称中找不到对应片段仍按论坛纯文本回退归为融合；等级 0 与论坛快照一致视为未知。
Public lot 查询允许装备分组与类型组合筛选，应用投影为预览、正式摘要和详情携带同一分组，页面验证复核筛选归属。

[ports/auction/queries](crates/auction-application/src/ports/auction/queries) 定义拍卖、lot、竞价/价格历史及账户安全投影，
[ports/public](crates/auction-application/src/ports/public) 定义规则、黑名单、补偿与装备链接等读取端口；
[queries/countdown.rs](crates/auction-application/src/ports/auction/queries/countdown.rs) 定义论坛图片所需的公开时间快照，
`AuctionReader` 读取时绑定公开可见性及数据库时钟。Public 指定列表接受正 `i64` 数量并保留 offset 防护
（见[分页契约](docs/contracts/routes.md#public-分页世界与金额)）；规则缓存只接收有效可重建快照，miss、损坏、过期或
后端失败均回源；完整装备 URL 仅能移交已授权的 HTTP DTO 或论坛渲染边界。Admin 装备录入把来源读取、key 加密和持锁
写入分成窄端口，来源/策略拒绝发生在写事务前。
成交价格历史的 `PublicPriceHistorySort` 限定时间、价格的升降序，旧 cursor 仅支持默认时间倒序；
名称按 Unicode 空白或英文逗号拆成最多八个词条，各词按字面包含并以 AND 组合，不要求相邻或依输入顺序出现；
完整筛选集统计的可选 `median_exact` 用规范十进制字符串保存整数或半个 Credit，应用与 HTTP 共同校验
安全范围、最低/最高价区间及旧整数中位数的一致性，不通过浮点重算金额。

[ports/forum](crates/auction-application/src/ports/forum) 区分公开镜像、外部同步与回帖命令，真实处理楼层不能由 post ID
或内容版本推算；`ForumTopicPublication` 区分真实创建回执与找到既有主题，Admin 状态读取复用公开安全投影端口而不获得
写入能力。同步端口冻结历史单页/新分页布局与新主题每帖 150 件的目录页大小、续页正文/身份与系统日志楼层，
`ForumCatalogPage` 的调试正文始终脱敏；终态完成版本由 Worker 在全固定帖证明后填入 claim 的
`confirmed_settled_version`，storage 必须锁内复核。`ForumReplyTopic` 持有有序续页证明，全部固定帖确认后才以同一版本推进处理水位；`ForumAuctionSnapshot.settled` 只代表
权威结算状态及齐备明细，`ForumLotSettlement` 携带连续领先起点与持久 adjustment_ppm。`ForumCommandOutcome` 分开携带
持久错误码及本地有界诊断，Worker
只能生成白名单英文提示；未知论坛写入只能先只读对账，再按持久人工授权重试
（见[外部写入不变量](docs/contracts/invariants.md#i-08-外部写入发生在本地提交之后)）。审计、邮件、实时和 dashboard
端口只返回有界安全投影。

## 投递、邮件与调度

[ports/delivery](crates/auction-application/src/ports/delivery) 定义 claim、发送与结果端口，transport 可收紧进程内并发
上限，让背压发生在领取和进入 `sending` 之前。`DeliveryTransport::prepare` 只作只读前置，返回持有同一凭据独占上下文的
消耗式 `DeliveryPreparedSend`，持久 fence 成功并确认续租后才 poll 写入；`prepare_authorized` 返回
`AuthorizedDeliverySend`，现金证据绑定真实 request_hash/token/fencing，经 `start_sending_authorized_until` 借用同
session 快照并收紧至有效租约/提交预算，默认实现明确 `CapabilityUnavailable`。仅期限前确定 `Sending` 才允许发送，
`Deferred` 是确定未写入/回滚或已提交冻结，Err/超时/取消撤销原 owner。`DeliveryWorkStore::renew` 严格匹配
token/fencing 和数据库实时租约，过期不得复活；release 只接受有效 claimed，fence 提交未知保留持久隔离。

[ports/mail](crates/auction-application/src/ports/mail) 与 [services/mail](crates/auction-application/src/services/mail)
拥有轮询、用户命令、装备入站及 delivery outbox 编排：处理前检查持久化完成证明，原接受决定可恢复，不能因取件后
资产变化改判退件；`MailPollStore::fail_after_prefix` 要求同一 fenced 事务提交连续成功前缀、游标和失败退避。
`MailPollReadJournal` 是详情 GET 前的窄持久队列端口，只传列表身份与当前 claim。读取与登记均核对 mailbox/world、token、
owner、版本和锁后数据库实时租约；详情读取前先提交整窗摘要，后续只随已完成前缀的入站事务删除，不把正文或凭据交给队列。
仅 `Retryable` 错误与超时进入有界重试；原始邮件/响应只进入独立加密诊断，不参与命令摘要、完成证明或 cursor
（见[邮件诊断](ops/mail/diagnostics.md)）。
`ports/mail/equipment_diagnostics/reason.rs` 拥有来源解析的九项闭集诊断码，进程内观察使用枚举，
只有 parse/protocol_drift 组合可投影到原有 sidecar 的 `detail_code`；不接受原始来源错误文本，
不参与业务决定、命令摘要或历史记录重写。

[ports/scheduling](crates/auction-application/src/ports/scheduling) 只表达冻结轮次与下一 UTC 触发时刻（cron 解析在配置层），
定时器不能替代持久化 due、游标和租约；[ports/operations](crates/auction-application/src/ports/operations) 定义安全水位、
告警评估与批次状态，告警不改变拍卖或外部投递事实；[errors](crates/auction-application/src/errors) 保留领域分类和可重试性。

## 读取、实时与会话

[services/sessions](crates/auction-application/src/services/sessions) 将 Public 认证版本与会话创建锁内复核衔接，
key 撤销/重签永久废止旧会话；长连接通过只读 `inspect` 检查用户、凭据与双重过期边界，不滑动刷新。Admin 服务启动前
撤销旧 Admin 会话，失败阻断启动，token 摘要绑定当前 actor/secret；账号/口令校验由会话 owner 导出，首次设置与敏感配置
复用相同边界。[ports/realtime](crates/auction-application/src/ports/realtime) 定义事件、连续游标、消费所有权和重放边界，
事件不能代替权威快照（见[实时契约](docs/contracts/events.md)）。

## 冻结策略与事务约束

可变业务命令使用稳定 `command_id` 与 32 字节 SHA-256 请求摘要，相同命令重放原结果、不同负载冲突；
`TransactionPolicy::serializable()` 最多尝试三次（含首次），`read_committed()` 一次，适配器不能混淆两者。新拍卖及后继
草稿校验新模板限制，历史读取按冻结策略，Admin 创建保持策略快照与兼容规则列一致；完整锁顺序与历史兼容边界见
[领域不变量](docs/contracts/invariants.md)。

## 维护与验证

```bash
cargo check --locked -p auction-application
```

仅静态编译；默认检查与测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)；
修改公共行为时同步对应契约。
