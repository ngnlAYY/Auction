# HTTP 路由契约

[文档导航](../README.md) · [实时事件](realtime.md) · [数据库](database.md) · [auction-http](../../crates/auction-http/README.md) · [DTO](../../crates/auction-contracts/README.md)

本页列出各进程对外提供的全部 HTTP 路径。路由表的权威来源是 [`crates/auction-http/src/http/routes/surfaces.rs`](../../crates/auction-http/src/http/routes/surfaces.rs)（API）与 [`crates/auction-http/src/assets/routes.rs`](../../crates/auction-http/src/assets/routes.rs)（浏览器页面）；增删路径时三处必须同步修改。

## 通用约定

### 响应外壳

```json
{"code": "ok", "message": "…", "data": { … }}
{"code": "bid_too_low", "message": "…", "data": null, "request_id": "…"}
```

- `code` 是稳定的机器可读值，客户端只依据 HTTP 状态与 `code` 分支，不解析 `message`（中文提示，可能调整）。通常成功为 HTTP 200 / `ok`；创建拍卖、用户和手工录入 lot 为 201 / `created`；凭据录入与首次发布规则为 201 / `ok`；触发邮件轮询为 202 / `accepted`；创建代理仍为 200 / `ok`。
- Public/Admin 响应头始终回传 `x-request-id`（合法输入沿用，缺失时生成，非法或重复输入返回 400；指标转发拒绝优先返回 404 并回传安全生成的标识）；错误 JSON 含 `request_id` 字段，但 handler 的常规错误可能为 null，排查以响应头为准。
- 时间为 RFC 3339 字符串；金额为整数 Credits；标识为小写 UUID 字符串。

### 通用错误码

| HTTP | code | 场景 |
| --- | --- | --- |
| 400 | `invalid_query`、`invalid_json`、`unexpected_body`、`invalid_*` | 查询参数不在白名单或格式错误；请求体不是合法 JSON、含未知字段或字段非法；仅接受空正文的接口收到请求体 |
| 401 | `unauthenticated` | 会话缺失或过期 |
| 403 | `origin_forbidden`、`bidder_ineligible` 等 | origin 不在 allowlist；无竞价资格 |
| 404 | `not_found` | 资源不存在或对当前调用者不可见 |
| 409 | `command_conflict` 及各业务冲突码 | 同一 `command_id` 携带不同请求；版本不匹配；业务规则拒绝 |
| 405 | `method_not_allowed`（带 `Allow`） | 路径存在但方法不受支持 |
| 413 / 415 | `body_too_large` / `unsupported_media_type` | 请求体超过路由上限；写接口非 JSON 内容类型 |
| 429 | `rate_limited`（带 `Retry-After`）、`realtime_busy`、`discovery_busy` | 速率或连接数超限 |
| 501 | `feature_not_supported` | 部分可选端口未装配（如 Admin 缺少数据密钥）；无数据库的 Public 读取/会话常用 503 |
| 503 | `storage_unavailable`、`temporarily_unavailable`、`realtime_unavailable` | 依赖或实时 Hub 暂不可用 |
| 422 | `invalid_watch`、`invalid_watch_preference`、`invalid_session_preference` | 标记或账号偏好输入不满足约束 |
| 500 / 504 | `internal_error` / `timeout` | 内部处理失败 / 普通 HTTP 请求或 WebSocket 握手超时 |

带过滤器的读取按各路由白名单解析查询参数，未声明的参数返回 `invalid_query`；不接受查询参数的业务命令由对应 handler 拒绝。JSON 写接口拒绝未知字段；登录、退出、会话 Cookie 同步与校验类接口按各自协议处理。Public/Admin 业务路由严格匹配方法，除倒计时明确支持 HEAD 外，JSON GET 不隐式接受 HEAD；健康、指标、静态资源与首次设置由各自 owner 定义方法边界。

### 写命令

持久化业务命令通常带客户端生成的 `command_id`（UUID）。服务端以“命令种类 + 聚合 + 请求摘要”记账：同一 ID 同一请求重放原结果；同一 ID 不同请求返回 `409 command_conflict`（部分 owner 使用专属冲突码）。客户端在结果不确定（超时、断网）时必须用**同一 ID 与同一请求体**重试。多数业务命令还要求携带服务端投影给出的期望版本（`expected_version`、`expected_state_version`、`expected_fencing_version`），版本不匹配时返回 409，需重新读取后再决定。登录、退出、空 PUT 会话同步、规则/runtime 候选校验、论坛预览、凭据在线校验与首次设置不使用该命令外壳；realtime 重放兼容省略 ID，由服务端按请求确定生成稳定 ID。

浏览器解除“结果未知”必须依据该路由可识别的确定响应，不能只依据 HTTP 409。标记、提醒渠道偏好与登录时长偏好的确定冲突分别为 `409 watch_conflict`、`409 watch_preference_conflict`、`409 session_preference_conflict`；无法解码或不符合该路由契约的 409 不解除此前累计的未知状态，仍须保留原命令与请求重试。

### 缓存头

公开目录类读取（世界、规则、黑名单、补偿池汇总、拍卖列表、当前拍卖、竞价日志、价格历史）返回 `Cache-Control: no-cache`；含会话、装备 key 或命令结果的响应以及全部 Admin 接口返回 `no-store`。

## 健康与指标（Public、Admin）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/health/live` | 进程存活：`{"code":"live","message":"…","data":{"live":true}}` |
| GET | `/health/ready` | 就绪：Public 要求数据库与 realtime relay 新鲜；Admin 要求数据库就绪；均按审计模式判定。默认 shadow 保持任一审计失败即未就绪，classified 仅由 permanent + unknown 阻断。成功为 200 / `ready`，未就绪为 503 / `not_ready` |
| GET | `/health`、`/api/v1/health` | 服务名、状态与运行进程版本 |
| GET / HEAD | `/metrics` | 直连 Prometheus 文本（HEAD 无正文）；需启用 metrics，且 listener 为 loopback TCP 或 Unix socket |

### 浏览器错误信标（Public、Admin）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/api/v1/browser-errors` | 匿名、限流、无请求体。查询参数 `class` 为闭集 `wasm_panic`、`dto_decode`、`realtime_protocol`；`build` 为 1–64 位 `[A-Za-z0-9._-]`。两者必须各出现一次；请求体必须为空。服务端只累加进程内计数，不写数据库、不记录查询值或错误正文。成功为 204 无正文；缺失、重复或非法参数为 400 `invalid_query`；任何非空请求体为 400 `unexpected_body`；超过每来源固定窗口上限为 429 `rate_limited`（带 `Retry-After`）。`build` 与当前进程构建号不同只增加构建偏差计数，不改变响应，也不进入指标标签。 |

信标使用独立的每来源限流账本，不消耗登录或业务 mutation 配额；拒绝计入 `auction_http_rate_limit_rejections_total{kind="browser_error"}`。`auction_browser_errors_total{class}` 只接受上述三类；`auction_browser_error_build_mismatches_total` 表示部署后仍在使用旧构建的页面。前端发送实现位于 `crates/auction-web-shared/src/bootstrap`。

数据库迁移模式默认 shadow，实际仍严格匹配；只有显式 compatible 才允许完整登记的兼容未来后缀，失败记录、缺项、未知等级或查询失败均不能放行。精确定义见[数据库契约](database.md#迁移兼容登记与就绪)。schema 子检查自身总预算 750ms，对 HTTP 缓存关闭、heartbeat 与 supervisor 也生效；审计与 relay 等其他条件仍参与最终结果。

Public/Admin 的 `/metrics` 默认拒绝任意来源携带 `Forwarded`、任何 `X-Forwarded-*`、`X-Real-IP` 或 `Via` 的请求，只判断头存在性，不解析或记录头值；空值、重复或非法值也拒绝。任何方法均返回空 404，不带 `Allow`，并附安全响应头、`Cache-Control: no-store` 与安全 request-id；该拒绝优先于 request-id 400、方法 405 和指标编码。CDN 专用来源头待 W0.12 核实实际代理链后再加入。

直连的其他方法保留空 405 与 `Allow: GET,HEAD`；GET/HEAD 在未启用、带 query 时返回 404，编码失败返回 500。未注册的路径变体沿用 fallback 的 404。路由使用 `any(...)` 注册并由 owner 显式检查方法，避免框架对拒绝响应补出 `Allow`。

部署层 `common.toml [metrics] reject_forwarded_requests` 默认 true，设为 false 并重启 Public/Admin 恢复旧访问行为；指标启用、query 与监听限制仍生效。Admin runtime 只覆盖 `metrics.enabled`，该安全开关不进入 runtime 文档、不改变文档指纹或命令摘要。配置见[配置参考](../../ops/config/README.md#commontoml)，Caddy 的公网 404 规则与应用收紧同时保留。

默认启用一秒进程缓存与并发单飞，只保存最终就绪布尔值；正负结果均缓存，过期后等待新检查，不继续返回过期成功。检查限时 750ms，超时、异常或刷新任务取消均产生未就绪；请求断开不取消共享刷新。`Cache-Control: no-store` 及 JSON 结构保持不变，缓存不由代理、CDN 或浏览器持有。开关见[就绪策略](../../ops/config/README.md#就绪策略)。

在持续探测、依赖检查可在预算内完成的条件下，缓存额外滞后至多 1 秒；真实条件在上一轮完成后变化，转换预算为 1 + 0.75 = 1.75 秒；变化发生在正在进行的检查中时，保守计入两轮检查，总计至多 2.5 秒，另加调用方探测间隔及调度/网络时间。无请求时没有后台刷新，下次请求才触发；不可重试或未知审计失败保持粘滞，需修复后受控重启。关闭缓存后不承诺这组时限，沿用逐次检查与 HTTP 请求预算。

探针每轮只读 `_sqlx_migrations`，校验完整迁移版本序列、成功状态及允许的 checksum；不读取其他角色心跳。Public relay 条件仍为最近 `max(4 × poll_interval_millis, 5 秒)` 内成功且未标记失败。心跳独立检查，不复用 HTTP 缓存。Worker control `/v1/health/ready` 仍只表示运行/排空状态，不等同于数据库心跳的 readiness。

## Public API

“会话”列：— 不需要；✓ 需要登录（Cookie `auction_public_session`）。

### 目录与内容

| 方法 | 路径 | 会话 | 查询参数 / 说明 |
| --- | --- | --- | --- |
| GET | `/api/v1/worlds` | — | 已启用的世界列表 |
| GET | `/api/v1/rules` | — | `lang`：规则文档（en、zh-CN、zh-TW、ja、ko） |
| GET | `/api/v1/blacklist` | — | `page`、`page_size` |
| GET | `/api/v1/compensation-pool` | — | `page`、`page_size`、`cursor`：已结算场次的补偿池批次（含结算策略与冲正净额），以及两个世界共用补偿池的当前余额 `balance` 与更新时间 `updated_at` |
| GET | `/api/v1/compensation-pool/{auction_id}/movements` | — | `page`、`page_size`：单场补偿账本流水 |
| GET | `/api/v1/compensation-pool/{auction_id}/lots` | — | `page`、`page_size`：单场已售 lot 的逐件结算明细（成交价、比例、税收、折扣需求、补偿、实付/所得、买家 `buyer_name` 与卖家 `seller_name`，取名口径同成交历史）与成交撤销冲正 |
| GET | `/api/v1/auctions` | — | `world`、`status`、`page`、`page_size`、`cursor` |
| GET | `/api/v2/auctions` | — | 同上，另含每场的论坛主题地址 |
| GET | `/api/v1/auctions/current` | — | `world`：当前（预览/进行中）场次 |
| GET | `/api/v1/auctions/{auction_id}/lots` | — | `category`、`equipment_group`（rare/fusion）、`max_level`、`include_unleveled`、`page`、`page_size` |
| GET | `/api/v1/lots/{lot_id}` | — | lot 详情（装备、价格、截止与领先状态；完整竞价历史另读 bid-log） |
| GET | `/api/v1/auctions/{auction_id}/bid-log` | — | `world`、`lot`、`page`、`page_size`、`cursor` |
| GET | `/api/v1/price-history` | — | `name`、`world`、`from`、`to`、`min_price`、`max_price`、`buyer`、`seller`、`sort`、`page`、`page_size`、`cursor` |
| GET | `/api/v1/registration-info` | — | 登录页注册说明用的 Bot 名单（`bot_players` 投影） |
| GET | `/api/v1/forum/sync-status` | — | 论坛同步开关与摘要 |
| GET | `/api/v1/auctions/{auction_id}/forum` | — | 该场论坛主题的公开镜像 |
| GET, HEAD | `/api/v1/auctions/{auction_id}/countdown/{timer}` | — | 论坛帖内嵌的倒计时 SVG 图片 |

### 会话与账户

| 方法 | 路径 | 会话 | 说明 |
| --- | --- | --- | --- |
| GET | `/api/v1/session` | — | 当前会话；未登录仍返回 200 / `ok`，`data: null`；不写 Cookie |
| POST | `/api/v1/session` | — | `{"key": "<登录 key>"}` 登录，成功后下发 HttpOnly Cookie；满额时在同一事务与用户行锁内注销最久未活动的有效会话，不返回 409 `session_limit` |
| PUT | `/api/v1/session` | ✓ | 空请求体，无查询参数；将当前服务端会话截止同步到浏览器 Cookie，不延长数据库截止 |
| DELETE | `/api/v1/session` | ✓ | 退出并撤销会话 |
| GET / PUT | `/api/v1/me/session-preference` | ✓ | 登录时长偏好：`{"command_id","duration_seconds"(null 为永久),"expected_version"}` |
| GET | `/api/v1/me/bids` | ✓ | `page`、`page_size`、`bid_page`、`proxy_page`：本人出价与代理 |
| GET | `/api/v1/me/lot-watches` | ✓ | `auction_id`：本人在该场的标记集合 |
| GET / PUT | `/api/v1/me/lot-watch-preference` | ✓ | 提醒渠道偏好：`{"command_id","channel":"mm"|"pm","expected_version"}` |
| PUT | `/api/v1/lots/{lot_id}/watch` | ✓ | 标记/取消标记：`{"command_id","watched","channel":"mm","expected_version"}`；`channel` 只是兼容占位，实际提醒渠道由账号偏好决定 |

写请求可附带 `x-auction-expected-user-id` 头：与当前 Cookie 身份不一致时拒绝，防止旧标签页在切换账号后误操作。

### 竞价与代理

| 方法 | 路径 | 会话 | 请求体 |
| --- | --- | --- | --- |
| POST | `/api/v1/lots/{lot_id}/bids` | ✓ | `{"command_id","amount"}` |
| GET | `/api/v1/lots/{lot_id}/proxy-authorizations/mine` | ✓ | 本人在该 lot 的代理与可执行能力 |
| POST | `/api/v1/lots/{lot_id}/proxy-authorizations` | ✓ | `{"command_id","maximum_amount","mode":"fast"|"slow"}` |
| POST | `/api/v1/lots/{lot_id}/proxy-authorizations/{authorization_id}/lower` | ✓ | `{"command_id","maximum_amount"}` |

竞价请求与响应字段保持不变。新建场次中，普通出价和新建代理的上限必须达到 `next_bid`；自动代理反击取当时 `next_bid` 与授权上限中的较小值，并且必须严格高于当前价，否则授权耗尽，合法的普通出价仍成功。等待中的代理降额能力仍受完整下一口约束，原上限已低于下一口时不能再降额。既有场次继续冻结的原策略，见[业务规则](../business-rules.md#代理竞价)。

竞价相关的业务错误（均为 409，除注明外）：

| code | 含义 |
| --- | --- |
| `bid_too_low` | 低于当前最低可出价 |
| `already_leading` | 已经领先，不能对自己加价 |
| `active_proxy_exists` | 已有生效代理，不能再普通出价或新建代理 |
| `auction_not_open` | 拍卖不在可竞价状态或已过截止 |
| `bidder_ineligible`（403） | 账号无竞价资格（如被拉黑） |
| `proxy_disabled` | 本场未启用代理 |
| `proxy_slow_final` | Final 阶段不能新建 Slow 代理 |
| `proxy_outbid_limit` | 本 lot 设置代理的次数已达上限 |
| `proxy_reentry_maximum_not_higher` | 重新设置的上限必须高于上一份 |
| `proxy_maximum_not_lower` / `proxy_maximum_below_binding` | 下调必须严格降低，且不低于当前约束价 |
| `proxy_authorization_not_active` | 代理已不在生效状态 |
| `invalid_amount` / `invalid_maximum_amount`（400） | 普通出价 / 代理上限非正数；正数但不满足领域金额范围或 1000 网格的输入沿用 v1 的 `503 temporarily_unavailable` |

### 实时

| 方法 | 路径 | 会话 | 说明 |
| --- | --- | --- | --- |
| GET | `/api/v1/events` | — | `auction_id` 或 `scope=auctions`（二选一）、`after`、`page_size`（1–200，默认 100）：持久化事件回放 |
| GET（Upgrade）/ CONNECT | `/api/v1/ws` | ✓ | `auction_id` 或 `scope=auctions`（只能其一）：WebSocket 推送 |

协议细节见[实时事件](realtime.md)。

## Admin API

除 `POST /api/v1/admin/session` 外全部需要 Admin 会话 Cookie `auction_admin_session` 与 CSRF Cookie `auction_admin_csrf`；写请求还须在 `x-csrf-token` 头回传相同 CSRF token。登录及当前会话 DTO 也返回 `csrf_token`，供浏览器持于内存。全部业务响应 `no-store`。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET / POST / DELETE | `/api/v1/admin/session` | 读取会话 / Basic 认证登录（actor:secret）/ 退出 |
| GET | `/api/v1/admin/dashboard` | 仪表盘聚合快照（健康、Worker、进程、邮件、realtime、凭据、投递、履约、恢复、告警） |
| GET | `/api/v1/admin/worlds` | 业务世界及启用状态 |
| PATCH | `/api/v1/admin/worlds/{world_id}` | 启用/停用业务世界（即时生效） |
| GET / POST | `/api/v1/admin/auctions` | 场次列表（`world_id`、`status`、`page`、`page_size`）/ 新建场次 |
| PATCH | `/api/v1/admin/auctions/{auction_id}/schedule` | 修改排期 |
| POST | `/api/v1/admin/auctions/{auction_id}/preview` | 进入预览 |
| POST | `/api/v1/admin/auctions/{auction_id}/open` | 开拍 |
| POST | `/api/v1/admin/auctions/{auction_id}/final-phase` | 进入 Final 阶段 |
| POST | `/api/v1/admin/auctions/{auction_id}/early-settlement` | 提前结算 |
| POST | `/api/v1/admin/auctions/{auction_id}/cancel` | 取消（仅开拍前） |
| GET | `/api/v1/admin/auctions/{auction_id}/lots` | lot 列表（`page`、`page_size`）；响应头 `x-auction-capability-manual-intake` 声明手工录入是否可用 |
| PATCH | `/api/v1/admin/lots/{lot_id}` | 草稿 lot：`withdraw` 撤下或 `set_start_price` 修改起拍价 |
| GET | `/api/v1/admin/lots/{lot_id}/proxy-events` | 代理事件历史（`page`、`page_size`、`cursor`） |
| POST | `/api/v1/admin/worlds/{world_id}/auction-intake/items` | 手工录入装备到该世界当前接收投稿的 Draft 或 Preview 场次；无接收目标或尚未到冻结的最早收件时刻时返回 `409 auction_intake_unavailable`；`source_kind=url` 的装备页面由 Worker control 代为读取（W1.10），Worker 不可用时返回 `503 equipment_source_unavailable`，外部访问暂停时返回 `409 external_access_paused` |
| POST | `/api/v1/admin/auction-forum/preview` | 论坛主题预览渲染 |
| GET | `/api/v1/admin/forum/sync-status` | 论坛同步状态 |
| GET | `/api/v1/admin/auctions/{auction_id}/forum` | 该场论坛主题与诊断的本地安全投影；刷新不请求论坛，也不触发或授权重写 |
| GET | `/api/v1/admin/auctions/{auction_id}/settlement-report` | 结算报告 |
| GET | `/api/v1/admin/auctions/{auction_id}/recovery-report` | 恢复报告，含 `payment_deadline_at`、`blocking_code` 与 `allowed_commands`；旧赛季装备的永久重列阻断对外映射为 `recovery_relist_unavailable` |
| GET | `/api/v1/admin/settlement-catalog` | 已结算场次目录（`world_id`、`sequence`、`page`、`page_size`） |
| POST | `/api/v1/admin/recoveries/{recovery_id}/confirm` | 人工确认恢复动作（带期望版本与原因） |
| GET | `/api/v1/admin/mail` | 邮箱轮询状态（`mailbox_key`、`include_diagnostics`） |
| GET | `/api/v1/admin/mail/intake` | 入站邮件处理记录（`mailbox_key`、`world_id`、`kind`、`status`、`decision`、`from`、`to`、`page`、`page_size`） |
| GET | `/api/v1/admin/mail/intake/{intake_id}` | 单封入站邮件详情 |
| PUT | `/api/v1/admin/mail/polling` | 暂停/恢复收件轮询 |
| POST | `/api/v1/admin/mail/polls:trigger` | 经 Worker control 立即触发一次轮询 |
| GET | `/api/v1/admin/deliveries` | 投递 outbox（`page`、`page_size`、`status`） |
| POST | `/api/v1/admin/deliveries/{delivery_id}/release` | 释放投递 |
| POST | `/api/v1/admin/deliveries/{delivery_id}/retry` | 重试失败投递 |
| POST | `/api/v1/admin/deliveries/{delivery_id}/reconcile` | 人工对账结果未知的投递（成功/失败/重试）；主动 Send 禁止仅凭手填外部引用确认成功。判失败的 `failure_code` 只接受 `verified_not_sent`、`recipient_invalid`、`no_longer_needed`；Worker 已在 Sent 目击同一结算义务主题时，重试与判失败返回 `409 delivery_sent_sighted` |
| GET | `/api/v1/admin/realtime/events` | 事件元数据（`stream_key`、`kind`、`page`、`page_size`） |
| GET | `/api/v1/admin/realtime/lag` | consumer 滞后（`stream_prefix`、`consumer_key`、`page`、`page_size`） |
| GET | `/api/v1/admin/realtime/failures`、`/api/v1/admin/realtime/failures/{failure_id}` | 转发失败列表（`stream_key`、`consumer_key`、`code`、`resolved`、`page`、`page_size`）/ 单条详情 |
| POST | `/api/v1/admin/realtime/failures/{failure_id}/replay` | 重放失败事件（追加新事件并解决原失败） |
| GET / POST / PUT | `/api/v1/admin/rules` | 读取（`lang`）/ 校验 / 发布规则文档 |
| GET / PUT | `/api/v1/admin/config` | 读取 / 更新 runtime 配置（带 fingerprint 与 `command_id`） |
| GET | `/api/v1/admin/runtime-switches` | 认证只读数据库意图与 Admin 自身实际快照，不依赖 runtime 文件 |
| POST | `/api/v1/admin/config:validate` | 只校验 runtime 配置候选 |
| GET / POST / PATCH | `/api/v1/admin/users` | 用户列表（`name`、`status`、`page`、`page_size`）/ 创建 / 修改（改名、签发/重签/吊销 key、绑定论坛 ID） |
| PUT / DELETE | `/api/v1/admin/users/{user_id}/blacklist` | 拉黑 / 解除 |
| GET | `/api/v1/admin/renames` | 改名历史（`user_id`、`name`、`page`、`page_size`） |
| POST | `/api/v1/admin/renames/{rename_id}/revert` | 回滚改名 |
| GET / POST / PATCH / DELETE | `/api/v1/admin/credentials` | 凭据列表（`kind`、`world`、`status`、`page`、`page_size`）/ 录入 Cookie 或以论坛账号密码获取并提取游戏 CK / 改名或启停 / 删除 |
| POST | `/api/v1/admin/credentials/{credential_id}/check` | 在线校验凭据 |
| GET | `/api/v1/admin/audit` | 审计记录（`actor`、`actor_type`、`action`、`entity`、`source`、`result`、`request_id`、`occurred_from`、`occurred_to`、`page`、`page_size`、`cursor`、`before_time`、`before_id`） |

请求体大小上限按路由冻结：多数命令 4–16 KiB，realtime 重放 64 KiB，凭据 128 KiB（在线校验 4 KiB），装备录入 72 KiB，规则 1 MiB，论坛预览 2 MiB，runtime 配置 4 MiB。

## 首次设置（auction-setup）

只在首次设置服务中存在，需请求头 `x-setup-token`（启动时打印的一次性令牌），请求体上限 8 KiB。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/api/v1/setup/verify` | 校验令牌 |
| POST | `/api/v1/setup/validate` | 校验表单并试连数据库 |
| POST | `/api/v1/setup/complete` | 生成并发布配置目录 |

浏览器入口为 `/setup`，仅另提供存在的 `/index.html`、`/favicon.ico` 与 `/assets/` 下资源；其余路径返回 404。首次设置使用独立的 `SetupResponse` / `SetupErrorResponse`，成功字段为 `status` 与 `initialize_and_start`；正式 Admin 不挂载这三个写接口。合并部署的 `complete` 还要求 `confirm_initialize = true`，完成后由监管进程初始化专用空库并启动角色。

## 三角色附加指标出口（loopback TCP）

Public/Admin/Worker 可在业务 listener、control socket 之外启用独立出口；
只提供 `GET / HEAD /metrics`，不挂载健康、控制、业务或 Web 资源路由。
地址由 common 的 `[metrics]` 三个 `*_loopback_listen_address` 字段指定，
省略即关闭；有效 `metrics.enabled = true` 且填写本角色地址才监听。
仅接受数值 `127.0.0.1` 与非零端口，三个地址不能重复，
也不能与本角色实际业务 TCP listener 冲突；已填写但未启用也必须合法。

新出口与 Public/Admin 业务 `/metrics` 使用同一 HTTP boundary 和渲染函数：
默认携带转发头的任意方法返回空 404，不带 `Allow`；拒绝优先于 request-id 400。
直连 GET/HEAD 返回 Prometheus 文本，HEAD 无正文；带 query 返回 404；
直连其他方法返回空 405 与 `Allow: GET,HEAD`，编码失败返回 500。
转发头集合、安全响应头及关联标识沿用
[Public/Admin 指标契约](#健康与指标publicadmin)。
新出口未知路径返回 404；请求标识等通用 HTTP 校验仍生效。

三角色新出口均使用 common 的 `reject_forwarded_requests`；
Admin runtime 仍只覆盖 enabled，不覆盖地址或拒绝策略。
绑定/服务失败仅以固定类别 ERROR 降级采集，不改变业务 readiness。
删除对应地址并重启即关闭新出口，原 listener/control 的指标路径保留。
Worker control 继续按原 peer UID/GID 授权，不使用新出口的转发拒绝策略。
示例、安装和 SSH 隧道见[运维手册](../../ops/README.md#指标)。

## Worker control（Unix socket）

只在 Worker 的 control socket 上提供，按 peer UID/GID allowlist 授权，见 [auction-jobs](../../crates/auction-jobs/README.md#worker-control)。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/v1/health/live` | 存活 |
| GET | `/v1/health/ready` | 调度运行时已开始服务且未排空 |
| GET | `/v1/status` | 版本与轮询器数量等安全状态 |
| GET | `/v1/mail/progress` | 各邮箱轮询进度 |
| POST | `/v1/mail/polls:trigger` | 触发一次邮件轮询 |
| POST | `/v1/equipment/reads` | W1.10：Admin 手工装备 URL 读取代为执行；请求只含 `world_id` 与公开装备链接 |
| POST | `/v1/mail/sent:lookup` | W1.10：CLI `mail inspect-sent` 的 Sent 只读核查代为执行 |
| GET | `/metrics` | Worker 指标 |

两个远端读取路由在 Worker 未装配 MoogleMail 完整能力时返回 `501 capability_unavailable`，
W1.9 主动暂停期间返回 `409 paused`（请求不发往远端），超时为 `504`；单次读取仍受 Worker
自己的 fetch 预算约束，Admin/CLI 不得回退到本地直连。

## 浏览器页面

只有下列路径返回 SPA 的 `index.html`；其他未知路径返回 404。`web-embed` 资源缺失时页面路径返回 503。

| Public 路径 | 页面 |
| --- | --- |
| `/`、`/auction` | 当前拍卖看板 |
| `/auctions` | 历史拍卖 |
| `/lots/{lot_code}` | lot 详情 |
| `/bidlog` | 出价日志 |
| `/history` | 成交价格历史 |
| `/login` | 登录 |
| `/me`、`/account` | 我的 |
| `/rules` | 规则 |
| `/blacklist` | 黑名单 |
| `/compensation-pool`、`/compensation` | 补偿池 |
| `/forum` | 论坛同步 |
| `/status` | 服务说明 |

| Admin 路径 | 页面 |
| --- | --- |
| `/`（别名 `/dashboard`、已登录时的 `/login`） | 仪表盘 |
| `/auctions`（别名 `/auction`） | 拍卖场次 |
| `/auctions/{auction_id}/lots` | lot 列表 |
| `/mails`（别名 `/mail`） | 收件审核 |
| `/cookies`（别名 `/credentials`） | Cookie 池 |
| `/users` | 用户与 Key |
| `/renames` | 改名回滚 |
| `/deliveries`（别名 `/delivery`） | 投递队列 |
| `/outbox` | 实时事件 |
| `/settlement` | 结算报表 |
| `/config` | 参数配置 |
| `/rules-content`（别名 `/rules`） | 规则内容 |
| `/audit` | 审计日志 |
| `/status` | 服务说明 |

带参数的路径段不得为空、`.`、`..`，不得含控制字符、`/`、`\` 或 `%`。

### 运行时开关只读接口

W0.13 在 database_intent 中增加 Worker 专用 bool `metrics_sli_snapshot_enabled`；
W7.7 第二批再增加 Worker 专用 `mooglemail_poll_isolation_enabled` 和三角色
`metrics_reject_forwarded_requests`；W1.9 增加三角色 `external_access_paused`，共六个已知意图。admin_applied 只含当前
Admin 消费的项，数量不固定且不要求与 database_intent 等长；前端按名称闭集
分别校验两个数组，并要求 admin_applied 是 database_intent 的名称子集。
复用现有只读卡片，不新增路由或 JSON 字段；更新后须刷新旧 Admin 页面。
Worker 实际值应核对其指标，不能把 Admin 显示的数据库意图当作已应用证明。
基线和在线覆盖规则见[配置参考](../../ops/config/README.md#数据库运行时开关)。

`GET /api/v1/admin/runtime-switches` 使用既有 Admin 限流与会话守卫，仅返回读取投影；无写入口。成功为 200，返回 `database_time`、`database_intent`（名称、revision、最新动作/值/期限、actor_id、entrypoint、reason）、`admin_last_success_age_seconds`（从未成功为 null）及 `admin_applied`（名称、revision、TOML 基线、实际值、是否覆盖）。数据库意图是本次限时一秒查询，Admin 实际值来自后台五秒刷新快照，二者可能暂时不同；接口不证明 Public/Worker 已应用。无效认证沿用 401，限流沿用 429，缺少能力或读取失败为 503。认证后的成功与存储失败均带 `Cache-Control: no-store`。页面继续使用 `/config`，没有新增 SPA 路由。
