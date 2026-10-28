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

[ports/forum/public.rs](crates/auction-application/src/ports/forum/public.rs) 共同拥有 P15 与 P30 的内容证明和公开链接校验。
`PublicForumLink` 只允许经校验构造，字段私有；复用工作区 `url` 做纯 HTTPS 解析，不进行网络访问。
P30 对历史未对账记录省略链接，对声明有内容证明但数据损坏的记录失败关闭，见[v2 拍卖历史契约](docs/contracts/routes.md#p30-v2-拍卖历史与论坛地址)。

[ports/delivery/cadence.rs](crates/auction-application/src/ports/delivery/cadence.rs) 拥有七类 Send 间隔及提交后唤醒窄端口；
间隔为 0–86400 秒、默认全零，非权威通知只加速领取，持久起点及发送资格仍归存储事务。runtime 同组 managed 控制接管，
分组和兼容语义见[发送间隔](ops/config/README.md#邮件业务发送间隔)，不改变金额、命令身份或已发送结果。

[ports/delivery/expectation.rs](crates/auction-application/src/ports/delivery/expectation.rs) 定义实际 Send 内容与 Take 会话的安全冻结身份；
`AuthorizedDeliverySend` 通过 `DeliveryPreparedExpectation` 区分两类单层 `Arc`，sending fence 取得身份后才可消费同一会话的 future。
Send 的主题、规范化正文摘要、精确金额/EID 与可选凭据版本绑定同事务持久化，不存正文、登录 key 或 Cookie；
Take 的 `DeliveryTakeExpectation` 冻结 world、MID、原凭据版本及 owner generation，资产继续来自同 command/hash 的不可变 payload。
无对应冻结身份不得授权 Send/Take；现金 fence 只接受 Send。发送前确定的非法负载经 `reject_preparation` 直接失败，
不伪造 sending 或释放资金占用。维护不从当前模板/key 或当前账号猜测旧动作；Return 不新增自动恢复。

## 身份与凭据约束

[credentials/acquisition.rs](crates/auction-application/src/ports/admin/credentials/acquisition.rs) 定义互斥的粘贴 Cookie /
论坛账号来源与获取窄端口；[services/admin/credentials](crates/auction-application/src/services/admin/credentials) 先核对命令账本，
再交集成适配器获取并密封 Cookie，网络期间不持有数据库事务。密码不持久化，账号来源只使用带密钥、分域的输入摘要；
原 Cookie 来源保持历史摘要形状，最终密文、安全结果与审计仍由原存储事务原子提交，单 active 不变量不变。

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

## 卖方全额结算

新拍卖模板默认冻结 `hold_curve_v2/version=2`，拒绝以 v1 新建；历史快照、命令与金额不回写。
结算用例在原 Serializable 事务中按锁内奖池及全场名义需求同比缩放优惠，再同事务保存成功账本、义务、审计及持久化任务；
零预算仍正常结算，不能让卖方承担差额。v2 只保留基础优惠，九变量账单展示其实分金额，不追加另一份优惠或重复抵扣。
规则与旧版本边界见[结算不变量](docs/contracts/invariants.md#i-07a-结算策略与命令重放必须可从权威事实重建)。

## 竞价与生命周期端口

[ports/core](crates/auction-application/src/ports/core) 定义事务、账本、仓储、竞价与时钟端口；
[services/auction/competition.rs](crates/auction-application/src/services/auction/competition.rs) 统一 Public 竞价和
Worker 慢速代理，[services/auction/lifecycle](crates/auction-application/src/services/auction/lifecycle) 统一
Admin/Worker 生命周期；取得权威锁后才读取命令时刻，同一计划共用该时间，Preview/Open/Final 与调度完成标记原子提交。
竞价、生命周期与 Admin 创建服务不接受进程时钟参数；统一由事务在全部业务锁齐备后提供实时钟，不保留兼容占位泛型。
基础 `AuctionService::create` 位于 [services/auction/commands.rs](crates/auction-application/src/services/auction/commands.rs)，
正式 Admin 创建由 [services/admin/auction.rs](crates/auction-application/src/services/admin/auction.rs) 的
`AdminAuctionCreateService` 拥有（稳定命令、冻结策略、幂等重放），二者不能混作同一入口。竞价成功响应在事务内冻结
授权、quota、历史及终态，HTTP 不在提交后补读当前账户状态；历史账本由 `competition/legacy_ledger.rs` 与
`slow_replay.rs` 严格校验。自动周排期来源由
[ports/scheduling/source.rs](crates/auction-application/src/ports/scheduling/source.rs) 冻结，后继按本地周及既有 DST
规则计算，无来源的历史轮次保持 UTC 周期；Admin/Worker 向生命周期服务注入启动时有效的 `AuctionPolicyTemplate`，
新后继冻结该模板，旧快照和已接受命令不重写。`draft_soft_close` 只对显式指定、从未发布的合格草稿修正 60/60 为
900/900，默认检查只读，执行需稳定命令、版本及精确确认（门禁见[CLI](apps/auctionctl/README.md#草稿软截止显式修正)）。
Admin 创建与改期统一要求 `preview < open < end`；两小时只是邮箱暂停提前量，不作为已登记轮次的最小预览窗口。
首次 Admin 创建先按 world → intake target 取得原有业务锁，再读取数据库实时钟并校验预览计划；等锁跨过预览时间
必须拒绝，创建时间也取锁后时刻。已接受命令在这组锁与时间门禁之前重放，不受当前世界状态或新模板影响。
竞价资格最多涉及出价人和当前代理两个用户，使用栈内有序去重集合维持原 UUID 加锁顺序；
资格冻结至多生成一个前置计划，仍先冻结再应用竞价，不改变锁后计时及事务原子性。
`CompetitionTransaction::apply_plan` 与 `apply_slow_proxy_plan` 只读借用计划，用例返回同一份计划，不再为响应逐笔复制。

新拍卖的 recovery 快照显式冻结 `cod_return_limit`，创建、自动 bootstrap 和后继共用同一模板序列化；
旧快照缺字段时仍按三次处理，不回写已有场次或已接受结果。普通出价的事实 ID 数量与领域规划共同使用有效
`bid_ceiling()`；历史超额 Slow 代理在 1B 同价边界立即守位，不依赖之后的慢速任务纠正领先者。
普通命令可选的第二条事实必须是确定性第二条 ID、Proxy 机制及另一位出价者，合法同价反击保持；
新编码与历史重放共用此绑定校验，不按当前状态重新计算历史结果。
旧竞价账本借用已解码结果构造计划与响应，不复制完整代理历史；时间编码原地裁剪 RFC3339 小数尾零，wire 字节不变。
Recovery 复用 `RequestHash::from_legacy_go_json` 的固定字段序与转义规则；连续片段直接进入摘要，
不另建完整 canonical 缓冲或维护第二套字符串编码器。

装备标记由 [ports/public/watch.rs](crates/auction-application/src/ports/public/watch.rs) 与
[services/public/watch.rs](crates/auction-application/src/services/public/watch.rs) 拥有：稳定命令摘要包含会话用户，
Serializable 锁内重放/CAS，同值新命令仍推进 fence；超价通知身份按 lot/用户/领先周期确定，冻结正文只含公开标识和
已接受价格。MM 正文使用固定分段英文，提示到网站查看最新价格并继续出价；正文仍参与原请求摘要，旧通知沿用已冻结负载，
不增加 runtime 模板键。外部发送不在竞价事务内执行。账号渠道由
[ports/public/watch_preference.rs](crates/auction-application/src/ports/public/watch_preference.rs) 与
[services/public/preferences](crates/auction-application/src/services/public/preferences) 拥有，只锁 user、账本先于 CAS、
旧 watch.channel=mm 不变。[services/public/watch_notification.rs](crates/auction-application/src/services/public/watch_notification.rs)
生成 PM 冻结内容/缺收件人事实；[ports/lot_watch_pm](crates/auction-application/src/ports/lot_watch_pm) 定义持久任务端口；
[ports/forum/pm.rs](crates/auction-application/src/ports/forum/pm.rs) 定义只读 prepare、一次性 prepared.send 与原 sender
Sent 对账，不能重寄未知写入。

## 跨层共用规则

以下规则只在本 crate 声明一份，配置、存储与外部适配器调用同一实现，不再各自维护副本：

- [runtime_config/mail_templates.rs](crates/auction-application/src/ports/admin/runtime_config/mail_templates.rs) 拥有 14 个
  MoogleMail 通知模板的字段闭集、默认英文正文与占位符规则；Admin runtime 文档、Worker TOML 与 Worker 启动快照共用
  `AdminRuntimeMailTemplatesConfig::validate`，错误消息稳定且不回显正文。
- [core/encoding.rs](crates/auction-application/src/ports/core/encoding.rs) 的 `lower_hex` 统一摘要、指纹与人工确认短语的
  固定两位小写十六进制编码，一次分配、逐字节结果不变。
- `valid_notification_body`（冻结通知摘要）、`valid_allowlist_host`/`valid_allowlist_path_prefix`（装备来源 allowlist）与
  `valid_listener_group`（Unix 监听属组）分别供入队/存储重放/发送前校验及部署配置复用。
- 闭集枚举名称与规范文本不变量归 [auction-domain](crates/auction-domain/README.md)，本 crate 只调用不复制。

## 结算与恢复端口

Recovery 每封退件由 storage 在同一事务追加唯一消费事实、增加计数并建立回收动作，重寄不释放原消费身份。
重复使用的 action 按来源 MID 区分新的 take prepare/activate，同轮重试保持原命令；
`MailDeliveryEnqueuer::find_recovery_preparation` 只读恢复旧版 prepare 已提交而 action 未关联的窗口，完整来源、
摘要和 payload 冲突失败关闭。旧 prepared/enqueued 不改写命令或外部结果。

[services/settlement](crates/auction-application/src/services/settlement) 拥有结算与履约用例，重放必须用冻结策略、
资金池、lot 事实和义务验证原结果；历史账本通过独立防腐入口重建只读结果，缺失证据不回退到写入。
首次结算在计算前校验锁内 workflow、领取命令和截止世代，并核对从拍卖与截止派生的稳定身份及完成请求摘要；
新版账本重放也复核这些身份和摘要。派生与摘要由应用层统一提供给 storage/Worker，字节沿用旧 Worker 算法；
历史已结算记录可缺少这些字段，旧 `settlement_complete` 仍走原有只读防腐与权威事实对账。
旧 Go `settlement_complete` 摘要时间按 `RFC3339Nano` 去除小数尾零；同时只读兼容早期 Rust `AutoSi` 补零摘要，
不改写旧账本或重新结算。内部证据摘要仍使用原小写十六进制字节，固定 32 字节摘要只分配一次编码缓冲。
新结算与新旧账本重放均按 lot 建立借用索引，避免明细、事实及代理关闭快照逐项全表扫描；
义务构造不再复制整份计划，仍按锁内事实顺序生成稳定自然键，并完整校验重复 lot、金额总计和前置关系。
旧结算义务统计借用闭集种类名及账本字段，不为每条义务重新分配字符串；计数溢出、重复种类与完整重放对账仍严格拒绝。
履约激活继续沿用同一 claim 的 token/fencing，租约存在期间不写 `next_attempt_at`；紧接的
reconcile 释放 claim 时原子写入下次追踪时间。COD、carrier 与卖家货款对齐共享五分钟窗口，
其它装备寄送沿用普通重试间隔，保持发送 owner 与履约 owner 分离。
[services/recovery](crates/auction-application/src/services/recovery) 统一 Admin/Worker 确认与版本门禁。履约 intent、
payload、稳定身份及摘要不得包含装备 secret，派发世界与装备来源世界分别建模；`SettlementFulfillmentBotIdentityReader`
只读当前 active Cookie/world 的 `VerifiedMailPlayerIdentity`，无预期绑定也远端核验，无法证明时返回错误。
买家装备 COD intent 从 claim 的对应 `SettlementLine` 冻结价格变量；调整后价格取买家应付与奖池抵扣之和，不使用现行策略
重算。精确旧三/八变量的 prepared/enqueued 快照沿用原 canonical 字节与摘要；显式 v2 九变量形状及兼容边界见
[交付载荷契约](docs/contracts/schema.md#结算恢复与交付)。
履约 intent 在栈内编码一次规范小写 UUID，统一核对所有义务的 reference；不通过宽松 UUID 解析接受其他拼写或改变 canonical 摘要。

[ports/settlement/internal](crates/auction-application/src/ports/settlement/internal) 与履约服务的 `internal.rs`、
`collected.rs`、`refund.rs` 拥有新 pending 的真实身份分支、内部满足、外部 COD 资金化及原合法退款用途授权：
`with_internal_fulfillment` 注入数据库窄端口和同一 opaque evidence reader，缺装配/证据/资金明确持久延期；
`prepare_next` 返回独立 `InternallySatisfied`（无 delivery ID），原 prepare/activate/reconcile 与旧 claim 字段保留。
只处理未准备且无旧资产冲突的新 pending；原稳定命令先重放，guard 跨完整事务及有界重试，未知提交撤销 owner、不换 ID。
非法 claim、命令构造或 canonical intent 在新 pending 路径持久化为 `manual_reconcile`，不依赖租约过期反复重领。
enqueued 对账只复核收件用户 UUID；原地址取 delivery 快照，改名不重定向已发送邮件，也不推翻精确实收证明。
退款仅原 `equipment_recovered` 门激活的新 pending；外部 COD 使用同 session with_sent 完整快照，在原外部满足后、
直接依赖解锁前调用资金 owner。B 及实际 C 转原 seller S/受限 T，retained 不自动再投资；Isekai 付款前及 handoff 各自
重核同 UID 真实库存，现金仍在 Main。[存储原子事务](crates/auction-storage/README.md#内部履约原子事务设计)独立保存
`bot_internal_v1` 证明，不伪造 Sent。
外部退款的 prepare/activate（包括旧 prepared）同样借用不可复制的 Armory 联合证据，
沿准备时的单调截止持有 commit guard 至事务确定结果；无账户 legacy 不再以公开装备详情替代库存证明。

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
`AdminRuntimeMailDeliveryBudget` 只携带 Worker 非敏感启用/读取/投递预算；候选校验复用同一谓词，
不在应用层读取部署文件，也不以新规则改写历史 durable 命令。
规则候选、发布命令和存储快照共用借用校验，不再为检查大小而复制整篇 Markdown；错误优先级与 1 MiB 上限不变。
runtime 名单与过滤词条按原输入顺序使用借用集合判重，保持大小写、错误优先级与配置指纹。
Admin 重签 key 直接移交密文信封，并在等待存储前销毁本地候选明文；响应仍来自已提交账本。

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
Public lot 查询允许装备分组、等级上限、无等级显示与类型组合筛选；`PublicLotListRequest::matches_level` 拥有包含式上限
及无等级独立判断，不能把 None 当零级。应用投影为预览、正式摘要和详情携带同一分组，列表验证复核全部筛选归属。
正式 lot 详情仅由摘要装备投影持有一份属性 JSON；应用校验对象形状，HTTP 消费时移交原值，列表仍不输出属性。

[ports/auction/queries](crates/auction-application/src/ports/auction/queries) 定义拍卖、lot、竞价/价格历史及账户安全投影，
[ports/public](crates/auction-application/src/ports/public) 定义规则、黑名单、补偿与装备链接等读取端口；
[queries/countdown.rs](crates/auction-application/src/ports/auction/queries/countdown.rs) 定义论坛图片所需的公开时间快照，
`AuctionReader` 读取时绑定公开可见性及数据库时钟。Public 指定列表接受正 `i64` 数量并保留 offset 防护
（见[分页契约](docs/contracts/routes.md#public-分页世界与金额)）；规则缓存只接收有效可重建快照，miss、损坏、过期或
后端失败均回源；GET 或损坏快照删除失败后，本次读取不再回填，避免权威读取成功后重复消耗故障缓存等待预算。
规则读取与发布失效不互锁：旧数据库快照可能在发布后的成功失效之后回填，旧内容最多保留至该次回填的 TTL 到期；
不能将发布后失效视为立即一致性保证。
完整装备 URL 仅能移交已授权的 HTTP DTO 或论坛渲染边界。Admin 装备录入把来源读取、key 加密和持锁写入分成窄端口，
来源/策略拒绝发生在写事务前。
成交价格历史的 `PublicPriceHistorySort` 限定时间、价格的升降序，旧 cursor 仅支持默认时间倒序；
名称按 Unicode 空白或英文逗号拆成最多八个词条，各词按字面包含并以 AND 组合，不要求相邻或依输入顺序出现；
完整筛选集统计的可选 `median_exact` 用规范十进制字符串保存整数或半个 Credit，应用与 HTTP 共同校验
安全范围、最低/最高价区间及旧整数中位数的一致性，不通过浮点重算金额。
Admin 改名历史分页校验对整个请求只进行一次 Unicode 小写转换；记录匹配仍按原有小写子串语义判断。
Admin 论坛预览的应用投影保留原解析字段供授权调用方使用；`Debug` 只输出长度、数量及数值标记，
不输出任何来源文本、玩家名、post ID 或可能包含装备 key 的 URL。

[ports/forum](crates/auction-application/src/ports/forum) 区分公开镜像、外部同步与回帖命令，真实处理楼层不能由 post ID
或内容版本推算；`ForumTopicPublication` 区分真实创建回执与找到既有主题，Admin 状态读取复用公开安全投影端口而不获得
写入能力。同步端口冻结历史单页/新分页布局与新主题每帖 150 件的目录页大小、续页正文/身份与系统日志楼层，
`ForumCatalogPage` 的调试正文始终脱敏；终态完成版本由 Worker 在全固定帖证明后填入 claim 的
`confirmed_terminal_version`，storage 必须锁内复核 settled/cancelled 及同一版本。`ForumReplyTopic` 持有有序续页证明，
全部固定帖确认后才以同一版本推进处理水位；`ForumAuctionSnapshot.state` 使用领域 `AuctionStatus`，来自同一数据库快照，
不由截止时间推断；Settled 仍要求完整权威结算证明，`ForumLotSettlement` 携带连续领先起点与持久 adjustment_ppm。
`ForumRenderedThread.reconciliation_titles` 最多携带二十二个去重的旧版/其他状态精确标题，不重复当前标题；
`ForumPublisher::find_topic_by_titles` 将当前标题与候选合并为最多二十三个精确标题，一次版面读取核对唯一主题；
未知写入不会逐标题重复 GET，也不能隐式创建新主题。
`ForumReadPost` 从远端编辑表单携带原始正文及首帖标题/说明，调试正文脱敏，未知写入不能用本地标题替代证明。
`ForumCommandOutcome` 分开携带持久错误码及本地有界诊断，Worker 只能生成白名单英文提示；
未知论坛写入只能先只读对账，再按持久人工授权重试
（见[外部写入不变量](docs/contracts/invariants.md#i-08-外部写入发生在本地提交之后)）。审计、邮件、实时和 dashboard
端口只返回有界安全投影。
`ForumWriteProgress` 贯通创建、回复和编辑，只按实际写入进度分类本轮超时；历史未知仍保留对账门禁。
调度端口接收当前可用世界，适配器在 LIMIT 前过滤；稳定帖子身份、原业务负载与可变楼层投影分离，
详细约束见[论坛不变量](docs/contracts/invariants.md#i-08-外部写入发生在本地提交之后)。

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
`MailPollReadJournal` 是详情 GET 前的窄持久队列端口，只传定位、列表快照与当前 claim。列表发件人仅为可能截断的
展示快照，完整业务身份与 allowlist 只采信详情；未获授权的待读项保留待核对，不推进游标或执行业务。
读取与登记均核对 mailbox/world、token、owner、版本和锁后数据库实时租约；详情读取前先提交整窗摘要，
后续只随已完成前缀的入站事务删除，不把正文或凭据交给队列。
仅 `Retryable` 错误与超时进入有界重试；原始邮件/响应只进入独立加密诊断，不参与命令摘要、完成证明或 cursor
（见[邮件诊断](ops/mail/diagnostics.md)）。
邮件附件和投稿项以借用 ID 集合去重，保持输入顺序、数量上限与原错误优先级，不复制附件正文或装备实体。
Admin 与邮件装备来源通过校验后直接移交已消费快照的名称、分类、外部 ID 和 JSON 属性，避免额外深拷贝；
事务重试仍持有独立可重放写入值，key 清零与封装边界不变。
起拍价分词借用原文，保持 CR/LF/CRLF 和原诊断索引；拒收 Return 移交原排序附件，不为保留数量复制列表。
安全展示在无附件 key 时跳过逐窗口摘要，但仍检查和屏蔽 URL key；URL-span 验证只读到首个命中。
装备来源的临时网络、限流或服务故障保留原可重试错误及诊断任务，不转为永久拒收或安排 Return；
确定的来源、身份或过滤拒绝继续沿原受控退件路径。来源采集时间与本地业务变更时间分离，外部读取不能决定锁后聚合时间。
`ports/mail/equipment_diagnostics/reason.rs` 拥有来源解析的九项闭集诊断码，进程内观察使用枚举，
只有 parse/protocol_drift 组合可投影到原有 sidecar 的 `detail_code`；不接受原始来源错误文本，
不参与业务决定、命令摘要或历史记录重写。

[poll/progress.rs](crates/auction-application/src/ports/mail/poll/progress.rs) 定义只读轮询观测端口与安全数字 MID 边界，
`WorkerControlPort::mail_progress` 提供可选跨进程读取；观测失败不改变命令、租约或持久游标，旧适配器明确不可用。

`MailboxMaintenanceService` 不把零条 Sent 匹配当作未发送证明，未知 Send 始终保留人工核对；
只在本批存在可重建的 Send 候选时读取完整 Sent 列表；空批次、仅 Take 或仅不可重建旧行不发起该扫描，
也不把无关 Sent 故障计为本批失败。有 Send 却没有完整列表时，仍停在首个 Send 候选前，不推进它及其后缀。
Take 先匹配原冻结 world/MID 与凭据 ID/version，才可解释严格缺失或原发件人完整详情的全部资产归零；
仅原装备集合、Credits、COD 完全相同才可有界重排。`MoogleMailboxInboxObservation` 的明确缺失同样携带观察凭据，
换账号、换凭据版本、旧无绑定或配置 Cookie 缺少持久绑定均保留人工核对。重启可改变 generation，但当前 generation、
原尝试和候选 fence 仍必须由恢复事务锁内复核。部分、额外、重复装备或金额漂移不成功、不重排；只统计已确认提交的实际迁移。
Send 恢复仍要求原冻结金额与资产证明，不能从已领取后的空资产反推原值；全部来源使用原实际渲染身份，
不重建当前模板/key，当前观测凭据版本必须匹配原身份。旧无身份通知仅保留无登录 key 的唯一入站关联证明；
Sent 时间落在原 `sending_at` 与未知结果记录时间之间，分钟精度不能向下放宽围栏，重复匹配与精度歧义均保持未知。
compose 的未知 COD 不视为零；Admin delivery offset 上限为 `100_000`，Admin 邮件可选空筛选等同省略。
`MoogleMailboxSweepCandidate` 即使无法重建发送载荷也保留投递身份；维护在尝试单条前持久推进环形位置，
不跨过未处理的预算后缀。整轮原 deadline 覆盖数据库、session 与远端；discard 必须先消费精确 fence/身份的一次性尝试，
全库同内容未绑定尝试不能仅凭批次前缀消歧；再在同 session 重读草稿，未知或取消不能自动重复。观察也核对原 fence，
不会附着到管理员重试后的新尝试；游标与发送完成证明互不替代。
旧无冻结身份的 compose 草稿也必须匹配规范排序且无重复的精确 EID，不能用附件数量冒充身份；
Credits 独立按金额核对，不因远端将其计作一条附件而误拒绝原现金草稿。Take 恢复与重排同样携带原观察的
`fencing_version`，管理员重试后的新尝试不能消费旧资产观察。
维护唯一性判断只保留借用候选；冻结 Send 在第 33 个列表匹配处即按既有 32 项上限拒绝，不先收集全部命中。
compose 仍统计完整匹配数；零匹配、多匹配、资产或身份不充分都不授予绑定、discard 或重寄权限。

可配置 MM 正文仍为 14 项，默认使用分段英文；模板只展示已决定的拒收、结算与账户事实，不重新决定退回或金额。
买家装备 COD 的 `{price_breakdown}` 只消费冻结结算明细，`{amount}` 保持实际 COD；已冻结正文与幂等摘要不因默认文案升级
重写。模板占位符与保存生效范围见[配置手册](ops/config/README.md#邮件正文模板)。
三层模板边界均拒绝任意位置的孤立右花括号；合法占位符、必需字段与已冻结正文不变。

[ports/scheduling](crates/auction-application/src/ports/scheduling) 只表达冻结轮次与下一 UTC 触发时刻（cron 解析在配置层），
定时器不能替代持久化 due、游标和租约；[ports/operations](crates/auction-application/src/ports/operations) 定义安全水位、
告警评估与批次状态，告警不改变拍卖或外部投递事实；[errors](crates/auction-application/src/errors) 保留领域分类和可重试性。
Recovery 外部余额失效时仍保留数据库积压与 `EvidenceStale` 告警，并同时报告 `source_unavailable`；
不伪造来源健康，也不在不完整观察中关闭旧告警。
告警批次由 `canonical_summary_prefix` 一次选取原排序下可容纳的最大非空前缀，避免反复删尾并重建正文；
正文、序号和摘要字节不变，存储只截取该前缀，未纳入信号仍留在持久队列。

## 读取、实时与会话

[services/sessions](crates/auction-application/src/services/sessions) 将 Public 认证版本与会话创建锁内复核衔接，
key 撤销/重签永久废止旧会话；Public 默认永久，有限期限不随普通活动延长。长连接只读 `inspect` 仍复核账号、凭据、
撤销和有限截止。账号时长、版本与会话绑定摘要由 [session_preference.rs](crates/auction-application/src/ports/public/session_preference.rs)
拥有；保存更新当前会话及后续登录偏好，重放返回原结果而不重置截止，其他会话不变。
Admin 服务启动前撤销旧 Admin 会话，失败阻断启动，token 摘要绑定当前 actor/secret；账号/口令校验由会话 owner 导出，
首次设置与敏感配置复用相同边界。[ports/realtime](crates/auction-application/src/ports/realtime) 定义事件、连续游标、
消费所有权和重放边界，事件不能代替权威快照（见[实时契约](docs/contracts/events.md)）。
Public/Admin 登录结果的 `Debug` 只保留身份投影，原始 session 与 CSRF token 始终脱敏；Public 两个截止同为 `None` 才表示永久。
会话 token 先检查 43 字节无填充编码长度，再解码到 32 字节栈缓冲；摘要输入仍是解码字节，不是 Cookie 文本。
Admin 录入命令中的完整装备 URL、论坛镜像正文和邮箱 compose 草稿的主题/正文也在 `Debug` 中脱敏，
避免 URL 包装或只读投影重新暴露装备 key、登录 key；实际业务字段和命令摘要保持原文。
本地 relay 只按有界分页遍历仍有事件位于本 consumer 游标之后的 stream，已追平的历史 stream 不进入每轮扫描；
每条已校验事件直接移交 sink，只保留确认所需的 ID/序号，
避免复制完整 JSON payload。sink 失败不推进游标，sink 成功但游标提交失败仍按原 at-least-once 规则重投。
公开事件注册表按每个字段一次查找同时拒绝缺失与 `null`；完整字段集合、类型、金额范围和流身份校验不变。

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
