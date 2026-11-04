# MoogleMail 邮件流程

[文档导航](../README.md) · [业务规则](../business-rules.md) · [结算与履约](settlement.md) · [auction-integrations](../../crates/auction-integrations/README.md)

Bot 通过 HentaiVerse 的 MoogleMail 与玩家交互：玩家寄来投稿、注册与换绑请求，Bot 寄出登录 key、回执、结算邮件与提醒。所有入站邮件都经持久化游标逐封处理；所有出站邮件都经 outbox 投递，结果不确定时转人工对账而不是重发。

## 收件轮询

- Worker 为每个启用的世界维护一个邮箱（`mailbox_key`，如 `mooglemail:main`），按 cron 计划（默认每 5 分钟，带抖动）读取收件箱，领取邮箱租约后逐封处理并推进持久游标。
- 默认收件与邮箱维护独立运行；两个世界同时启用时按世界平移日程。Admin 手动触发立即唤醒收件，不受相位影响，也不带动独立维护；配置与旧路径回退见[配置参考](../../ops/config/README.md#mooglemail-收件与维护)。
- 发件人白名单（`sender_allowlist`）按详情中的完整名称、不区分大小写核验，不能用列表截断名称提前授权。名单为空且未显式关闭（`sender_allowlist_disabled = true`）时拒绝拉取；名单非空时会读取详情后再核验，名单外邮件保留在待读队列中，游标不越过它。生产环境应配置白名单或明确关闭。
- 每封邮件以“邮箱 + 远端邮件 ID”记入 `inbound_mail`，重复读取不会重复处理；已持久接受的投稿在重试时恢复原决定，不因远端页面变化或重试时跨越赛季边界改判。
- Admin 可在“仪表盘”或“收件审核”页暂停/恢复某个邮箱，或立即触发一次轮询（经 Worker control）。启用自动排期时，轮次进入预览后从计划开拍前 2 小时起暂停对应世界，实际开拍后恢复；取消或错过已预览的轮次也会解除排期暂停。参与赛季早鸟期的世界还可能受赛季前暂停控制，完整时序见[赛季排期](../business-rules.md#赛季早鸟期auctionscheduleearly_bird)。暂停只控制轮询，不能替代下述事务内的收件门禁。

## 主题路由

主题按 ASCII 不区分大小写匹配，不裁剪首尾空白：

| 主题 | 处理 |
| --- | --- |
| `auction` | 拍卖投稿 |
| `register` | 注册或找回登录 key |
| `rebind` | 改名后换绑账号 |
| `rekey` | 轮换登录 key 并注销全部会话 |
| `auction equipment delivery [<UUID>]`、`auction payment request [<UUID>]` | 结算邮件被退回（退件） |
| 其他 | 不含附件/Credits/COD：归档；含资产：整封退回 |

## 拍卖投稿

玩家把要寄售的装备作为附件，主题写 `auction`，发给 Bot 所在世界的账号。

1. 邮件附带 Credits 或 COD → 整封退回并回信说明（`auction_assets_rejected`）。
2. 没有附件 → 回信提示（`auction_no_attachments`）。
3. 解析正文中的起拍价指令（见下）。指令有误 → 整封退回。
4. 逐件读取装备详情，校验身份与来源采集时间，再按过滤规则检查：
   - 必须可交易（`tradeable_required`）；
   - 品质在接收列表内（默认 Legendary、Peerless；早鸟窗口内使用早鸟品质列表）；
   - 启用过滤时检查前缀、后缀、分类、类型与等级范围。
   任一件不合格 → **整封**退回，并回信说明原因（`auction_equipment_rejected`）。装备页面暂时读不到等非确定错误不会退回，而是留待重试。
5. 全部通过 → 在一个事务内把装备加入该世界当前接收投稿的场次（Draft 或 Preview），保存加密的装备 key，然后投递一个“收取附件”操作把装备收入 Bot 账号。若锁后数据库当前时间或邮件原始 `received_at` 早于该场次冻结的最早收件时刻（新赛季开始），事务拒收 → **整封**退回并回信说明（`auction_equipment_rejected`，原因 `season_intake_closed`），不收取附件。因此旧赛季来信即使延迟到新赛季处理仍被拒绝；关闭赛季前轮询暂停也不绕过此门禁。已持久接受的投稿按上文重放原决定。

被拒绝的装备读取会留下加密诊断证据（默认保留 7 天），可用 `auction-cli mail export-evidence` 导出。

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
- 说明信（`auction` 拒收/无附件说明、`rebind` 结果、`user_assets_rejected`、论坛回执）按发件人或论坛账号每 24 小时最多 10 封（窗口与上限可配置）；超出时只记录命令与审计，不寄说明信，但邮件附带的资产仍照常退回。

账户类邮件带资产时整封退回并回信说明（`user_assets_rejected`）。登录 key 只出现在回信中，系统只保存其摘要与加密信封；回信需要 `user_login_data_key`。

## 结算退件

结算寄出的 COD 或装备邮件被收件人拒收（或过期）退回时，主题仍带原义务 ID。处理器只把它记录为退件证据（义务 ID、Credits、COD、附件），由恢复流程在后续事务中决定收回装备、重寄 COD 或寄回卖家，见[结算与履约](settlement.md#4-恢复付款超时与拒收)。

## 出站邮件

所有出站邮件先写入 outbox，由投递循环发送（详见[结算与履约](settlement.md#3-投递outbox)）。常见类型：

| 类别（发送间隔配置键） | 邮件 |
| --- | --- |
| `account_seconds` | 登录 key、注册与换绑结果 |
| `intake_seconds` | 投稿接收/拒绝回执、论坛竞价通知 |
| `settlement_seconds` | COD 付款、卖方货款、退款、付款提醒 |
| `equipment_seconds` | 装备交付、重寄、流拍/撤拍寄回 |
| `outbid_seconds` | 标记装备的超价提醒 |
| `operations_seconds` | 运维告警 |
| `manual_seconds` | 手工及未归类邮件 |

- 每类可设置发送起点的最小间隔（默认 0），跨世界共用，并与客户端请求间隔（默认 1 秒）及单账号串行叠加。
- 出价被超提醒的有效期只到该 lot 的有效截止（含软截止延长）：截止后尚未开始发送的提醒在发送围栏前按预检退役，记为未发送失败（`lot_watch_invalidated`），不发送也不补发；论坛私信队列同样在截止后退役（`suppressed`/`source_invalid`）。已经进入发送围栏的提醒不改写结果，未知时只做人工对账。
- 正文模板可在 `[worker.mooglemail.mail_templates]` 或 runtime 文件中定制；模板只能使用规定的占位符（如 `{login_key}`、`{amount}`、`{reference}`），未知键或非法正文在 Worker 启动时被拒绝。
- 同一时刻只有一个 Worker 持有邮件 Cookie 的运行时所有权；对同一账号的远端写入串行执行。

## 通知目录

这是全部出站通知的唯一清单（W14.1）。新增或修改通知时必须同步本表、`auction_application::NotificationKind` 与 `AdminRuntimeMailTemplatesConfig`；`crates/auction-application/tests/notification_catalog.rs` 会核对三处一致。

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
| `user_assets_rejected` | 账号类主题（`register`/`rebind`/`rekey`）携带资产 | 发件人游戏名 | MM | account | `deliveries` outbox / delivery 循环 | 发件人 24 小时说明信上限（抑制）；资产照常退回 | 收件人、主题、退回邮件身份；正文无占位符 |
| `forum_auction_notice` | 论坛回帖命令 `notify` 接受并建立稳定命令事实 | 论坛回帖者游戏名 | MM | intake | `deliveries` outbox / delivery 循环 | 与说明信共用发件人 24 小时上限（抑制）；同一 `post_id`/行/lot 只建一条命令 | 收件人、主题、lot 代码与行号；正文为冻结模板文本 |
| `settlement_winner_equipment_cod` | 结算产生竞得装备 COD 义务并准备发送 | 买家游戏名 | MM | settlement（含装备时为 equipment） | `deliveries` outbox / delivery 循环 | 义务被取消或前置未满足（不进围栏）；未领取 COD 超过付款期限转恢复流程 | 收件人、义务 ID、装备 ID、COD 金额、结算明细；占位符 `{item_name}`、`{amount}`、`{reference}`、`{price_breakdown}` |
| `settlement_payment_carrier` | 异世界竞得者需向 main 支付 1 Credit 载体 | 买家游戏名 | MM | settlement | `deliveries` outbox / delivery 循环 | 前置义务未完成；COD 未领取超过期限转恢复 | 收件人、义务 ID、Credits 与 COD；占位符 `{amount}`、`{reference}` |
| `settlement_payment_reminder`（payload 身份 `recovery_payment_reminder`） | 恢复流程判定 COD 款项逾期仍未领取 | 买家游戏名（固定 main） | MM | settlement | `deliveries` outbox / delivery 循环 | 付款/领取已完成或恢复动作已终结即不再创建；已创建不重发 | 收件人、recovery 身份、原邮件 ID；占位符 `{reference}` |
| `settlement_equipment_handoff` | 结算需要把装备交给买家（非 COD 路径） | 买家游戏名 | MM | equipment | `deliveries` outbox / delivery 循环 | 前置义务未满足；退件由恢复流程重新决定 | 收件人、义务 ID、装备 ID；占位符 `{item_name}`、`{reference}` |
| `settlement_seller_payout` | 结算确认买方付款后向卖家支付货款 | 卖家游戏名 | MM | settlement | `deliveries` outbox / delivery 循环 | 存在前置义务或被冲正；只按冻结义务发送 | 收件人、义务 ID、金额；占位符 `{amount}`、`{reference}` |
| `settlement_buyer_refund` | 恢复/撤拍决定退还买方 Credits | 买家游戏名 | MM | settlement | `deliveries` outbox / delivery 循环 | 前置证据不足或已完成 | 收件人、义务 ID、金额；占位符 `{amount}`、`{reference}` |
| `lot_watch` | 竞价事务内领先者失去领先周期，且其偏好渠道为 MM | 被超玩家的游戏名 | MM | outbid | `deliveries` outbox / delivery 循环 | 取消标记、重新领先（`invalidated_at`）或 lot 有效截止已过，在围栏前退役（`lot_watch_invalidated`） | 通知 ID、lot、领先周期、被超价格、世界；正文冻结为英文模板 |
| `lot_watch_pm` | 同一领先周期失去领先，且偏好渠道为 PM 并已绑定论坛账号 | 绑定论坛账号 | PM | ——（PM 队列自有时序） | `lot_watch_pm_deliveries` / forum PM 循环 | 未绑定论坛账号时直接 `suppressed`（`recipient_missing`）；lot 截止后 `suppressed`/`source_invalid` | 通知 ID、论坛账号 ID、标题与正文摘要 |
| `operations_alert` | 扫描器判定需要运营处理（积压、失败、逾期、余额、证据过期等） | 配置的运营收件人 | MM | operations | `deliveries` outbox（受 `operations_alert_batches.status='enqueued'` 门禁）/ delivery 循环 | 批次被合并、冷却或取消即不再创建；同指纹去重 | 收件人、告警批次、安全摘要；占位符 `{summary}` |
<!-- notification-catalog:end -->

### 通知指标

`auction_notification_events_total{kind,stage}` 按上表 `模板键 / 身份` 统计 `created`、`retired`、`suppressed`、`sent`、`failed`；`auction_notification_failures_total{kind,reason}` 细分失败原因（`send_rejected` 另立 `insufficient_postage`），`auction_notification_delivery_latency_seconds{kind}` 记录从 outbox 创建到终态的延迟，`auction_notification_postage_estimate_credits_total{kind}` 按发送次数累计估算邮资。标签全部来自闭集，不含收件人、正文或 MID。记录失败只影响观测，不改变发送、抑制、退役或人工对账结果。

PM 渠道没有独立 outbox：`lot_watch_pm` 的 `created`、`suppressed`（`recipient_missing`）与 `retired`（截止后 `source_invalid`）随 `lot_watch_pm_deliveries` 的队列转迁记录；`sent` 与 `failed` 由 PM 循环在 `finish` 收口时记录，回执缺失按 `write_unknown` 计入失败。PM 领取结构不带创建时间，因此 `auction_notification_delivery_latency_seconds{kind="lot_watch_pm"}` 目前没有样本；需要 PM 延迟时先给 PM claim 增加只读 `created_at`。

邮资金额仍未核实（W0.12），`auction_notification_postage_estimate_credits_total` 在核实前保持零值，只作为 W11.2 的接口；不得把零值当作“免邮资”结论。

## 资产动作证明

- 投稿的 Take 与整封 Return 都先冻结原邮件身份与资产期望，执行前重新核对；Take 只有在响应证明原邮件身份一致、资产归零且领取回执匹配时才算成功。
- Return 需要绑定原发件人、收件人、主题、正文及完整资产的 `[RTS]` 回执。详情 GET 本身就会把邮件从仅含未读项的 Inbox 移走，因此 Inbox 中找不到邮件不能证明退回成功。
- Send 需要从发送前 Sent 基线之外找到新邮件，精确匹配冻结的收件人、主题、正文与完整资产，并再次证明 compose 已干净；成功提示或 HTTP 200 本身不足以证明完成。进入远端写入后无法取得确定证明时，保留未知结果转人工对账，不自动重复执行。 发送前基线默认处于影子模式：旧完整排除集裁决，同时只读比较临时 MID 边界的新邮件集合与列表候选集合。启用水位后，每次发送前读取完整最新分钟组，发送后核对原锚点并读完边界分钟，再取得候选；边界只缩小扫描范围，不代替身份、正文与资产证明。异常时完整回退仍受页数与预算限制；发送后边界前提失效且不能恢复原排除集时保持未知。配置和人工切换条件见[发送前基线](../../ops/config/README.md#mooglemail-发送前基线)，默认影子模式本身不解除完整扫描上限。

## 邮箱维护

Worker 周期运行邮箱维护：观察 compose 草稿与 Sent 列表，为结果未知的投递寻找精确匹配的 Sent 证据（主题、正文摘要、世界、收件人与完整附件；列表时间只到分钟，按发送开始所在分钟比较并排除同分钟同内容的其他尝试），对未找回的结算发送还会按唯一义务主题在完整 Sent 中记录目击，阻止 Admin 重试或判失败（见[结算与履约](settlement.md#3-投递outbox)），并在精确匹配后受控丢弃残留草稿。找不到 Sent 不等于未发送——未知的发送永不自动重试，最终由 Admin 人工对账。

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
