# MoogleMail 邮件流程

[docs/README.md](../README.md) · [docs/business-rules.md](../business-rules.md) · [docs/guide/settlement.md](settlement.md) · [crates/auction-integrations/README.md](../../crates/auction-integrations/README.md)

Bot 通过 HentaiVerse 的 MoogleMail 与玩家交互：玩家寄来投稿、注册与换绑请求，Bot 寄出登录 key、回执、结算邮件与提醒。所有入站邮件都经持久化游标逐封处理；所有出站邮件都经 outbox 投递，结果不确定时转人工对账而不是重发。

## 收件轮询

- Worker 为每个启用的世界维护一个邮箱（`mailbox_key`，如 `mooglemail:main`），按 cron 计划（默认每 5 分钟，带抖动）读取收件箱，领取邮箱租约后逐封处理并推进持久游标。
- 默认收件与邮箱维护独立运行；两个世界同时启用时按世界平移日程。Admin 手动触发立即唤醒收件，不受相位影响，也不带动独立维护；配置与旧路径回退见[ops/config/README.md#mooglemail-收件与维护](../../ops/config/README.md#mooglemail-收件与维护)。
- 发件人白名单（`sender_allowlist`）按详情中的完整名称、不区分大小写核验，不能用列表截断名称提前授权。名单为空且未显式关闭（`sender_allowlist_disabled = true`）时拒绝拉取；名单非空时会读取详情后再核验，名单外邮件保留在待读队列中，游标不越过它。生产环境应配置白名单或明确关闭。
- 每封邮件以“邮箱 + 远端邮件 ID”记入 `inbound_mail`，重复读取不会重复处理；已持久接受的投稿在重试时恢复原决定，不因远端页面变化或重试时跨越赛季边界改判。
- Admin 可在“仪表盘”或“收件审核”页暂停/恢复某个邮箱，或立即触发一次轮询（经 Worker control）。启用自动排期时，轮次进入预览后从计划开拍前 2 小时起暂停对应世界，实际开拍后恢复；取消或错过已预览的轮次也会解除排期暂停。参与赛季早鸟期的世界还可能受赛季前暂停控制，完整时序见[docs/business-rules.md#赛季早鸟期auctionscheduleearly_bird](../business-rules.md#赛季早鸟期auctionscheduleearly_bird)。暂停只控制轮询，不能替代下述事务内的收件门禁。

## 主题路由

主题按 ASCII 不区分大小写匹配，不裁剪首尾空白：

| 主题 | 处理 |
| --- | --- |
| `auction` | 拍卖投稿 |
| `register` | 注册或找回登录 key |
| `rebind` | 改名后换绑账号 |
| `rekey` | 轮换登录 key 并注销全部会话 |
| 发件人恰为系统 `MoogleMail`，主题为 `[RTS] ` + `auction equipment delivery [<UUID>]`、`auction payment request [<UUID>]`、`auction seller payout [<UUID>]` 或 `auction buyer refund [<UUID>]` | 结算邮件被退回（退件），见[结算退件](#结算退件) |
| 其他（含无前缀的结算主题与玩家自拟的 `[RTS] …` 主题） | 不含附件/Credits/COD：归档；含资产：整封退回 |

## 拍卖投稿

玩家把要寄售的装备作为附件，主题写 `auction`，发给 Bot 所在世界的账号。

1. 邮件附带 Credits 或 COD → 整封退回并回信说明（`auction_assets_rejected`）。
2. 没有附件 → 回信提示（`auction_no_attachments`）。
3. 解析正文中的起拍价指令（见下）。指令有误 → 整封退回。
4. 逐件读取装备详情，校验身份与来源采集时间，再按过滤规则检查：
   - 必须可交易（`tradeable_required`）；
   - 品质在接收列表内（默认 Legendary、Peerless；早鸟窗口内使用早鸟品质列表）；
   - 启用过滤时检查前缀、后缀、分类、类型与等级范围。
   任一件不合格 → **整封**退回，并回信说明原因（`auction_equipment_rejected`）。装备页面暂时读不到等非确定错误不会退回，而是留待重试；Bot 角色正在战斗（返回战斗页）或远端限流同样只算暂时读不到。外部装备 ID 已登记时，仅当该装备最新 lot 已 `sold`、所属场次已结算并已交付给当前发件人，且旧装备 key 指纹、名称与分类均匹配时，复用旧 `equipment_id` 创建新 lot；未交付、非当前持有人、key 或公开属性不匹配、最新 lot 仍 pending/live 的重复投稿均按确定性冲突整封退回，原因码 `auction_equipment_conflict`。
5. 全部通过 → 在一个事务内把装备加入该世界当前接收投稿的场次（Draft 或 Preview），保存加密的装备 key，然后投递一个“收取附件”操作把装备收入 Bot 账号，并按整封投稿合并寄出投稿接收回执（`auction_submission_receipt`：场次、每个 lot 的代码、装备名与起拍价）。若场次在回执发送前已开拍，回执在 sending 围栏前退役，不补寄旧场次回执。若锁后数据库当前时间或邮件原始 `received_at` 早于该场次冻结的最早收件时刻（新赛季开始），事务拒收 → **整封**退回并回信说明（`auction_equipment_rejected`，原因 `season_intake_closed`），不收取附件。因此旧赛季来信即使延迟到新赛季处理仍被拒绝；关闭赛季前轮询暂停也不绕过此门禁。已持久接受的投稿按上文重放原决定。

被拒绝的装备读取会留下加密诊断证据（默认保留 7 天），可用 `auction-cli mail export-evidence` 导出。诊断归档优先使用独立的 `mail_diagnostics_data_key` 域；未配置时沿用 `credential_data_key`、再退回 `equipment_data_key`。归档头部记录写入域与 key ID，导出按头部选择同域密钥环，因此轮换期间保留的旧 key 也能解密历史证据；见[ops/keys/README.md](../../ops/keys/README.md)。

### 起拍价指令

未写指令时，每件使用邮件默认起拍价（默认 50,000 Credits，`[worker.mooglemail] start_price`）。

| 写法 | 含义 |
| --- | --- |
| `start 200k` | 单独成行：本封所有装备起拍价 200,000 |
| `https://hentaiverse.org/equip/<ID>/<KEY> start 1.5m` | 逐件指定：链接中的装备 ID 与 key 必须与附件一致 |

- 金额可写纯数字、`k`（千）或 `m`（百万）后缀，后缀形式允许小数（如 `1.5m`），也允许千位逗号（`1,500,000`）；结果必须为正整数。
- 逐件指令可以夹在说明文字中、可跨行；`#` 开头（或空白后的 `#`）是注释。统一指令与逐件指令不能混用，同一件不能重复指定，链接对应的装备必须在附件中。
- 统一指令前后可保留普通说明、网页复制的 Attachment 清单与分隔线；这些内容不会计入指令字段。但重复、混合或畸形的明确 `start` 仍会整封拒绝，例如 `start:200k`、`start=200k`。未逐件指定的附件保留默认价。

## 注册与换绑

| 主题 | 正文 | 结果 |
| --- | --- | --- |
| `register` | 任意（不能带附件、Credits 或 COD） | 以发件人游戏名注册并回信登录 key；已注册则重新发送 key。被拉黑或身份冲突时回信说明 |
| `rebind` | 旧登录 key | 把 key 对应账号的名称改为当前发件人的名称（用于游戏内改名后）；key 无效、被拉黑或冲突时回信说明 |
| `rekey` | 任意（不能带附件、Credits 或 COD） | 以发件人游戏名找到账号，在同一事务内轮换登录 key、注销该账号全部有效会话并回信新 key；未注册、被拉黑或身份冲突时回信说明。可在 Worker 配置中关闭该主题 |

- `register` 与 `rekey` 各自按账号 24 小时只回信一次（窗口可配置）；同一窗口内更早的旧 key 仍无法登录，因为轮换命令本身已经提交。超出的请求照常记录命令与审计，但不再寄出 key 或说明信。
- 说明信（`auction` 拒收/无附件说明、`rebind` 结果、`user_assets_rejected`）按发件人每 24 小时最多 10 封（窗口与上限可配置）；超出时只记录命令与审计，不寄说明信，但邮件附带的资产仍照常退回。

账户类邮件带资产时整封退回并回信说明（`user_assets_rejected`）。登录 key 只出现在回信中，系统只保存其摘要与加密信封；回信需要 `user_login_data_key`。

## 结算退件

结算寄出的 COD、装备、货款或退款邮件被收件人拒收（或过期）退回时，由系统发件人 `MoogleMail` 寄回 Bot 收件箱：主题为 `[RTS] ` + 原主题（仍带原义务 ID），正文为原正文加一行 `This message was returned from <玩家>, kupo!`。退信携带的附件、Credits 与 COD 原样记作证据，处理器不推断退信是否保留原 COD。处理器只在发件人恰为 `MoogleMail`（精确匹配，与集成层退信回执的判定一致）且去掉前缀后是上表四种结算主题之一时识别为退件。义务 ID 可由公开的场次与 lot ID 推出，不能证明来源，因此无前缀的结算主题、玩家自拟的同名 `[RTS] …` 主题和其他系统退信（例如 `[RTS] Auction payment overdue`）都按未知邮件处理。识别后处理器只把它记录为退件证据（义务 ID、Credits、COD、附件），不领取、不退回、不回信；来源已认证的退件即使资产异常（例如不可领取）也照样记为证据，交给后续消费方隔离，不让入站游标停在这封邮件上。

退件证据由 recovery Worker 在 Serializable 事务内按持久化的发件人与主题再次核对来源，并按义务分成互斥的两路消费，每封只消费一次：

- 恢复记录的付款或交付义务（main 装备 COD、isekai 载体 COD、isekai 装备交付）的退件由恢复流程与义务的预期退件（原装备或原载体 Credits，COD 为 0）严格比对，不符即隔离转人工，相符才决定收回装备、重寄 COD 或寄回卖家，见[docs/guide/settlement.md#4-恢复付款超时与拒收](settlement.md#4-恢复付款超时与拒收)。
- 其余退件（卖方货款、买方退款、流拍/撤拍寄回，以及没有恢复记录的付款）只追加 `settlement_return_evidence` 证据并同事务建立退件案件：能唯一绑定原派发且履约尚未终结时，同事务把履约转为人工对账（`manual_reconcile`，失败码 `settlement_mail_returned`）；履约已终结时只留证；无法绑定（未知义务、主题与义务种类不符、派发未知或不唯一、补寄正文无法证明、原派发未发送、世界不符或派发已被取代）时只隔离。消费本身不领取、不退回、不重发，也不改变资金。
- 来源无法证明的历史记录（撤掉的无前缀兼容入口留下的）只隔离为 `untrusted`，不计入恢复退回次数，也不改变恢复状态。

退件证据另记系统后缀写明的原收件人、原正文的 canonical 摘要与正文末行的补寄派发引用（只记这三项，正文本身不入库）。补寄的冻结正文末行固定为 `Dispatch reference: v1/<派发编号>/<补寄投递 ID>`，由发送端追加，不受模板修订影响；系统退回原样带回后，消费方据此把退件绑定到具体派发，并要求原正文摘要等于该派发发送时冻结的摘要、原收件人等于冻结收件人。没有引用的退件只认首寄；首寄已被其他退件绑定、只登记了后续派发而缺首寄历史时按 `unbound` 隔离，不默认绑定最新投递。

退件案件由 Admin 在退件页处理，每个动作按 `command_id` 幂等、携带案件版本，资产、金额与收件人只来自服务端冻结事实：

| 案件 | 可用动作 | 结果 |
| --- | --- | --- |
| `untrusted` 隔离 | 标记已核对 | 结案（`reviewed`），不授予领取、资金或补寄 |
| `unbound` 隔离 | 标记已核对；重新核验 | 重新核验由 Worker 只读读取 Bot 收件箱中的原退件，重新满足系统来源、资产与唯一原派发绑定时改绑为待决定，读不到或仍不成立时保持隔离；人工填写义务不构成证明 |
| 已绑定、待决定 | 领回并补寄；领回后代管；留箱结案 | 领回动作建立持久 Take；留箱是明确例外，资产记为退回待领取、原权利人的权益保留 |
| 已代管 | 从代管补寄 | 同补寄规则 |

持久 Take 以受限来源 `settlement_return_action` 经 delivery owner 先 prepared、再以独立激活身份进入 pending，外部 Take 由投递 Worker 在租约与发送围栏之后执行，收件人字段匹配系统发件人 `MoogleMail`；它与入站邮件、恢复动作对同一邮件的领取/退回互斥，失败与未知不解除。只有精确领取证明（同世界/MID、动作移除或成功页、预期资产逐项一致、投递身份与载荷未变）才一次性记为领回事实：现金退件同事务把原派发的资金分配由 `spent` 恢复为 `available`（金额不变，补寄时由新投递再次经资金门支出）；计划补寄且仍满足补寄资格时授权新派发，否则替原权利人代管，原履约以 `settlement_return_held` 终结。Take 失败或结果未知时案件停在 `take_unknown`，只认系统可验证证明，不自动重试、不接受操作者声明，投递台也不能对该 Take 人工重试或声明结果。

补寄只寄原冻结收件人，沿用原义务换新派发：履约派发编号加一并重置为 pending，由履约 Worker 按补寄命名空间的投递/prepare/activate 身份准备，冻结正文末行写派发引用；新派发的可信寄出证明成立后本案结束（`resent`），原义务仍等待收件人领取，再次退回形成下一案件。每个案件在建立时冻结补寄上限 3（不含首寄，按派发编号计），达到上限后只能代管。补寄限于卖方货款、买方退款与流拍/撤拍寄回；付款 COD 与异世界交付归恢复流程，普通退件不重寄、不取消成交、不拉黑；原派发没有资金分配的历史现金退件只能代管或留箱。

Admin “结算退件”页（`/settlement-returns`）按案件列出全部退件（含隔离记录），详情给出资产位置、派发链、领回与代管、处理事件，以及服务端判定的允许动作与阻断原因；页面只提交 `command_id`、案件版本与动作名，接口见[docs/contracts/routes.md#admin-api](../contracts/routes.md#admin-api)。Take 结果未知的案件没有人工声明入口，当前版本也还没有为结算退件 Take 补采可验证证明的路径，案件会保持打开并持续计入隔离告警，处置见[ops/README.md#结算退件处理](../../ops/README.md#结算退件处理)。

## 出站邮件

所有出站邮件先写入 outbox，由投递循环发送（详见[docs/guide/settlement.md#3-投递outbox](settlement.md#3-投递outbox)）。常见类型：

| 类别（发送间隔配置键） | 邮件 |
| --- | --- |
| `account_seconds` | 登录 key、注册与换绑结果 |
| `intake_seconds` | 投稿接收/拒绝回执 |
| `settlement_seconds` | COD 付款、卖方货款、退款、付款提醒 |
| `equipment_seconds` | 装备交付、重寄、流拍/撤拍寄回 |
| `outbid_seconds` | 标记装备的超价提醒 |
| `operations_seconds` | 运维告警 |
| `manual_seconds` | 手工及未归类邮件 |

- 每类可设置发送起点的最小间隔（默认 0），跨世界共用，并与客户端请求间隔（默认 1 秒）及单账号串行叠加。
- 出队顺序固定为：运维告警 → 账号类回信（登录 key、注册与换绑结果）→ 按 lot 有效截止从早到晚的出价被超提醒 → 其他邮件；同类别按创建顺序。顺序只决定先后，不改变发送类别间隔、冻结载荷或任何结果。
- 客户端请求门按容量 1、不允许突发执行配置的请求间隔；业务写入优先于只读轮询，连续 3 次写入后必须把下一个令牌给排队中的后台请求（低优先级保底）。MoogleMail 与论坛客户端共用同一套请求门实现，优先级只决定先后，不改变请求间隔与写入未知语义。装备页读取不经请求门，但与同一账号的邮件请求共用在途锁与账号忙熔断：不与邮件请求同时发出，熔断窗口内不读取。长列表扫描按段释放成员关系锁：段间若有详情读取推进成员关系，本轮扫描放弃并按新状态重扫，不缓存半份列表。
- 出价被超提醒的有效期只到该 lot 的有效截止（含软截止延长）：截止后尚未开始发送的提醒在发送围栏前按预检退役，记为未发送失败（`lot_watch_invalidated`），不发送也不补发；论坛私信队列同样在截止后退役（`suppressed`/`source_invalid`）。已经进入发送围栏的提醒不改写结果，未知时只做人工对账。
- 收件人异常分两类收口：非结算零资产通知（未携带 Credits、CoD 或装备）在远端明确报告收件人不存在时直接失败关闭，写入终态审计（`recipient_not_found`），不进人工对账，也不保留退信 `mid`；带资产的投递仍按退信流程转人工取回。领取前还会核对提醒收件人是否仍是该用户当前游戏名：玩家改名后尚未开始发送的提醒按预检退役（`recipient_renamed`），不重发、不补发，也不改写冻结载荷。卖方汇总与换季提醒的冻结收件人认三种改名来源：Admin 改名（未撤回）、`rebind` 换绑改名（含大小写修正）与论坛身份同步改名；冻结名仍是某个当前用户的名称（含改回原名）时不退役。
- 正文模板保存在数据库，在 Admin 参数页的“MoogleMail 模板”里逐个定制或恢复默认（见[ops/config/README.md#出站通知模板数据库](../../ops/config/README.md#出站通知模板数据库)）；没有改过的模板使用程序内置的默认正文。模板只能使用规定的占位符（如 `{login_key}`、`{amount}`、`{reference}`），非法正文在保存时被拒绝。出价被超提醒（`lot_watch`、`lot_watch_pm`）带有 `{item_name}`、`{lot_code}`、`{amount}` 与可选的行级占位符 `{deadline_line}`、`{lot_url_line}`（PM 另有 `{notification_id}`，且模板必须保留 `Notification: {notification_id}.` 一句以绑定发送证明）；正文在创建通知时渲染并冻结进投递载荷，摘要/私信摘要绑定该冻结正文，之后修改模板只影响新通知。刷新、生效与读取失败时的规则见[docs/contracts/database.md#出站通知模板](../contracts/database.md#出站通知模板)。
- 同一时刻只有一个 Worker 持有邮件 Cookie 的运行时所有权；对同一账号的远端写入串行执行。
- **发送分类开关**：runtime `[operations.mail.send_classes]` 把出站邮件分成六类并可分别关闭：账号回信（`account_reply`）、投稿回信（`intake_reply`）、结算通知（`settlement_notice`）、出价被超提醒（`outbid_reminder`）、玩家汇总与换季提醒（`player_summary`）、运维告警的 MoogleMail 通知（`operations_alert`）。开关在 Worker 领取投递时生效：关闭类的投递保留在 outbox、暂不领取，重新开启后按正常调度补发仍有效的通知；已过有效期的提醒由既有预检退役。关闭只暂停通知，不改变已提交的命令、履约义务、审计与告警台记录；结算/履约动作邮件（COD 交付、货款、退款等）与资产退回不受任何开关影响。部署配置 `[worker.mooglemail.send_classes]` 提供同一组默认值，runtime 接管 `mail` 组后以其为准。

## 通知目录

这是全部出站通知的唯一清单。新增或修改通知时必须同步本表、`auction_application::NotificationKind` 与 `AdminRuntimeMailTemplatesConfig`；`crates/auction-application/tests/notification_catalog.rs` 会核对三处一致。

阅读约定：

- **触发事实**：创建通知的持久业务事实；不是外部页面事件。
- **渠道**：`MM` = MoogleMail；`PM` = 论坛私信。
- **发送类别**：`delivery_send_category` 的持久分组，对应 `[worker.mooglemail.send_intervals]` 的 `*_seconds` 键。
- **出队位置**：领取并执行发送的队列与循环。
- **失效条件**：在 sending 围栏前允许退役或抑制的条件；进入围栏后只按未知结果人工对账，不改写已冻结载荷。
- **模板键与冻结字段**：正文在创建时冻结；之后改模板只影响新通知。模板键之外的字段（收件人、主题、资产、引用）同样冻结并参与请求摘要。

<!-- notification-catalog:start -->
| 模板键 / 身份 | 触发事实 | 收件人 | 渠道 | 发送类别 | 出队位置 | 失效条件 | 冻结字段（正文外） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `register_key` | `register` 命令成功（`registered`）或找回既有账号（`recovered`） | 发件人游戏名 | MM | account | `deliveries` outbox / delivery 循环 | 同账号 24 小时内已回过一封（抑制）；创建后不退役 | 收件人、主题、登录 key 引用；正文占位符 `{login_key}`、`{username}` |
| `register_result` | `register` 命令被拒（`blacklisted`、`identity_conflict`） | 发件人游戏名 | MM | account | `deliveries` outbox / delivery 循环 | 同账号 24 小时预算（抑制） | 收件人、结果码；占位符 `{result_code}`、`{username}` |
| `rebind_result` | `rebind` 成功或失败（key 无效、被拉黑、身份冲突） | 发件人游戏名 | MM | account | `deliveries` outbox / delivery 循环 | 发件人 24 小时说明信上限（抑制） | 收件人、结果码；占位符 `{result_code}`、`{username}` |
| `rekey_key` | `rekey` 命令轮换成功（原会话已注销） | 发件人游戏名 | MM | account | `deliveries` outbox / delivery 循环 | 同账号 24 小时预算（抑制） | 收件人、主题、登录 key 引用；占位符 `{login_key}`、`{username}` |
| `rekey_result` | `rekey` 被拒（未注册、被拉黑、身份冲突） | 发件人游戏名 | MM | account | `deliveries` outbox / delivery 循环 | 同账号 24 小时预算（抑制） | 收件人、结果码；占位符 `{result_code}`、`{username}` |
| `auction_assets_rejected` | 投稿邮件含 Credits、CoD 或非装备资产，整封退回 | 发件人游戏名 | MM | intake | `deliveries` outbox / delivery 循环 | 发件人 24 小时说明信上限（抑制）；退回动作独立执行 | 收件人、主题、退回邮件身份；正文无占位符 |
| `auction_no_attachments` | `auction` 邮件事务内没有任何装备附件 | 发件人游戏名 | MM | intake | `deliveries` outbox / delivery 循环 | 发件人 24 小时说明信上限（抑制） | 收件人、主题；正文无占位符 |
| `auction_equipment_rejected` | 装备校验失败（筛选规则、结构漂移、重复、已投递等） | 发件人游戏名 | MM | intake | `deliveries` outbox / delivery 循环 | 发件人 24 小时说明信上限（抑制）；退回动作独立执行 | 收件人、主题、原因码、退回邮件身份；占位符 `{reason_code}`/`{reason_text}`、`{next_steps}`、`{mail_reference}` |
| `auction_submission_receipt` | 拍卖投稿被接受（`accepted`）；整封投稿按原邮件合并为一封 | 发件人游戏名 | MM | intake | `deliveries` outbox / delivery 循环 | 原场次已开拍或撤场（auction 状态离开 `draft`/`preview`）时在 sending 围栏前退役（`auction_notice_invalidated`，原因 `auction_opened`）；正文超出 16 KiB 或投稿事实缺失时不创建 | 收件人、场次序号、每个 lot 的代码、装备名与起拍价；正文在创建时冻结 |
| `auction_seller_summary` | 场次结算完成后，按卖方汇总该场全部成交与流拍 lot；每场每卖方一封 | 投稿时冻结的卖方游戏名 | MM（固定 main） | settlement | `deliveries` outbox / delivery 循环 | 冻结收件人已改名时在 sending 围栏前退役（`recipient_renamed`）；正文超出 16 KiB 时不创建；历史场次不回填 | 收件人、场次世界与期号、每个 lot 的代码、装备名与最终成交价、成交/流拍计数与合计；正文在创建时冻结 |
| `isekai_season_reminder` | 已冻结赛季边界前 7 天内，收件人仍有未完成的 isekai 结算义务（`pending`/`prepared`/`enqueued`） | 义务收件人（游戏名或已登记用户名） | MM | settlement | `deliveries` outbox / delivery 循环 | 到达冻结赛季边界（`season_notice_invalidated`，原因 `season_boundary_passed`）、义务全部满足（原因 `season_reminder_resolved`）或收件人已改名时在 sending 围栏前退役；没有冻结边界时不创建 | 收件人、冻结赛季边界、未完成义务的 lot 代码、装备名、金额与种类；正文在创建时冻结 |
| `user_assets_rejected` | 账号类主题（`register`/`rebind`/`rekey`）携带资产 | 发件人游戏名 | MM | account | `deliveries` outbox / delivery 循环 | 发件人 24 小时说明信上限（抑制）；资产照常退回 | 收件人、主题、退回邮件身份；正文无占位符 |
| `settlement_winner_equipment_cod` | 结算产生竞得装备 COD 义务并准备发送 | 买家游戏名 | MM | settlement（含装备时为 equipment） | `deliveries` outbox / delivery 循环 | 义务被取消或前置未满足（不进围栏）；未领取 COD 超过付款期限转恢复流程 | 收件人、义务 ID、装备 ID 与名称、COD 金额、结算明细、冻结付款期限；占位符 `{item_name}`、`{amount}`、`{reference}`、`{price_breakdown}`、`{payment_deadline}` |
| `settlement_payment_carrier` | 异世界竞得者需向 main 支付 1 Credit 载体 | 买家游戏名 | MM | settlement | `deliveries` outbox / delivery 循环 | 前置义务未完成；COD 未领取超过期限转恢复 | 收件人、义务 ID、装备名、Credits 与 COD、冻结付款期限；占位符 `{item_name}`、`{amount}`、`{reference}`、`{payment_deadline}` |
| `settlement_payment_reminder`（payload 身份 `recovery_payment_reminder`） | 恢复流程判定 COD 款项逾期仍未领取 | 买家游戏名（固定 main） | MM | settlement | `deliveries` outbox / delivery 循环 | 付款/领取已完成或恢复动作已终结即不再创建；已创建不重发 | 收件人、recovery 身份、原邮件 ID；占位符 `{reference}` |
| `settlement_equipment_handoff` | 结算需要把装备交给买家（非 COD 路径） | 买家游戏名 | MM | equipment | `deliveries` outbox / delivery 循环 | 前置义务未满足；退件由恢复流程重新决定 | 收件人、义务 ID、装备 ID；占位符 `{item_name}`、`{reference}` |
| `settlement_seller_payout` | 结算确认买方付款后向卖家支付货款 | 卖家游戏名 | MM | settlement | `deliveries` outbox / delivery 循环 | 存在前置义务或被冲正；只按冻结义务发送 | 收件人、义务 ID、装备名、金额；占位符 `{item_name}`、`{amount}`、`{reference}` |
| `settlement_buyer_refund` | 恢复/撤拍决定退还买方 Credits | 买家游戏名 | MM | settlement | `deliveries` outbox / delivery 循环 | 前置证据不足或已完成 | 收件人、义务 ID、装备名、金额；占位符 `{item_name}`、`{amount}`、`{reference}` |
| `lot_watch` | 竞价事务内领先者失去领先周期，且偏好包含 MM（`mm` 或 `both`）；被超者慢速代理仍可反击时只记待定事实（`lot_watch_pending_notifications`），代理耗尽（含到期与 Final 无法反超）或被更高普通出价击败的同一事务才入队；慢代理计划在提交前统一解析 pending | 被超玩家的游戏名 | MM | outbid | `deliveries` outbox / delivery 循环 | 待定期间反超成功（`leading_again`）或 lot 截止/场次离开 live/final（`lot_closed`）即退役； 取消标记、重新领先（`invalidated_at`）或 lot 有效截止已过，在围栏前退役（`lot_watch_invalidated`） | 通知 ID、lot、领先周期、被超价格、世界；正文在创建时按模板冻结；占位符 `{item_name}`、`{lot_code}`、`{amount}`、`{deadline_line}`、`{lot_url_line}` |
| `lot_watch_pm` | 同一领先周期失去领先，且偏好包含 PM（`pm` 或 `both`）并已绑定论坛账号；慢速代理待定规则与 MM 相同 | 绑定论坛账号 | PM | ——（PM 队列自有时序） | `lot_watch_pm_deliveries` / forum PM 循环 | 未绑定论坛账号时直接 `suppressed`（`recipient_missing`）；lot 截止后 `suppressed`/`source_invalid` | 通知 ID、论坛账号 ID、标题与正文摘要；正文在创建时按模板冻结；占位符同 `lot_watch`，另可 `{notification_id}` |
| `operations_alert` | 扫描器判定需要运营处理（积压、失败、逾期、余额、证据过期等） | 配置的运营收件人 | MM | operations | `deliveries` outbox（受 `operations_alert_batches.status='enqueued'` 门禁）/ delivery 循环 | 批次被合并、冷却或取消即不再创建；同指纹去重 | 收件人、告警批次、安全摘要；占位符 `{summary}` |
<!-- notification-catalog:end -->

### 通知指标

`auction_notification_events_total{kind,stage}` 按上表 `模板键 / 身份` 统计 `created`、`retired`、`suppressed`、`sent`、`failed`；`auction_notification_failures_total{kind,reason}` 细分失败原因（`send_rejected` 另立 `insufficient_postage`），`auction_notification_delivery_latency_seconds{kind}` 记录从 outbox 创建到终态的延迟，`auction_notification_postage_estimate_credits_total{kind}` 按发送次数累计估算邮资。标签全部来自闭集，不含收件人、正文或 MID。记录失败只影响观测，不改变发送、抑制、退役或人工对账结果。

PM 渠道没有独立 outbox：`lot_watch_pm` 的 `created`、`suppressed`（`recipient_missing`）与 `retired`（截止后 `source_invalid`）随 `lot_watch_pm_deliveries` 的队列转迁记录；`sent` 与 `failed` 由 PM 循环在 `finish` 收口时记录，回执缺失按 `write_unknown` 计入失败。PM 领取结构不带创建时间，因此 `auction_notification_delivery_latency_seconds{kind="lot_watch_pm"}` 目前没有样本；需要 PM 延迟时先给 PM claim 增加只读 `created_at`。

邮资金额仍未核实，`auction_notification_postage_estimate_credits_total` 在核实前保持零值，只作为偿付能力估算的接口；不得把零值当作“免邮资”结论。

## 资产动作证明

- 投稿的 Take 与整封 Return 都先冻结原邮件身份与资产期望，执行前重新核对；Take 只有在响应证明原邮件身份一致、资产归零且领取回执匹配时才算成功。
- Return 需要绑定原发件人、收件人、主题、正文及完整资产的 `[RTS]` 回执。详情 GET 本身就会把邮件从仅含未读项的 Inbox 移走，因此 Inbox 中找不到邮件不能证明退回成功。
- Send 需要从发送前 Sent 基线之外找到新邮件，精确匹配冻结的收件人、主题、正文与完整资产，并再次证明 compose 已干净；成功提示或 HTTP 200 本身不足以证明完成。进入远端写入后无法取得确定证明时，保留未知结果转人工对账，不自动重复执行。 发送前基线默认处于影子模式：旧完整排除集裁决，同时只读比较临时 MID 边界的新邮件集合与列表候选集合。启用水位后，每次发送前读取完整最新分钟组，发送后核对原锚点并读完边界分钟，再取得候选；边界只缩小扫描范围，不代替身份、正文与资产证明。异常时完整回退仍受页数与预算限制；发送后边界前提失效且不能恢复原排除集时保持未知。配置和人工切换条件见[ops/config/README.md#mooglemail-发送前基线](../../ops/config/README.md#mooglemail-发送前基线)，默认影子模式本身不解除完整扫描上限。

## 普通投递载荷口径

入站处理、恢复动作、运维告警、出价被超提醒、卖方汇总与换季提醒写入 `deliveries` 的普通（非结算）投递，在入队校验、发送前解码与内部履约的资产冲突排除三处使用同一判定（`auction-application` 的 `MailDeliveryShape`），任一处都不单独放宽或收紧：

- 来源 × 操作：入站邮件（`mail_intake`）可 Send、Return、Take；恢复动作（`settlement_recovery_action`）只 Send 付款提醒与 Take 退件，结算退件动作（`settlement_return_action`，`origin_id` 为 `settlement_return:<案件 ID>`）只 Take，两者 Take 的附件按字符串严格升序；运维告警、出价被超提醒、卖方汇总与换季提醒只 Send。闭集外的组合三处都拒绝。
- UUID：`origin_id` 与邮件 ID 中的 UUID（恢复记录、告警批次、由命令 ID 派生的邮件 ID）一律是小写连字符规范形式且非 nil；大写、无连字符、花括号与 urn 写法都拒绝。
- 附件装备 ID：1–32 位十进制、不全为零且不重复，最多 1000 件。入站邮件业务 envelope 的校验复用同一附件判定，附件越界的邮件在入站处理开始时即按无效 envelope 拒绝，不会等到入队才失败。
- `external_mail_id`：不超过 256 字节的规范单行文本；资金预览、入库闭合等需要正整数的路径另外要求无前导零的正整数。
- Send 不携带资产，模板字段按通知模板表核对；Return 与 Take 不携带模板、结果码、登录 key 引用或正文，入站邮件的 Return/Take 也不带金额。

入队拒绝越界命令；发送前解码把越界载荷判为无效载荷；资产冲突排除无法证明越界载荷与目标装备无关，按冲突失败关闭。

## 邮箱维护

Worker 周期运行邮箱维护：观察 compose 草稿与 Sent 列表，为结果未知的投递寻找精确匹配的 Sent 证据（主题、正文摘要、世界、收件人与完整附件；列表时间只到分钟，按发送开始所在分钟比较并排除同分钟同内容的其他尝试），对未找回的结算发送还会按唯一义务主题在完整 Sent 中记录目击，阻止 Admin 重试或判失败（见[docs/guide/settlement.md#3-投递outbox](settlement.md#3-投递outbox)），并在精确匹配后受控丢弃残留草稿。找不到 Sent 不等于未发送——未知的发送永不自动重试，最终由 Admin 人工对账。

默认每个世界的维护任务先执行上述恢复与清理，再检查完整 Read Mail 纯观察是否到期。纯观察只记录完整已读列表数量，默认最小间隔一小时，首次同样延后；实际执行向上取整到维护节奏，失败也消耗间隔。既有待读 MID 与 Take 的恢复定位仍按需读取原完整列表和详情，不受纯观察降频影响，也不从历史已读列表创建新业务指令。

## 凭据

配置了完整的 `credential_data_key` 时，Worker 只从数据库读取适用世界或 shared scope 的 active 邮件凭据，不再回退 `.keyenv` 或敏感配置；没有该密钥时，先使用显式 `keyenv_file`，未配置才使用 `mooglemail_cookie`。凭据可在 Admin “Cookie 池”页或用 `auction-cli credential import` 录入；全库同时只能有一个 active 邮件凭据，录入或切换后需重启 Worker。

## Admin 与 CLI 入口

| 入口 | 用途 |
| --- | --- |
| Admin “收件审核” | 各邮箱轮询进度与诊断、入站邮件处理结果与详情 |
| Admin “投递队列” | 出站邮件状态、释放、重试、人工对账 |
| `auction-cli mail export-evidence` | 解密导出诊断证据 |
| `auction-cli mail inspect-sent` | 只读核对撤拍退回邮件的 Sent 状态 |
