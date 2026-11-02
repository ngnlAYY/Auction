# MoogleMail 邮件流程

[文档导航](../README.md) · [业务规则](../business-rules.md) · [结算与履约](settlement.md) · [auction-integrations](../../crates/auction-integrations/README.md)

Bot 通过 HentaiVerse 的 MoogleMail 与玩家交互：玩家寄来投稿、注册与换绑请求，Bot 寄出登录 key、回执、结算邮件与提醒。所有入站邮件都经持久化游标逐封处理；所有出站邮件都经 outbox 投递，结果不确定时转人工对账而不是重发。

## 收件轮询

- Worker 为每个启用的世界维护一个邮箱（`mailbox_key`，如 `mooglemail:main`），按 cron 计划（默认每 5 分钟，带抖动）读取收件箱，领取邮箱租约后逐封处理并推进持久游标。
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
- 正文模板可在 `[worker.mooglemail.mail_templates]` 或 runtime 文件中定制；模板只能使用规定的占位符（如 `{login_key}`、`{amount}`、`{reference}`），未知键或非法正文在 Worker 启动时被拒绝。
- 同一时刻只有一个 Worker 持有邮件 Cookie 的运行时所有权；对同一账号的远端写入串行执行。

## 资产动作证明

- 投稿的 Take 与整封 Return 都先冻结原邮件身份与资产期望，执行前重新核对；Take 只有在响应证明原邮件身份一致、资产归零且领取回执匹配时才算成功。
- Return 需要绑定原发件人、收件人、主题、正文及完整资产的 `[RTS]` 回执。详情 GET 本身就会把邮件从仅含未读项的 Inbox 移走，因此 Inbox 中找不到邮件不能证明退回成功。
- Send 需要从发送前 Sent 基线之外找到新邮件，精确匹配冻结的收件人、主题、正文与完整资产，并再次证明 compose 已干净；成功提示或 HTTP 200 本身不足以证明完成。进入远端写入后无法取得确定证明时，保留未知结果转人工对账，不自动重复执行。

## 邮箱维护

Worker 周期运行邮箱维护：观察 compose 草稿与 Sent 列表，为结果未知的投递寻找精确匹配的 Sent 证据（主题、正文摘要、世界、收件人与完整附件；列表时间只到分钟，按发送开始所在分钟比较并排除同分钟同内容的其他尝试），对未找回的结算发送还会按唯一义务主题在完整 Sent 中记录目击，阻止 Admin 重试或判失败（见[结算与履约](settlement.md#3-投递outbox)），并在精确匹配后受控丢弃残留草稿。找不到 Sent 不等于未发送——未知的发送永不自动重试，最终由 Admin 人工对账。

## 凭据

配置了完整的 `credential_data_key` 时，Worker 只从数据库读取适用世界或 shared scope 的 active 邮件凭据，不再回退 `.keyenv` 或敏感配置；没有该密钥时，先使用显式 `keyenv_file`，未配置才使用 `mooglemail_cookie`。凭据可在 Admin “Cookie 池”页或用 `auction-cli credential import` 录入；全库同时只能有一个 active 邮件凭据，录入或切换后需重启 Worker。

## Admin 与 CLI 入口

| 入口 | 用途 |
| --- | --- |
| Admin “收件审核” | 各邮箱轮询进度与诊断、入站邮件处理结果与详情 |
| Admin “投递队列” | 出站邮件状态、释放、重试、人工对账 |
| `auction-cli mail export-evidence` | 解密导出诊断证据 |
| `auction-cli mail inspect-sent` | 只读核对撤拍退回邮件的 Sent 状态 |
