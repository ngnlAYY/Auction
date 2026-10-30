# HTTP 路由契约

[文档导航](docs/README.md) · [实时事件](docs/contracts/realtime.md) · [数据库](docs/contracts/database.md) · [auction-http](crates/auction-http/README.md) · [DTO](crates/auction-contracts/README.md)

本页列出各进程对外提供的全部 HTTP 路径。路由表的权威来源是 [`crates/auction-http/src/http/routes/surfaces.rs`](crates/auction-http/src/http/routes/surfaces.rs)（API）与 [`crates/auction-http/src/assets/routes.rs`](crates/auction-http/src/assets/routes.rs)（浏览器页面）；增删路径时三处必须同步修改。

## 通用约定

### 响应外壳

```json
{"code": "ok", "message": "…", "data": { … }}
{"code": "bid_too_low", "message": "…", "data": null, "request_id": "…"}
```

- `code` 是稳定的机器可读值，客户端只依据 HTTP 状态与 `code` 分支，不解析 `message`（中文提示，可能调整）。创建类成功返回 HTTP 201 与 `code = "created"`。
- 错误响应带 `request_id`（来自请求头 `x-request-id`，没有则生成），便于日志关联。
- 时间为 RFC 3339 字符串；金额为整数 Credits；标识为小写 UUID 字符串。

### 通用错误码

| HTTP | code | 场景 |
| --- | --- | --- |
| 400 | `invalid_query`、`invalid_json`、`invalid_*` | 查询参数不在白名单或格式错误；请求体不是合法 JSON、含未知字段或字段非法 |
| 401 | `unauthenticated` | 会话缺失或过期 |
| 403 | `origin_forbidden`、`bidder_ineligible` 等 | origin 不在 allowlist；无竞价资格 |
| 404 | `not_found` | 资源不存在或对当前调用者不可见 |
| 409 | `command_conflict` 及各业务冲突码 | 同一 `command_id` 携带不同请求；版本不匹配；业务规则拒绝 |
| 405 | `method_not_allowed`（带 `Allow`） | 路径存在但方法不受支持 |
| 413 / 415 | `body_too_large` / `unsupported_media_type` | 请求体超过路由上限；写接口非 JSON 内容类型 |
| 429 | `rate_limited`（带 `Retry-After`）、`realtime_busy`、`discovery_busy` | 速率或连接数超限 |
| 501 | `feature_not_supported` | 对应能力未装配（如未配置数据密钥、无数据库） |
| 503 | `storage_unavailable`、`temporarily_unavailable` | 依赖暂不可用，可稍后重试 |

查询参数一律白名单解析：出现未声明的参数即返回 `invalid_query`。写接口严格解析 JSON（拒绝未知字段），并对 HTTP 方法严格匹配。

### 写命令

所有改变状态的业务请求都带客户端生成的 `command_id`（UUID）。服务端以“命令种类 + 聚合 + 请求摘要”记账：同一 ID 同一请求重放原结果；同一 ID 不同请求返回 `409 command_conflict`。客户端在结果不确定（超时、断网）时必须用**同一 ID 与同一请求体**重试。多数写命令还要求携带服务端投影给出的期望版本（`expected_version`、`expected_state_version`、`expected_fencing_version`），版本不匹配时返回 409，需重新读取后再决定。

### 缓存头

公开目录类读取（世界、规则、黑名单、补偿池汇总、拍卖列表、当前拍卖、竞价日志、价格历史）返回 `Cache-Control: no-cache`；含会话、装备 key 或命令结果的响应以及全部 Admin 接口返回 `no-store`。

## 健康与指标（Public、Admin）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/health/live` | 进程存活：`{"data":{"live":true}}` |
| GET | `/health/ready` | 就绪：Public 要求数据库与 realtime relay 新鲜；Admin 要求数据库就绪。未就绪返回 503 |
| GET | `/health`、`/api/v1/health` | 服务名、状态与运行进程版本 |
| GET | `/metrics` | Prometheus 文本；需启用 metrics，且 listener 为 loopback TCP 或 Unix socket |

## Public API

“会话”列：— 不需要；✓ 需要登录（Cookie `auction_public_session`）。

### 目录与内容

| 方法 | 路径 | 会话 | 查询参数 / 说明 |
| --- | --- | --- | --- |
| GET | `/api/v1/worlds` | — | 已启用的世界列表 |
| GET | `/api/v1/rules` | — | `lang`：规则文档（en、zh-CN、zh-TW、ja、ko） |
| GET | `/api/v1/blacklist` | — | `page`、`page_size` |
| GET | `/api/v1/compensation-pool` | — | `page`、`page_size`：已结算场次的补偿池批次 |
| GET | `/api/v1/compensation-pool/{auction_id}/movements` | — | `page`、`page_size`、`cursor`：单场补偿账本流水 |
| GET | `/api/v1/auctions` | — | `world`、`status`、`page`、`page_size`、`cursor` |
| GET | `/api/v2/auctions` | — | 同上，另含每场的论坛主题地址 |
| GET | `/api/v1/auctions/current` | — | `world`：当前（预览/进行中）场次 |
| GET | `/api/v1/auctions/{auction_id}/lots` | — | `category`、`equipment_group`（rare/fusion）、`max_level`、`include_unleveled`、`page`、`page_size` |
| GET | `/api/v1/lots/{lot_id}` | — | lot 详情（装备投影、价格、截止、竞价历史） |
| GET | `/api/v1/auctions/{auction_id}/bid-log` | — | `world`、`lot`、`page`、`page_size`、`cursor` |
| GET | `/api/v1/price-history` | — | `name`、`world`、`from`、`to`、`min_price`、`max_price`、`buyer`、`seller`、`sort`、`page`、`page_size`、`cursor` |
| GET | `/api/v1/registration-info` | — | 登录页注册说明用的 Bot 名单（`bot_players` 投影） |
| GET | `/api/v1/forum/sync-status` | — | 论坛同步开关与摘要 |
| GET | `/api/v1/auctions/{auction_id}/forum` | — | 该场论坛主题的公开镜像 |
| GET, HEAD | `/api/v1/auctions/{auction_id}/countdown/{timer}` | — | 论坛帖内嵌的倒计时 SVG 图片 |

### 会话与账户

| 方法 | 路径 | 会话 | 说明 |
| --- | --- | --- | --- |
| GET | `/api/v1/session` | — | 当前会话；未登录返回 401 |
| POST | `/api/v1/session` | — | `{"key": "<登录 key>"}` 登录，成功后下发 HttpOnly Cookie |
| PUT | `/api/v1/session` | ✓ | 按账号的登录时长偏好刷新浏览器 Cookie 期限 |
| DELETE | `/api/v1/session` | ✓ | 退出并撤销会话 |
| GET / PUT | `/api/v1/me/session-preference` | ✓ | 登录时长偏好：`{"command_id","duration_seconds"(null 为永久),"expected_version"}` |
| GET | `/api/v1/me/bids` | ✓ | `page`、`page_size`、`bid_page`、`proxy_page`：本人出价与代理 |
| GET | `/api/v1/me/lot-watches` | ✓ | `auction_id`：本人在该场的标记集合 |
| GET / PUT | `/api/v1/me/lot-watch-preference` | ✓ | 提醒渠道偏好：`{"command_id","channel":"mm"|"pm","expected_version"}` |
| PUT | `/api/v1/lots/{lot_id}/watch` | ✓ | 标记/取消标记：`{"command_id","watched","channel","expected_version"}` |

写请求可附带 `x-auction-expected-user-id` 头：与当前 Cookie 身份不一致时拒绝，防止旧标签页在切换账号后误操作。

### 竞价与代理

| 方法 | 路径 | 会话 | 请求体 |
| --- | --- | --- | --- |
| POST | `/api/v1/lots/{lot_id}/bids` | ✓ | `{"command_id","amount"}` |
| GET | `/api/v1/lots/{lot_id}/proxy-authorizations/mine` | ✓ | 本人在该 lot 的代理与可执行能力 |
| POST | `/api/v1/lots/{lot_id}/proxy-authorizations` | ✓ | `{"command_id","maximum_amount","mode":"fast"|"slow"}` |
| POST | `/api/v1/lots/{lot_id}/proxy-authorizations/{authorization_id}/lower` | ✓ | `{"command_id","maximum_amount"}` |

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
| `invalid_amount`（400） | 金额不是 1000 的正整数倍或超过上限 |

### 实时

| 方法 | 路径 | 会话 | 说明 |
| --- | --- | --- | --- |
| GET | `/api/v1/events` | — | `auction_id` 或 `scope=auctions`（二选一）、`after`、`page_size`（1–200，默认 100）：持久化事件回放 |
| GET（Upgrade）/ CONNECT | `/api/v1/ws` | ✓ | `auction_id` 或 `scope=auctions`（只能其一）：WebSocket 推送 |

协议细节见[实时事件](docs/contracts/realtime.md)。

## Admin API

除 `POST /api/v1/admin/session` 外全部需要 Admin 会话（Cookie `auction_admin_session`）；写请求还须在 `x-csrf-token` 头回传 CSRF token（Cookie `auction_admin_csrf`）。全部响应 `no-store`。

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
| POST | `/api/v1/admin/worlds/{world_id}/auction-intake/items` | 手工录入装备到该世界的草稿场次 |
| POST | `/api/v1/admin/auction-forum/preview` | 论坛主题预览渲染 |
| GET | `/api/v1/admin/forum/sync-status` | 论坛同步状态 |
| GET | `/api/v1/admin/auctions/{auction_id}/forum` | 该场论坛主题与诊断 |
| GET | `/api/v1/admin/auctions/{auction_id}/settlement-report` | 结算报告 |
| GET | `/api/v1/admin/auctions/{auction_id}/recovery-report` | 恢复报告 |
| GET | `/api/v1/admin/settlement-catalog` | 已结算场次目录（`world_id`、`sequence`、`page`、`page_size`） |
| POST | `/api/v1/admin/recoveries/{recovery_id}/confirm` | 人工确认恢复动作（带期望版本与原因） |
| GET | `/api/v1/admin/mail` | 邮箱轮询状态（`mailbox_key`、`include_diagnostics`） |
| GET | `/api/v1/admin/mail/intake` | 入站邮件处理记录（`page`、`page_size`） |
| GET | `/api/v1/admin/mail/intake/{intake_id}` | 单封入站邮件详情 |
| PUT | `/api/v1/admin/mail/polling` | 暂停/恢复收件轮询 |
| POST | `/api/v1/admin/mail/polls:trigger` | 经 Worker control 立即触发一次轮询 |
| GET | `/api/v1/admin/deliveries` | 投递 outbox（`page`、`page_size`、`status`） |
| POST | `/api/v1/admin/deliveries/{delivery_id}/release` | 释放投递 |
| POST | `/api/v1/admin/deliveries/{delivery_id}/retry` | 重试失败投递 |
| POST | `/api/v1/admin/deliveries/{delivery_id}/reconcile` | 人工对账结果未知的投递（成功/失败/重试） |
| GET | `/api/v1/admin/realtime/events` | 事件元数据（`stream_key`、`kind`、`page`、`page_size`） |
| GET | `/api/v1/admin/realtime/lag` | consumer 滞后（`stream_prefix`、`consumer_key`、`page`、`page_size`） |
| GET | `/api/v1/admin/realtime/failures`、`…/failures/{failure_id}` | 转发失败记录 |
| POST | `/api/v1/admin/realtime/failures/{failure_id}/replay` | 重放失败事件（追加新事件并解决原失败） |
| GET / POST / PUT | `/api/v1/admin/rules` | 读取（`lang`）/ 校验 / 发布规则文档 |
| GET / PUT | `/api/v1/admin/config` | 读取 / 更新 runtime 配置（带 fingerprint 与 `command_id`） |
| POST | `/api/v1/admin/config:validate` | 只校验 runtime 配置候选 |
| GET / POST / PATCH | `/api/v1/admin/users` | 用户列表（`name`、`status`、`page`、`page_size`）/ 创建 / 修改（改名、签发/重签/吊销 key、绑定论坛 ID） |
| PUT / DELETE | `/api/v1/admin/users/{user_id}/blacklist` | 拉黑 / 解除 |
| GET | `/api/v1/admin/renames` | 改名历史（`user_id`、`name`、`page`、`page_size`） |
| POST | `/api/v1/admin/renames/{rename_id}/revert` | 回滚改名 |
| GET / POST / PATCH / DELETE | `/api/v1/admin/credentials` | 凭据列表（`kind`、`world`、`status`、`page`、`page_size`）/ 录入 / 改名或启停 / 删除 |
| POST | `/api/v1/admin/credentials/{credential_id}/check` | 在线校验凭据 |
| GET | `/api/v1/admin/audit` | 审计记录（`actor`、`actor_type`、`action`、`entity`、`source`、`result`、`request_id`、`occurred_from`、`occurred_to`、`page`、`page_size`、`cursor`、`before_time`、`before_id`） |

请求体大小上限按路由冻结：多数命令 4–16 KiB，凭据 128 KiB，装备录入 72 KiB，规则 1 MiB，论坛预览 2 MiB，runtime 配置 4 MiB。

## 首次设置（auction-setup）

只在首次设置服务中存在，需请求头 `x-setup-token`（启动时打印的一次性令牌），请求体上限 8 KiB。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/api/v1/setup/verify` | 校验令牌 |
| POST | `/api/v1/setup/validate` | 校验表单并试连数据库 |
| POST | `/api/v1/setup/complete` | 生成并发布配置目录 |

其余路径返回 Admin 页面资源，浏览器入口为 `/setup`。

## Worker control（Unix socket）

只在 Worker 的 control socket 上提供，按 peer UID/GID allowlist 授权，见 [auction-jobs](crates/auction-jobs/README.md#worker-control)。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/v1/health/live` | 存活 |
| GET | `/v1/health/ready` | 调度运行时已开始服务且未排空 |
| GET | `/v1/status` | 版本与轮询器数量等安全状态 |
| GET | `/v1/mail/progress` | 各邮箱轮询进度 |
| POST | `/v1/mail/polls:trigger` | 触发一次邮件轮询 |
| GET | `/metrics` | Worker 指标 |

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
