# HTTP 路由与响应契约

[文档导航](docs/README.md) · [路由注册](crates/auction-http/src/http/routes/surfaces.rs) · [稳定 DTO](crates/auction-contracts/src/contracts) · [浏览器入口](crates/auction-http/src/assets/routes.rs)

路由、方法和状态码的冻结清单；请求/响应 DTO 与 handler 必须同步维护。

## 本页导航

- [基线与统计口径](#基线与统计口径)
- [进程健康探针](#进程健康探针)
- [通用响应与错误](#通用响应与错误)
- [Public](#public)
- [Admin](#admin)
- [首次部署初始化](#首次部署初始化)
- [已实现路径的补充约束](#已实现路径的补充约束)
- [契约变更规则](#契约变更规则)

## 基线与统计口径

`auction-http` 拥有路径注册，`auction-contracts` 拥有稳定 DTO。feature API 现为 Public **25** 个、Admin **46** 个唯一路径；多方法共用路径只计一次，P25/P26 共用账号偏好路径，不计健康、静态资源或代理路径。仅支持 PostgreSQL；已发布路径、字段、错误码和未装配能力的错误处理必须保留，运行证据见[验证状态](docs/status.md)。

首次部署另有独立进程的 `/setup`、部署码验证接口和两个配置接口，不计入 feature API；正式 Admin 不注册它们，`/setup` 在正式静态入口返回 404。

进程级可观测路径不计入统计：Public 与 Admin 在主配置 `[metrics].enabled = true` 时提供 `GET/HEAD /metrics` Prometheus text scrape，Admin 配置 `runtime_config_file` 时以 runtime 的 `[metrics].enabled` 为准；指标关闭、带 query 或路径变体均 404。Worker 不开启 TCP HTTP 监听；启用 control 时同一经 peer credential 校验的 Unix socket 提供 `GET/HEAD /metrics` 并使用同一 registry，未启用或指标关闭时 404。

OTLP/HTTP exporter 不是业务路由：Public/Worker 读主配置 `[tracing]`，Admin 配置 runtime 时读其 `[tracing]`；默认关闭，启用后使用有界异步队列，OTLP headers 只在敏感 TOML。SIGINT/SIGTERM 先触发 10 秒业务/HTTP drain，再按配置刷新 exporter，第二次信号强制收口。

## 进程健康探针

不计入 feature API 数量，两个 HTTP surface 都提供：

| 方法 | 路径 | 成功 | 未就绪 | 数据 |
| --- | --- | --- | --- | --- |
| GET/HEAD | /health/live | 200 | — | `{"live": true}` |
| GET/HEAD | /health/ready | 200 | 503 | `{"ready": true/false}` |
| GET/HEAD | /health、/api/v1/health | 200 | — | `HealthResponse`：status、service、version |

live/ready 均用统一 `{code,message,data}` 外壳。`/health/live` 不访问下游；`/health/ready` 经应用层 readiness 端口只读检查当前二进制的必需依赖，Public 配置 realtime relay 时还检查 relay 新鲜度；数据库未配置、迁移不完整或 relay 停滞返回 503。Admin 以数据库连接和全部 SQLx 迁移为 readiness 边界。

## 通用响应与错误

`/health` 与 `/api/v1/health` 是兼容的进程信息入口，返回 status/service/version，不能替代 readiness。除 P22 成功返回动态 SVG 外，业务 API 使用统一外壳（P22 错误也用此 JSON）：

    {"code": "...", "message": "...", "data": ...}

业务 code、字段与错误 code 由本表及 DTO 共同冻结。504 表示处理期限耗尽，503 表示其他依赖不可用，未知路径/方法返回 404/405，SPA 仅允许显式浏览器路由。

| 状态码 | 含义 |
| --- | --- |
| 200 | 读取或已完成的命令 |
| 201 | 创建命令已完成 |
| 202 | 已接受、由 Worker 异步处理 |
| 400 | 查询、路径、JSON、命令或分页参数无效 |
| 401 | 未认证或会话失效 |
| 403 | origin、CSRF、资格或权限拒绝 |
| 404 | 资源不存在或公开边界不允许暴露 |
| 409 | 状态版本、命令幂等或业务条件冲突 |
| 413/415 | 请求体过大或 Content-Type 不支持 |
| 422 | P24 标记或 P26 账号警告偏好的业务输入无效 |
| 429 | 限流或有界连接资源耗尽 |
| 500 | 服务器内部无法安全完成请求 |
| 501 | 当前部署档位或端口不提供该能力 |
| 503/504 | 下游依赖不可用或处理超时 |

### 严格 JSON 与方法闭集

请求必须使用 `application/json` 媒体类型（允许合法参数），body 受每条路由自己的上限约束，解码只接受一个 JSON 值，请求 DTO 拒绝未知字段。非 JSON、超限、语法/未知字段/尾随值分别映射 415 `unsupported_media_type`、413 `body_too_large`、400 `invalid_json`；A24 replay 保留 `invalid_replay`。业务错误固定含 `data`，无业务数据时为 `null`，可附加安全 `request_id`；初始化 API 使用独立错误 DTO。

方法是严格闭集：GET 不因 Axum 默认行为额外接受 HEAD；未显式注册的方法返回 JSON `405 method_not_allowed`，`Allow` 只列该路径真实注册的方法并使用冻结排序。健康探针、metrics、P22 与静态文件的 HEAD 由各自契约决定。

### 请求边界

Public/Admin 统一 boundary 只接受 1 至 64 字节的 ASCII 字母数字/`-_.` `X-Request-ID`，缺失时生成并在响应回传，重复或非法值返回 `400 invalid_request_id`；同一值进入 Public 竞价/代理、Admin 生命周期和管理 mutation 的追加式审计，不参与命令请求哈希。边界还提取 W3C trace context、记录访问日志、隔离 panic、设置安全响应头、执行默认 15 秒处理超时和固定 5 秒 HTTP/1 请求头读取超时，`/api/v1/ws` 不受处理超时限制。

客户端地址只在 TCP peer 命中配置的规范 CIDR，或 Public/Admin Unix listener 显式启用可信上游时解析 `X-Forwarded-For`；非法链整体丢弃，Unix 连接不伪造 TCP peer，Unix 信任开关不放宽真实 TCP peer，地址链从右向左跳过受信代理。`CF-Connecting-IP`、`Forwarded`、`X-Forwarded-Proto` 不用于推导客户端地址、Origin 或 Secure Cookie；Cloudflare 经 Caddy 清洗为可信 XFF 后交给应用，见[部署约束](ops/README.md#cloudflare-整站代理)。CSP 只允许同源脚本及 WASM 编译，内联样式例外仅用于 Dioxus 首屏防闪样式和组件 style 属性。登录/变更限流器按该地址工作，最多保留 4096 项并确定性淘汰最久未访问项。

Public 浏览器的已认证写请求（P06 DELETE、P18–P20、P24、P26）携带可选 `X-Auction-Expected-User-Id`，
值为确认操作所属用户的规范非零 UUID。HTTP 仍只从当前 Cookie 获取权威身份；认证成功后、执行写命令前核对该前置条件，
不匹配返回 `409 session_changed`，非法或重复头返回 `400 invalid_expected_user_id`，均不执行写入。
未提供该头的既有 v1 客户端保持原语义，JSON DTO 与命令摘要不变。浏览器登录/退出只广播随机通知值，
其他标签页收到通知或页面恢复可见时重新读取会话；业务 401 或 `session_changed` 也触发重读，换号后销毁旧用户的确认状态与命令宿主。

### 静态入口与缓存

未注册的 `/api`、`/health` 保留路径固定返回 `404 not_found` JSON；其他未知请求在 `Accept` 明确允许 HTML 时返回最小页面 404，否则返回同一 JSON 404。静态文件和批准的浏览器路径只接受 GET/HEAD，其他方法返回带 `Allow: GET, HEAD` 的 JSON 405。Public 15 条（含独立 `/forum` 页面）、Admin 21 条浏览器路由由两个 Dioxus `Route` 枚举与 `auction-http/src/assets/routes.rs` 同步维护，只有这些路径可读 `index.html`；批准路径缺构建产物时返回 503。
精确静态文件查找拒绝空段、`.`、`..`、反斜线及重复斜线，不将路径规范化后再选文件或授予 immutable。
百分号编码仍按原始路径查找；开发态资源必须来自受信目录，正式 payload 的符号链接禁令保持不变。

缓存：Public 普通公开读取（含 P15 forum）为 `no-cache`；P14 补偿流水、P21 `/forum/sync-status`、events/session/lot 列表与详情/账户/竞价/代理/标记及全部 Admin 业务接口为 `no-store`，错误响应沿用相同策略；浏览器路由入口、直接 `index.html` 和 HTML 资源为 `no-store`；直接静态文件中只有 `assets/` 非 HTML 指纹资源使用一年 `immutable`，`wasm/*.js`、`*.wasm`、稳定别名 `/favicon.ico` 和其他稳定文件名使用 `no-cache`、`Last-Modified` 与 `If-Modified-Since`；未知 404/静态 405/产物缺失 503 均为 `no-store`。

正式 Public/Admin 边界另统一发送 `CDN-Cache-Control` 与 `Cloudflare-CDN-Cache-Control`：默认 `no-store`，包括所有 API（即使浏览器策略为 `no-cache`）、登录、管理/私有数据、探针、错误和 WebSocket 握手；仅精确静态 handler 的私有响应标记允许 CDN 沿用该资源的缓存策略，不能凭路径后缀或 API 的 `public` 头放行。未自行声明浏览器缓存策略的边界错误等响应补 `Cache-Control: no-store`。首次设置静态入口也固定 CDN `no-store`，不纳入正式站点 CDN 白名单；边缘必须尊重源站头，不能配置 Cache Everything/强制 Edge TTL，静态白名单与上线限制见[运维契约](ops/README.md#cloudflare-整站代理)。

## Public

| 编号 | 方法 | 路径 | 认证/边界 | 成功 | 明确错误状态 |
| --- | --- | --- | --- | --- | --- |
| P01 | GET | /api/v1/worlds | 公开 | 200 | 400, 503/504 |
| P02 | GET | /api/v1/rules | 公开；`lang` 可选 | 200 | 400, 503 |
| P03 | GET | /api/v1/events | 公开读取；由 auction boundary 校验；从持久化事件表回放 | 200 | 400, 404, 503/504 |
| P04 | GET/CONNECT | /api/v1/ws | Public session；只读服务端推送；连接范围必须明确 | HTTP/1.1 101；HTTP/2 200 | 400, 401, 403, 404, 429, 503/504 |
| P05 | GET | /api/v1/blacklist | 公开；页码分页 | 200 | 400, 503/504 |
| P06 | POST/GET/DELETE | /api/v1/session | POST 登录；GET/DELETE 会话边界 | 200 | 400, 401, 403, 409, 429, 503/504 |
| P07 | GET | /api/v1/auctions | 公开 | 200 | 400, 503/504 |
| P08 | GET | /api/v1/auctions/current | 公开 | 200 | 400, 503/504 |
| P09 | GET | /api/v1/auctions/{auction_id}/lots | 公开；PostgreSQL 直接返回完整装备 URL；响应 `no-store` | 200 | 400, 404, 503/504 |
| P10 | GET | /api/v1/auctions/{auction_id}/bid-log | 公开 | 200 | 400, 404, 503/504 |
| P11 | GET | /api/v1/lots/{lot_id} | 公开；PostgreSQL 直接返回完整装备 URL；响应 `no-store` | 200 | 400, 404, 503/504 |
| P12 | GET | /api/v1/price-history | 公开；成交事实仅由 PostgreSQL 完整档提供 | 200 | 400, 501, 503/504 |
| P13 | GET | /api/v1/compensation-pool | 公开 | 200 | 400, 501, 503/504 |
| P14 | GET | /api/v1/compensation-pool/{auction_id}/movements | 公开；完整装备 URL；响应 `no-store` | 200 | 400, 404, 501, 503/504 |
| P15 | GET | /api/v1/auctions/{auction_id}/forum | 公开 | 200 | 400, 404, 501, 503/504 |
| P16 | GET | /api/v1/me/bids | Public session | 200 | 400, 401, 503/504 |
| P17 | GET | /api/v1/lots/{lot_id}/proxy-authorizations/mine | Public session | 200 | 400, 401, 404, 503/504 |
| P18 | POST | /api/v1/lots/{lot_id}/proxy-authorizations | Public session、origin、限流 | 200 | 400, 401, 403, 409, 413/415, 429, 503/504 |
| P19 | POST | /api/v1/lots/{lot_id}/proxy-authorizations/{authorization_id}/lower | Public session、origin、限流 | 200 | 400, 401, 403, 409, 413/415, 429, 503/504 |
| P20 | POST | /api/v1/lots/{lot_id}/bids | Public session、origin、限流 | 200 | 400, 401, 403, 409, 413/415, 429, 503/504 |
| P21 | GET | /api/v1/forum/sync-status | 公开；不接受 query；响应 `no-store` | 200 | 400, 501, 503/504 |
| P22 | GET/HEAD | /api/v1/auctions/{auction_id}/countdown/{timer} | 公开拍卖边界；`timer` 仅 `list.svg` / `auction.svg`；禁止 query 和缓存 | 200 SVG | 400, 404, 503/504 |
| P23 | GET | /api/v1/me/lot-watches | Public session；必填 `auction_id`；本人整场标记集合；`no-store` | 200 | 400, 401, 404, 503/504 |
| P24 | PUT | /api/v1/lots/{lot_id}/watch | Public session、origin、限流；版本 CAS；`no-store` | 200 | 400, 401, 403, 404, 409, 413/415, 422, 429, 503/504 |
| P25 | GET | /api/v1/me/lot-watch-preference | Public session；无 query；账号警告渠道；`no-store` | 200 | 400, 401, 503/504 |
| P26 | PUT | /api/v1/me/lot-watch-preference | Public session、origin/CSRF、限流；无 query；独立版本 CAS；`no-store` | 200 | 400, 401, 403, 409, 413/415, 422, 429, 503/504 |

### P23/P24 装备标记与超价提醒

P23 只接受指定公开拍卖的 `auction_id`，用户身份来自会话，不接受 `user_id` 或客户端身份覆盖。`PublicLotWatchesResponse` 返回 `{user_id, auction_id, watches}`；集合含取消标记的版本墓碑，至多 10000 条，超限显式失败，不得截断后将缺项解释为未标记。`watches` 每项为 `PublicLotWatchResponse {lot_id, watched, channel, version}`，持久化版本为正 `i64`；无持久记录才由缺项表示 `watched=false`、`version=0`。UUID 用字符串，不暴露其他用户标记；非法 UUID、缺失/重复/未知查询参数 400；被活动场次遮蔽的 preview 不可读取或新写入；浏览器直接解析 JSON 整数为 i64，不限制为 JS 安全整数或经浮点转换。

P24 的 `PublicSetLotWatchRequest` 必填 `command_id`、`watched`、`channel`、`expected_version`，拒绝未知字段；`expected_version` 为非负整数，`channel` 严格只接受 `mm` 历史兼容占位，不表示实际通知方式，也不覆盖账号偏好；旧账本/请求摘要/响应形状不变。成功返回 `PublicSetLotWatchResponse {user_id, auction_id, watch}`，不改变竞价/代理响应或公开实时事件。同命令同负载返回原响应，不同负载或版本冲突 409；业务输入/不可用渠道 422，未装配存储 503。路径/command UUID、查询词法、JSON 词法及未知字段错误 400；负 expected_version 与未启用渠道等业务输入 422；身份、Origin、限流、请求体及方法闭集沿用通用边界。取消保留版本，不同新命令即使设置相同值也须阻止更早的未知写入晚到覆盖。

服务器为标记权威。Public 只在同 user UUID、auction UUID 下缓存非敏感标记，GET 成功前禁止修改，退出或切换账号/场次不得沿用旧响应；成功标记或竞价/代理命令后重读完整集合，GET 不产生自动标记。未知 PUT 在同账号/场次宿主存活期间保留原 `command_id` 和完整请求以供原样重试，翻页/分类不丢失；退出、换号、换场、离开宿主或整页重载后不自动重放，重新进入先读服务器，待定命令不写入 localStorage；409 显式提示并刷新权威状态，不得静默改版本或用新 ID 重试未知结果。普通竞价不自动标记，新建代理成功时同事务默认标记；降额、自动追价、刷新与旧命令重放不恢复手动取消。通知按真实“领先 → 被超越”周期去重，不是每次价格变化，也不是论坛 `notify mm` 单次请求；MM 走既有邮件投递链，无需浏览器在线；取消或重新领先使未开始发送的旧提醒失效，已发送/发送中的不可撤回。事务、迁移及未知发送约束见[不变量](docs/contracts/invariants.md#i-09g-装备标记与超价提醒按领先周期去重)。

### P25/P26 账号竞价警告渠道

SQL 与用例 owner 为 lot-watch preference，不是每 lot 设置。GET/PUT 成功 data 均为 `PublicLotWatchPreferenceResponse {user_id, channel, version}`，UUID 字符串，channel 严格 `mm|pm`，version 正 i64；新账号默认 MM/1，无 GET 写入。PUT `PublicSetLotWatchPreferenceRequest {command_id, channel, expected_version}` 拒绝未知字段，4 KiB 上限；身份只来自会话；无查询参数，未知/重复查询、JSON/UUID 词法错误 400；未登录 401；非法渠道或非正版本 422 `invalid_watch_preference`；命令/版本冲突 409 `watch_preference_conflict`；缺依赖 503 `storage_unavailable`。同 ID 同负载重放原结果，不同负载冲突；不同新 ID 即使同渠道也推进版本。未知请求仅原 payload/ID 重试，409 后须显式 GET 再表达新意图，重放成功后 GET 权威状态。

偏好只影响之后的已标记装备反超事件，不影响结算/COD；已排队通知冻结原渠道，不撤回、不补发、不双发。无稳定论坛账号 ID 仍可保存 PM，但事件形成 `suppressed/recipient_missing`，不是可送达承诺。Public 我的账户顶部 `outbid-notifications` 区独立挂载，账号 keyed scope，未知意图不写 localStorage；离开/重载先 GET，不自动重放。迁移和运行证据见[状态](docs/status.md)。

### P22 自托管论坛倒计时

`auction-http/public/countdown/` 拥有动态 SVG 和样式，`AuctionReader::public_auction_countdown` 提供窄时间快照，`auction-storage/database/auctions/countdown.rs` 用单次 PostgreSQL 读取绑定公开可见性、拍卖时间及数据库时钟。草稿、取消场次和关闭世界中的活动场次 404，settled 历史保留；存在活动场次时不公开后继预览；预览态不提前公开计划结束时间，缺计划结束时间的 Auction Timer 显示 `Not scheduled`，不推算。

`list.svg` 为 List Timer：实际 `opened_at` 优先，否则 `scheduled_open_at`；`auction.svg` 为 Auction Timer：使用本场 `scheduled_end_at`，不合并单件 soft-close。图片固定英文标题、日/时/分/秒、UTC 日期及 `Closed`/`Ended` 终态，不接受标题、时间、任意 SVG、外链字体或其他自由文本。成功类型 `image/svg+xml; charset=utf-8`，不包 JSON；HEAD 无响应体，其他方法 JSON 405；非法 UUID/query 400，未知计时器、缺场次或隐藏场次 404，未装配/失败 503。

剩余时间由数据库时钟在加载时计算，CSS 离散动画每秒换字，到期隐藏数字并显示终态，不运行 JavaScript、不维持连接或轮询；数字节点数不随剩余秒数增长，天数最多六位，输出至多 32 KiB。计时数字默认可见，减少动态效果也保留逐秒变化，不发生滑动或闪烁；不支持或禁用动画时显示加载时的剩余数字快照，需刷新更新，UTC 截止始终保留。已打开图片不会得知后续改期或状态变化，须刷新重读；此展示不能决定竞价资格、结算或单件截止。浏览器 `Cache-Control` 和两种 CDN 头均 `no-store`，不使用 ETag/Last-Modified/304；只有成功图片的私有响应标记启用 `Cross-Origin-Resource-Policy: cross-origin` 并将 CSP 收紧到禁止脚本/外部资源、仅允许内联样式，不允许 iframe、不放宽其他 API 或站点 CSP，也不依赖 Origin/Referer/Cookie 才能展示。论坛首帖从已配置公开 origin 生成同源图片链接并保留 `#.png` 兼容后缀，实际格式仍为 SVG；未配置公开 origin 时由首帖时间列表兜底，不回退第三方服务。上线顺序与 CDN 要求见[部署手册](ops/README.md#cloudflare-整站代理)。

### P21 论坛同步开关

P21 使用 `PublicForumSyncStatusResponse`，成功响应固定 `code=ok`、`data={"enabled": true/false}`，不依赖当前世界或拍卖是否存在。布尔值来自 Worker 按 `[worker.forum].enabled` 发布的非敏感心跳字段，仅采纳数据库时钟下未过期且非 `stopping` 的 Worker：任一明确 true 表示启用，所有有效实例均明确 false 才表示关闭。无实例、旧版心跳缺字段或无法证明开关值时返回 `503 forum_sync_unavailable`；未装配存储或查询失败返回 `503 storage_unavailable`。启用不表示主题已发布或扫描成功，真实楼层仍由 P15 的 `processed_through_post_number` 提供。Public `/forum` 每 30 秒重读 P21，关闭提示优先于无拍卖/未开拍状态，关闭时不挂载 P15 轮询；未知和请求失败显示不可用，不解释为关闭；公开页无手动重试按钮。手动刷新入口在 Admin `/auctions`，升级须同时更新 Worker、两端 API 与内嵌页面。

### Public 分页、世界与金额

Public 浏览器目录、往期拍卖、出价记录（含装备详情）、成交价、黑名单及补偿池使用 URL `page`/`page_size`，默认 10 条，保留 10/20/50/100 预设，允许自定义正 `i64` 数量，不设 200 条或其他业务数量上限；输入非法不提交，URL 非法数量回退 10。URL 明确值优先于本地偏好，缺失时恢复 `hv.public.page-size`；仅用户确认数量才保存，存储不可用不阻断页面。切换数量回首页并保留其他筛选；写回及离开历史条目明确保留数量（含 10）。数字页码和跳页遵守现有 offset 上限，不削减权威 total。补偿池展开项用 `auction` 标识，流水用独立 `movement_page`/`movement_page_size` 与 `hv.public.movement-page-size` 偏好，API 请求仍映射成 P14 的 `page`/`page_size`，不新增后端参数；
缺 total 的投影不得显示伪造总数。P05、P07、P09、P10、P12–P14 的 HTTP `page_size` 取消 200 条上限，接受 `1..=i64::MAX`，默认值、offset 上限、cursor 与筛选边界不变；游标查询仅对额外探测一行的内部 limit 使用饱和加法，请求及响应数量不截断；Admin、账户与实时事件分页保留各自原上限。不新增或改变 DTO 字段、事件或数据库结构。

P07 拍卖编号页的 `total` 表示完整筛选集总数，与当前页来自同一读取快照；越界空页不改为 0；cursor 页不执行或返回总数统计。业务世界关闭后，Public 当前目录、preview/live/final/settling 投影、写入入口和 discovery 增量都必须隐藏该世界；只允许已 `settled` 的拍卖、lot、竞价/账户历史、价格历史与补偿事实继续经历史读取边界访问，不能把 `cancelled` 或禁用世界中的活动拍卖顺带公开。P09 的 lot 投影允许读取仍属公开安全边界的 settled 拍卖；响应上下文固定包含 `auction_id`、`world`、`number`、状态、展示模式、开标时间、可选有效结束时间和 `preview_revision`。
公开黑名单的 `total` 与当前页也在同一只读快照中读取，保留原筛选、排序和越界空页的完整总数。
装备目录的列显隐仅为 Public 浏览器展示偏好，不改变 P09/P11 的字段、请求参数、筛选、分页、权限或返回范围。
设置入口、持久化键及可见列布局由[Public 设计](crates/auction-web-public/DESIGN.md#5-表格与信息密度)拥有。

P18-P20 新提交的普通出价、代理创建/降低金额最高为 1B（1,000,000,000 Credits），仍要求 1000 网格；新命令由领域层校验并沿用 `invalid_amount`/`invalid_maximum_amount`，既有 command_id 的成功重放不被重新解释。P09/P11 的 `next_bid` 保留数学最低价和严格抬价关系；超过 1B 时表示没有可提交的新出价，浏览器显示“已达上限”，不能压低到 1B 后提交。普通输入使用整数 K；代理输入扩展为 K/M/B，M/B 最多三/六位小数，均须精确落在 1K 网格且不超过 1B。P16/P17 的活动代理能力投影不把“上限不足完整下一口”当作损坏：合法慢速等待仍返回 `waiting=true`，上限不高于降额绑定下限时返回 `can_lower=false`；例如默认 5% 加价、当前价 99,000、慢速上限 100,000 时允许等待并反超，不因下一口 104,000 返回 503；P19 的降额约束不放宽。

### P09/P11 装备分组

P09 的可选 `equipment_group` 查询参数只接受 `rare`、`fusion`，省略表示全部；空值、`all`、未知值、重复参数返回 400。
它与既有 `category` 组合，并在分页前应用。`category_counts` 统计所选装备分组下各类型的总数，
`total` 统计再叠加类型条件后的总数；数量与当前页来自同一数据库读取快照，越界空页仍保留真实总数。
未选择分组时沿用现有 SQL 计数和分页；选择分组时流式读取轻量身份，用应用层统一规则筛选，
仅保留请求页的 ID，再在同一事务中读取完整拍品，保持既有顺序。

P09/P11 的正式装备对象、预览摘要与预览详情装备对象均返回必填 `equipment_group: "rare" | "fusion"`；
预览摘要与详情值必须相同。分组由应用层与论坛共用的标色判定生成：Peerless 固定彩虹或首条匹配规则实际产生名称颜色时为
`rare`，其余为 `fusion`。Public 只展示文字分组，不应用论坛颜色；分组独立于品质与装备类型，不决定收录、价格、出价或结算。
当前、预览及历史拍卖目录都使用进程启动时载入的规则；规则不写入装备身份、拍卖快照或历史审计。

公开页把“全部／稀有／融合”置于装备类型筛选上方，默认全部；任一筛选变化回到第一页，并保留另一筛选及每页数量。
URL、异步请求作用域与实时刷新携带同一分组；列表装备列仅显示名称，装备详情展示服务端分组。
Public API 与内嵌页面需同版升级，配置共享与重启要求见[配置说明](ops/config/README.md)。

### 标的编号、来源与历史读取

所有 lot/竞价/账户/成交投影中的 `lot_number` 都是装备分类内从 1 开始的用户可见编号，必须与 `equipment.category` 组合解释为 `One01`、`Two01` 等编码，不是全局身份。详情和写命令只以 `lot_id` UUID 定位，价格历史的 opaque cursor 继续绑定内部场次顺序，不能由可重复的分类编号重建。P10 竞价日志额外保留 `world` 与 `lot` 筛选；`lot` 可以是 lot UUID 或 `One01` 一类分类编码，二者在内部请求中互斥，并在 storage 的全局结果集分页前应用，不能只筛选当前页面。条目按 `created_at`、`lot_id`、`accepted_state_version`、`fact_index`、`id` 降序排列，cursor 是绑定这五个字段的版本化 opaque 值；
编码数字部分最少两位（如 `One01`），分类编号超过 99 时保留完整十进制位数（如 `One100`），不得截断、重排或复用既有编号；论坛目录与命令解析使用同一格式。
带 cursor 的返回使用 next_cursor/has_more，不得把未知总数伪装成精确 total。可选 `source` 只允许 `forum`/`proxy`：论坛普通竞价仅在账本保存真实楼层时同时返回 `source_post_number`；若同一账本还能证明真实 thread/post ID，则增加规范 `source_post_url`。链接身份不得由楼层号或数字论坛账号 ID 推算，玩家展示继续用名称；代理事实不区分 fast/slow 且不携带楼层或帖子链接；普通 Web 出价三者都为空；历史论坛事实楼层未知时只标记论坛来源。P09/P11 的 preview/live 装备投影可选返回按 `Strength, Dexterity, Agility, Endurance, Intelligence, Wisdom` 固定顺序生成的 `primary_attribute_initials`，缺少对应属性时省略而不是伪造。
P12 保留 `name`、`world`、日期、价格、buyer/seller 筛选；名称按 Unicode 空白或英文逗号拆词，忽略空词后进行忽略大小写的
字面包含 AND 匹配，不要求词条相邻或依输入顺序出现；例如 `Leg Sav Pow Hel Bal` 可匹配 `Legendary Savage Power Helmet of Balance`。
原逗号形式继续支持，总长最多 1024 UTF-8 字节、最多八词、每词最多 128 字节；拆后词条中的控制字符拒绝，
`%`、`_` 与反斜杠按字面转义。编号与 cursor 及完整筛选集统计共用相同条件；buyer/seller 忽略大小写精确匹配。
编号页新增可选 `sort=closed_desc`（默认兼容）、`closed_asc`、`price_desc`、`price_asc`，排序在完整筛选集分页前应用；
主排序值相同时沿用成交时间倒序、场次倒序、内部 lot 顺序和 UUID 升序。旧 `v1/type5` opaque cursor 固定 `closed_desc`，
非默认排序与 cursor 组合返回 `400 invalid_query`，旧 token 词法保持不变。
编号页附带同一读取快照内完整过滤集的 `stats`，cursor 页不返回统计；`count/min/max/median` 继续保留，
新增可选 `median_exact` 十进制字符串（无前导零，整数或 `.5`，范围 `0..=9,007,199,254,740,991`），空集为 `"0"`。
中位数以排序后中间一至两笔价格的精确平均值计算，不经浮点数；`median` 保留向最近整数取整、半数取偶的旧数值口径
（例如 `2.5 → 2`、`3.5 → 4`），`median_exact` 始终保留精确值。
应用与 HTTP 同时校验精确值在最低/最高价区间内，并与兼容整数一致；旧适配器缺失 `median_exact` 仍可读取。
成交读取必须基于 settlement 事实链，价格取成交时 lot 的 `current_price`、时间取 `effective_end_at`，不能把状态列或空集合当作成交证明。

P03/P04 的查询、事件 JSON、`after` 游标、缺口高水位、会话复核、配额与关闭 code/reason 由[事件契约](docs/contracts/events.md)登记；P03 是持久化 Catch-up，P04 是服务端单事件推送，发现流必须显式指定 `scope=auctions`。不得把 sequence 改成时间戳，或用 HTTP 分页替代长连接通知。
Public 浏览器仅在已认证且当前拍卖为 `live`/`final` 时启用 P04；预览及其他非竞价阶段停用拍卖与发现流连接，
通过既有当前场次权威读取确认开拍后再连接。服务端 P03/P04 的请求与响应契约不变，客户端展示策略由[Public 设计](crates/auction-web-public/DESIGN.md#7-数据时间与实时状态)拥有。

### Public 时间词法

所有 Public JSON 时间使用 RFC3339Nano 词法：UTC 时间用 `Z`，小数秒裁掉尾随零，整秒不附加 `.000000`。P18-P20 从历史命令账本重放时还必须保留账本字符串原有的 offset 与纳秒精度，同时以解析后的 UTC instant 做一致性校验。该规则不改变 PostgreSQL 关系字段的微秒精度，也不改变 Admin 固定的六位微秒投影。

## Admin

除 Admin session 本身外，下表路径都经过 Admin session；mutation 还经过 origin、CSRF、限流和 actor 注入。boundary 规范化后的 `request_id` 注入支持审计的管理命令和生命周期命令，Worker 内部命令保持 `None`，不得把 `command_id` 冒充请求关联。多方法路径必须按方法分别保留成功状态；下表还适用通用请求/方法边界错误，所有 mutation 受 `429 rate_limited` 约束。

| 编号 | 方法 | 路径 | 成功 | 明确错误状态 |
| --- | --- | --- | --- | --- |
| A01 | POST/GET/DELETE | /api/v1/admin/session | POST/GET/DELETE 200 | 400, 401, 403, 429, 503/504 |
| A02 | GET | /api/v1/admin/dashboard | 200 | 400, 401, 503/504 |
| A03 | GET/POST | /api/v1/admin/auctions | GET 200；POST 201 | GET: 400, 401, 501, 503/504；POST: 400, 401, 403, 409, 413/415, 501, 503/504 |
| A04 | POST | /api/v1/admin/worlds/{world_id}/auction-intake/items | 201 | 400, 401, 403, 409, 413/415, 501, 503/504 |
| A05 | POST | /api/v1/admin/auction-forum/preview | 200 | 400, 401, 403, 413/415, 503/504 |
| A06 | GET | /api/v1/admin/auctions/{auction_id}/lots | 200 | 400, 401, 404, 501, 503/504 |
| A07 | GET | /api/v1/admin/auctions/{auction_id}/settlement-report | 200 | 400, 401, 404, 501, 503/504 |
| A08 | GET | /api/v1/admin/auctions/{auction_id}/recovery-report | 200 | 400, 401, 404, 501, 503/504 |
| A09 | POST | /api/v1/admin/auctions/{auction_id}/preview | 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A10 | POST | /api/v1/admin/auctions/{auction_id}/open | 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A11 | POST | /api/v1/admin/auctions/{auction_id}/cancel | 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A12 | POST | /api/v1/admin/auctions/{auction_id}/final-phase | 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A13 | PATCH | /api/v1/admin/lots/{lot_id} | 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A14 | POST | /api/v1/admin/recoveries/{recovery_id}/confirm | 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A15 | GET | /api/v1/admin/lots/{lot_id}/proxy-events | 200 | 400, 401, 404, 501, 503/504 |
| A16 | GET | /api/v1/admin/mail | 200 | 400, 401, 404, 501, 503/504 |
| A17 | GET | /api/v1/admin/mail/intake | 200；列表含可空 `sender`/`subject` 与 `has_message_display` | 400, 401, 501, 503/504 |
| A18 | GET | /api/v1/admin/mail/intake/{intake_id} | 200；详情返回 `sender`/`subject`、脱敏 `message_display`、可选 `equipment_diagnostic` 与 `deliveries` | 400, 401, 404, 501, 503/504 |
| A19 | PUT | /api/v1/admin/mail/polling | 200 | 400, 401, 403, 409, 413/415, 501, 503/504 |
| A20 | POST | /api/v1/admin/mail/polls:trigger | 202 | 400, 401, 403, 409, 413/415, 501, 503/504 |
| A21 | GET | /api/v1/admin/realtime/lag | 200 | 400, 401, 501, 503/504 |
| A22 | GET | /api/v1/admin/realtime/failures | 200 | 400, 401, 501, 503/504 |
| A23 | GET | /api/v1/admin/realtime/failures/{failure_id} | 200 | 400, 401, 404, 501, 503/504 |
| A24 | POST | /api/v1/admin/realtime/failures/{failure_id}/replay | 200 | 400, 401, 403, 404, 409, 413, 415, 500, 501, 503/504 |
| A25 | GET/POST/PUT | /api/v1/admin/rules | GET 可带可选 `lang`；GET/POST/PUT 200；创建时 201 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A26 | GET/PUT | /api/v1/admin/config | GET/PUT 200 | 400, 401, 403, 409, 413/415, 501, 503/504 |
| A27 | POST | /api/v1/admin/config:validate | 200 | 400, 401, 403, 413/415, 501, 503/504 |
| A28 | GET/POST/PATCH | /api/v1/admin/users | GET 200；POST 201；PATCH 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A29 | GET | /api/v1/admin/renames | 200 | 400, 401, 503/504 |
| A30 | POST | /api/v1/admin/renames/{rename_id}/revert | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A31 | PUT/DELETE | /api/v1/admin/users/{user_id}/blacklist | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A32 | GET/POST/PATCH/DELETE | /api/v1/admin/credentials | GET 200；POST 201；PATCH/DELETE 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A33 | GET | /api/v1/admin/audit | 200 | 400, 401, 503/504 |
| A34 | GET | /api/v1/admin/deliveries | 200 | 400, 401, 501, 503/504 |
| A35 | POST | /api/v1/admin/deliveries/{delivery_id}/release | 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A36 | POST | /api/v1/admin/deliveries/{delivery_id}/retry | 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A37 | POST | /api/v1/admin/deliveries/{delivery_id}/reconcile | 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A38 | POST | /api/v1/admin/auctions/{auction_id}/early-settlement | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A39 | PATCH | /api/v1/admin/auctions/{auction_id}/schedule | 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A40 | GET | /api/v1/admin/worlds | 200 | 400, 401, 503/504 |
| A41 | PATCH | /api/v1/admin/worlds/{world_id} | 200 | 400, 401, 403, 404, 409, 413/415, 429, 501, 503/504 |
| A42 | GET | /api/v1/admin/settlement-catalog | 200 | 400, 401, 501, 503/504 |
| A43 | GET | /api/v1/admin/realtime/events | 200 | 400, 401, 500, 501, 503/504 |
| A44 | POST | /api/v1/admin/credentials/{credential_id}/check | 200；只读认证观测 | 400, 401, 403, 404, 409, 413/415, 429, 500, 501, 503/504 |
| A45 | GET | /api/v1/admin/forum/sync-status | 200；只读本地状态 | 400, 401, 501, 503/504 |
| A46 | GET | /api/v1/admin/auctions/{auction_id}/forum | 200；只读本地投影 | 400, 401, 404, 501, 503/504 |

## 首次部署初始化

PostgreSQL + `web-embed` 的 `auction-admin-api`/`auction-server` 初始化模式默认监听 `127.0.0.1:8091`，正式 Admin 不挂载这些入口。分体 Admin 仅在默认配置不存在且未设置 `AUCTION_CONFIG_FILE` 时无参数进入；已有目录、悬空链接仍算已有配置，元数据错误、显式配置缺失或解析失败均阻止启动。合并入口按 `--config-dir`（默认 `auction-config`）判断，拒绝已设置的 `AUCTION_CONFIG_FILE`。分体保存后退出；合并入口只有在明确确认并关闭设置服务后才初始化专用空库、启动角色；普通启动不隐式迁移。

静态资源可匿名读取，三个 API 每次须携带终端输出的 64 字符十六进制 `x-setup-token`。正文上限 8 KiB，DTO 拒绝未知字段，令牌/非阻塞操作门禁先于正文。HTTP/1 请求头预算 5 秒，正文与异步处理合计 10 秒，超时 504 `request_timeout`。停止接收后最多排空 10 秒，超时或再次信号取消连接；合并入口只接受无终止信号的确定 Completed，无法确认终态时失败关闭。

合并入口通过 `--allow-remote-setup`、`--setup-origin` 和 `--setup-proxy-ip` 明确允许远程 HTTPS 代理，非 loopback 监听无此授权失败。HTTP 门禁检查 TCP peer、精确 Host、HTTPS 转发头与写请求 Origin；本机模式也检查 loopback peer 与 Host/Origin，失败返回 403 `setup_origin`。所有应用响应（含拒绝、框架错误和超时）统一设置 `Cache-Control: no-store`、`Referrer-Policy: no-referrer`，复用正式表面的 CSP（含 `frame-ancestors 'none'`）、`X-Frame-Options: DENY`、`nosniff` 与浏览器能力限制，保持 Dioxus WASM/解释器和内联样式所需策略，不放宽 Origin 或令牌门禁。
成功响应新增布尔 `initialize_and_start`，默认分体 false、合并 true，旧客户端可忽略。配置 DTO 新增可省略的 `confirm_initialize`（默认 false）；合并 `complete` 要求 true，否则 400 `setup_confirmation_required`，网页只在用户主动勾选专用空库及启动授权后发送 true；`completed` 只表示配置已落盘，不声称数据库初始化或三个角色已就绪。

`verify` 无请求体，只校验令牌和完成状态，不连接数据库、写文件、消费令牌或建立免鉴权会话。三个 API 鉴权后共用操作锁，覆盖校验、数据库等待与目录发布；竞争为 409 `setup_busy`，完成后 409 `setup_completed`。目录已发布但持久化无法确认时进入终态 500 `setup_outcome_unknown`，不再读库/写配置，须停止进程并核对目录；输入、连接及发布前明确失败仍可重试。
完成状态及一次性退出通知由同一操作锁持有；目录发布后在释放锁前同步设置终态并发送通知，不再经过额外异步锁等待。

页面先验证部署码，返回 `verified` 才展示配置；部署码/验证状态仅保存在内存，刷新重新验证，`setup_auth` 清空部署码及密码并退回第一步，完成后清空并隐藏表单，密码确认仅在浏览器比较。数字按原始输入校验，错误用五语言定位摘要展示并展开进阶字段；修改输入清除旧成功提示，浏览器校验不替代配置 owner，`validated` 不表示落盘。每个浏览器请求独立限时 10 秒且不自动重试；保存超时、断连或不可识别响应按 `setup_outcome_unknown` 处理，该码使页面清空秘密、锁定输入及提交回调且不提供解锁/重试。客户端中止请求不能证明服务端未写入，也不新增服务端状态码。

`validate`/`complete` 使用同一 `SetupRequest`：必填 `database`、`admin_actor`、`admin_secret`、`public_origin`、`admin_origin`、`metrics_enabled`、`schedule_enabled`、`start_price`、`log_filter`、`worker_concurrency`、`worker_poll_interval_seconds`、`realtime_poll_interval_millis`；`database` 必填 `host`、`port`（1–65535 整数）、`username`、`password`、`name`。
账号/密码/库名按原文提交，不做 URL 编码；不接受 `database_url`。连接检查与落盘复用配置模型，保存为敏感 TOML `[database]`。`public_listener_port`/`admin_listener_port` 默认 8090/8091，均为不同的 1–65535 整数，非法为 400 `invalid_input`；只生成正式服务 `127.0.0.1:<port>`，Admin 角色与 runtime listener 同步，不改变设置监听器、Origin、代理或 Cookie，不探测占用/权限；runtime 文件为 `runtime/admin.toml`。
“恢复进阶默认值”只重置日志、Worker 并发/轮询、实时轮询并清旧提示，保留凭据、Origin、端口及基础设置，不发请求/写配置。

配置请求还接受可省略的 `schedule` 对象，启用状态仍由必填布尔 `schedule_enabled` 决定；整个对象或内部单项省略时使用下表默认值，显式空串、`null`、错误类型或未知字段失败关闭，不自动纠正。

| `schedule` 字段 | 默认值 | 首次设置约束 |
| --- | --- | --- |
| `timezone` | `UTC` | 有效 IANA 时区，不超过 128 个 UTF-8 字节 |
| `preview_day` / `preview_time` | `Thursday` / `22:00` | 预览星期与当地分钟时间 |
| `open_day` / `open_time` | `Friday` / `00:00` | 开拍星期与当地分钟时间 |
| `nominal_end_day` / `nominal_end_time` | `Saturday` / `12:00` | 名义结束星期与当地分钟时间 |

星期只接受 `Monday` 至 `Sunday` 的完整英文名称，时间须为精确 24 小时制 `HH:MM`；三个阶段在同一周一至周日周期内按当地分钟排序，满足 `preview + 120 <= open < nominal_end`，不自动跨周或使用浏览器时区。服务端在连接数据库前复用配置 owner 的排期校验，非法输入 400 `invalid_input`，即使 `schedule_enabled=false` 也必须保存合法候选，关闭不创建拍卖。完整计划保存到 `common.toml` 的 `[auction.schedule]`，世界固定为 `main`、`isekai`，表单不接受 `worlds` 或 `managed`；生成的 runtime 保持 `operations.schedule.managed=false`，不覆盖公共计划，后续仅影响 Worker 新建的周拍卖，不改已有拍卖、任务游标或租约。页面星期用五语言下拉框但发送英文值，时间用分钟控件；浏览器仅做时区文本边界检查，IANA 有效性与实际周计划由服务端判定。

| 方法 | 路径 | 成功 | 明确失败 |
| --- | --- | --- | --- |
| GET/HEAD | `/setup` | 200，Admin WASM 初始化页 | 403 `setup_origin`，404/503（无资源） |
| POST | `/api/v1/setup/verify` | 200 `{"status":"verified","initialize_and_start":false}` | 403 `setup_auth`，409 `setup_busy`/`setup_completed`，500 `setup_outcome_unknown` |
| POST | `/api/v1/setup/validate` | 200 `{"status":"validated","initialize_and_start":false}` | 同 verify，另有 400 `invalid_input`/`configuration_invalid`，503 `database_unavailable` |
| POST | `/api/v1/setup/complete` | 200 `{"status":"completed","initialize_and_start":false}`；写出完整配置并关闭设置服务 | 同 validate，另有 409 `setup_locked`，500 `setup_io` |

数据库凭据、管理员密码只在请求与目标敏感 TOML 中存在；响应只返回稳定错误码，不返回输入、SQLx 错误或 TOML 正文；数据库检查只确认连接，不建库、不迁移；保存后目标配置目录存在即禁止再次首次设置。表中成功 JSON 以分体模式为例，合并模式布尔值为 true；所有接口可返回 403 `setup_origin` 或 504 `request_timeout`，合并 complete 还可返回 400 `setup_confirmation_required`。完整运行参数仍在正式 Admin `A26/A27` runtime 页面管理。

## 已实现路径的补充约束

### 公开读取与命令结果

P02 接受可选 `lang`，缺失回退英文；P05 为安全黑名单页码投影。P07 的数字/keyset 分页、P10 world/lot 筛选及 P12 成交统计遵守上文分页契约。P09 保留 v1 preview/live 互斥数组、分类统计、页码及拍卖上下文；P11 只返回当前允许公开的 lot；P13/P14 返回补偿池批次、余额守恒摘要、opaque keyset cursor 与单场账本页，从同一 Serializable 快照读取。
P13 的全局 `balance` 是所有 world 当前 `settlement_pools.balance` 的合计，`updated_at` 为池更新时间最大值；
各批次的 opening/closing 仍是原场次冻结摘要，不拿最近某个世界的结算余额冒充全局当前余额。

P09/P11/P14 在 PostgreSQL 按 lot/equipment/world 校验密文后短暂解密，直接在既有 `game_equipment_url` 返回固定 HentaVerse origin 的完整 URL；10 位 key 可供用户主动导航，但不得写日志、审计、事件或持久化缓存，响应 `no-store`、全局 `no-referrer`，不伪造 keyless 链接。
P14 浏览器解码与目录复用同一完整链接校验：只接受 `https://hentaiverse.org/equip/{id}/{key}` 或 `/isekai/equip/{id}/{key}`，ID 为 1–32 位 ASCII 数字，key 为 10 位十六进制，拒绝额外路径、query、fragment 及凭据。`opening`/`closing` 省略装备、比例与时长字段；提供链接时不得为空串或仅含 ID。分页字段及金额、余额链语义不变。

P15 读取本地论坛镜像的源/内容版本、真实连续楼层、同步/最近成功时间及有界安全错误。`/forum` 仅在 P21 明确启用后轮询 P15：`pending`/`publishing` 每次完成后等 2 秒，`published` 等 30 秒，发布后显示“当前同步到 #xx”；失败暂停帖子轮询，重新进入或加载页面后重读，不提供公开页手动重试按钮。公开页仅展示同步摘要、真实处理楼层、更新时间与简短延迟提醒，已同步提示行右侧提供主题链接，不展示正文或诊断详情。Admin `/auctions` 承接只读刷新及标题、正文、版本、同步/扫描时间和错误详情，正文默认折叠为转义纯文本；P15/A46 的既有 DTO 字段不变，不新增读写权限，首页不轮询。

P06 会话、P16/P17 账户/代理读取和 P18–P20 竞价命令使用 PostgreSQL 适配。账户投影包含普通竞价、quota、转换历史、最近 proxy bid 与 owner capability。P20 使用稳定 `command_id`/`request_hash`；P18/P19 返回完整 `{ authorization: ... }`，quota、转换历史和终止原因与命令结果同事务冻结，不在提交后另读账户，也不暴露账本 payload；未知结果必须重放原命令，详见[幂等不变量](docs/contracts/invariants.md#i-01-命令账本是权威幂等边界)。

### 管理拍卖与报表

A01 使用 Basic Auth、CSRF 和持久化会话。Admin 每次装配数据库会话服务先永久撤销全部旧会话，失败则阻止启动，重启须重新登录；随机 token 摘要用 HMAC-SHA256 绑定当前 actor/secret，读取再核对 actor，不额外存凭据摘要；换凭据或恢复旧配置都不能复活撤销 token，复用原表/撤销标记，Public 不受影响。同库多 Admin 必须使用相同凭据，任一实例启动会撤销既有会话，轮换须先停全部旧凭据实例。浏览器每次成功确认后重新安排检查，即使绝对到期使 `expires_at` 不变；业务 401 触发确认，网络错误不当作退出。

| 接口 | 读取或写入契约 |
| --- | --- |
| 拍卖列表（A03 GET） | `world_id`/`status` 筛选、有界页码；生命周期与 lot 数量来自同一 Serializable 快照 |
| 创建（A03 POST） | 零版本 fence；同事务锁 world/intake target，写策略快照、命令账本、审计与 `auction.created` |
| 录入（A04） | URL/canonical JSON 录入，固定 HTTPS 来源、key 密文及 Serializable 原子写入；只返回拍卖/投稿/lot/装备安全事实和分类内 `lot_number`/`equipment_category` |
| 拍品列表（A06） | 同快照读取拍卖/lot/装备/投稿，body 固定 `auction/lots/pagination`；外部录入能力通过 `X-Auction-Capability-Manual-Intake` 头提供 |
| 代理历史（A15） | 页码/opaque cursor 代理历史，包含创建、降额与黑名单子命令的 `frozen` 转换；冻结不伪装成 bid |
| 报告（A07/A08） | 一致结算/恢复安全报告，lot 带 `equipment_category`；A07 拍卖字段闭集不含 `scheduled_end_at`/`effective_end_at`，lot 提供权威 `listed_price`、`hold_ppm`、带符号 `adjustment_ppm` |
| 结算目录（A42） | 仅列 `settled` 且有 `settled_at` 的拍卖；可选 `world_id`/`sequence` 与有界 `page/page_size`，count 与页同 Serializable 快照 |

A04 的 `equipment_source.require_tradeable` 默认关闭，允许手工 Soulbound；开启后在事务前限制可交易装备，不改变 MoogleMail 过滤，也不回传来源 URL/canonical 正文/key。A07 的 `adjustment_ppm` 正为税费/惩罚、负为折扣，页面不从税额反推持有比例；报告序列化冲突/死锁最多尝试 3 次（含首次）。已结算报告排除未参与结算的 `withdrawn` 标的，其余标的缺少结算明细仍明确失败；累计领先时长从同一快照中的追加竞价事实计算。A42 指定世界按期号降序，否则按结算时间、世界、期号、UUID 稳定排序；结算页用第一页首项作为默认最新一期，再按 UUID 读报告。

A09–A12、A38 携带 `command_id`、`expected_state_version`；取消/提前结算另需非空理由，原因最多 1024 字节且不含控制字符，其他动作不得携带 reason；状态、账本、动作 metadata、审计与事件同事务写入。人工 Final 将晚于事务时间加冻结 `final_phase_lead_seconds` 的 live lot 截止收紧至该窗口（默认 900 秒），不延长更早截止，之后仍允许软截止延长。A38 仅在完整档且 `allow_manual_early_settlement=true` 时提供（默认关闭），接受 `live`/`final`，将未到期 live lot 截止收紧到事务时间；成交、义务与履约仍由提交后的 settlement Worker 写入；未开启或缺能力返回 `403 manual_early_settlement_disabled`，不套用一般能力缺失的 501。

A39 只允许 `draft`/`preview`，请求含 `command_id`、`expected_state_version`、`preview_scheduled_at`、`scheduled_open_at`、`scheduled_end_at`，满足 `preview < open < end`；Draft 三者须在数据库当前时间之后，Preview 原样保留已发生的预览时间，其余在未来。完整档 Serializable 事务按 world、scheduled round、auction 加锁，同步计划列、结束策略快照、调度记录、账本、审计与 `auction.schedule_updated`；未过期且有 owner 的租约拒绝修改，实际变更时清除过期租约或无 owner 的失败退避并递增 round 版本，阻断陈旧 Worker 写回；事件 v1 字段不变，只唤醒权威快照重读，其他状态不提供修改。

既有 delivery 投影接受独立 `lot_watch` 来源，不新增 Admin 路径或动作。只有原因/失败码均为 `lot_watch_invalidated`、状态 failed、无 sending/lease/外部引用、零 COD 的 watch 行使用发送前失败时间链；创建/领取/完成时间仍须有序，不伪造 sending 或 Sent 证明，此类行不展示重试入口。Admin release/retry/reconcile-retry 在 lot → delivery 锁内复用 watch proof，失效或缺来源不能重新排队，未知发送走原人工核对。A34–A37 为完整档 delivery 安全列表、状态筛选、数字页及 release/retry/reconcile，携带 fencing version 和独立账本；
A34 的数字分页偏移必须在 `0..=100000`，HTTP、应用与存储复用同一边界，非法深分页返回 `400 invalid_query`。
reconcile 请求明确 `retry`/`succeeded`/`failed` 与对应外部引用/失败码。已有持久资金 allocation 的新 release、retry、reconcile 三类请求（共五种动作）统一返回 `400 invalid_delivery_command`（Invalid），不改状态或释放资金；原 command_id 已提交成功结果先重放。无绑定旧 retry 在同事务按 recovery scope→所需业务锁→actions→delivery→fulfillment/reset 预锁，原权限与 CAS 不变，本轮不新增资金人工恢复 API。列表从 send payload 投影 COD，从 outcome metadata 投影 `reason_code`、`take_required`、`returned_mail_id`，不返回原文；
`take_required=false` 省略，客户端缺失按 false，显式 null/非布尔仍无效。命令只推进本地状态，不伪造远端回执；基础通知、结算 Send、入站 Return/Take 与 Sent/Balance 核对由 Worker 执行，缺 exact remote ID 进入人工复核；未装配 owner 明确报告能力缺失。

### 投递对账与远端证明

Admin `reconcile` 的 `succeeded` 分支不能用 `payload.operation=send` 加裸 `external_reference` 完成状态；缺少 Sent exact payload proof 的请求按既有 invalid 条件拒绝，Admin Web 隐藏该成功选项，`retry`/`failed` 仍须显式命令并审计，Return/Take 保留独立人工核对路径；该防御不新增 HTTP 字段或错误类别。Delivery 列表允许告警确定未发送的失败形状：仅 `operations_alert`、`reason_code=operations_alert_batch_terminal`、错误码 `dependency_mismatch|retry_exhausted`，attempt/fencing 为零、claimed/sending/lease/外部引用为空且完成时间有效；
Storage 与 Admin Web 按同一闭集接受，其他投递终态仍须提供完整执行轨迹。另一个具名例外是 `mail_intake` 的 `reason_code=mail_disposition_conflict`：历史待发送资产冲突可在没有 `sending_at` 时进入 `manual_reconcile`，保留原 payload/命令和隔离原因，只允许人工确认 `failed`，禁止确认成功、重试和失败后重新激活；收件详情的关联 delivery 同步返回可省略的 `reason_code`，不暴露原始 metadata。主动 ComposeSend 的成功 metadata 固定包含 `sent_verified=true`、`external_reference_proven=true`、`proof=sent_exact_payload_v1` 与真实引用 `mooglemail:world:send:mid`；
Return/Take 使用独立 action proof。baseline/scan、精确匹配和结果未知边界见[Sent 证明](ops/mail/diagnostics.md#主动-composesend-的-sent-证明)，不新增 HTTP 路由。

### Recovery 与实时运维

恢复健康中的 `balance_status` 与 `evidence_status` 增加成对的 `not_applicable`：UID 账户为 `record_only` 时自动刷新仅更新恢复计数，不读取余额。
旧余额及观察时间保留为历史，管理页显示“仅记账，无需核对余额”并隐藏旧数值和余额证据年龄；真实履约失败及恢复积压继续参与健康判断。

A14 为 PostgreSQL 完整档的版本 fence、证据复核、账本、退款激活、guarded relist、审计及预览目录事件事务；结算为获胜 lot 同事务建立 recovery case，Worker 按 `case → return → action → relist → refund` 持久化处理；已付款退货的退款确认要求 handoff 义务为 `ready` 且未满足，仍须完整取回证据，不能以付款前 `blocked` 替代；Delivery 人工 retry/reconcile-retry 在原命令事务恢复匹配的 Recovery action 与父任务阻断，不改变外部成功证明门禁。真实 MoogleMail 回执与部署证据见[验证状态](docs/status.md)。

A21–A24、A43 提供事件元数据、lag/failure 与 replay 安全边界，未装配端口返回 501；A24 命令重放、兼容 ID 派生与追加事件/审计见[事件契约](docs/contracts/events.md#worker-消费游标与重放边界)，不代表外部广播成功。A25 的 GET、POST 候选校验和 PUT 发布由规则 owner 拥有；PUT 以 `command_id`、`expected_revision`、请求摘要在 Serializable 事务做 revision CAS、账本与审计。
Admin delivery、realtime failure/lag 的数字分页在同一 `REPEATABLE READ READ ONLY` 快照中取得计数与记录，
避免并发更新使两者矛盾；筛选、顺序、数量上限及响应字段不变。这增加事务控制开销，不表示查询已测得加速。

### 论坛预览与审计读取

A45/A46 需要 Admin 会话、拒绝非空查询，始终 `no-store`；只读取本地 `PublicForumReader` 安全投影，不调用论坛、不创建恢复命令、不推进 Worker 状态；分别复用 P21/P15 的 DTO 形状，时间沿用 Admin 固定六位微秒格式。A45 无有效 Worker 观测返回 `503 forum_sync_unavailable`；未装配存储、读取失败或无效投影返回 `503 storage_unavailable`，后端明确能力缺失返回 `501 feature_not_supported`。A46 接受规范非零 UUID，否则 `400 invalid_auction_id`；无可读镜像为 `404 not_found`，不是发布失败；管理页可手动刷新本页拍卖，但不能用刷新解除人工写入门禁；分页主题的 `body` 仍是首帖正文，不拼接目录续页。

A05 已接入 Admin session、origin/CSRF 边界和 2 MiB 请求体上限；请求必须在 `html` 与 `text` 中恰好选择一个非空输入，由本地论坛解析器返回 catalog、posts 和 warnings。catalog 合并紧邻首帖、同作者且拍卖/页码标记匹配的续页，posts 保留全部回帖及真实顺序，HTML 路径要求同一数字作者 ID；纯文本预览只比较可选作者标签，不能进入 Worker 远端导入路径。不根据正文标记授予写入权限或跳过用户命令；该接口只做内存预览，不抓取远程页面、不发布回帖、不写数据库，输入无效或解析失败 400。远程主题发布/读取、回帖竞价导入和通知由 Worker 的独立论坛 owner 负责，已接入逐主题持久化调度/健康、命令账本和不确定结果停靠；历史世界级回帖 cursor 只保留历史 schema，不是当前写入 owner。

A33 已接入 Admin session 和只读安全投影，保留 actor/action/entity/source/result 精确筛选、occurred 时间范围、keyset opaque cursor 与 `page` 数字分页；数字页在同一数据库读取事务内取得 total 与记录，keyset 页不伪造 total；metadata 仅允许 JSON object，数据来自追加式审计表。

### 拍品管理与撤拍义务

A13 已接入 Admin session、CSRF、空查询校验和稳定 `command_id`；`action` 只允许 `withdraw` 与 `set_start_price`，后者必须提供正的 `start_price` 并携带 `expected_state_version`。PostgreSQL 完整档在同一 Serializable 事务锁定 world/auction/lot，完成 lot 版本 CAS、命令账本、审计和预览 realtime 事件；withdraw 还创建 `withdrawn_return` 义务及 pending/send fulfillment，由 Worker 在提交后将该 lot 的原装备在原世界单独寄给原卖家，Credits/COD 均为零。成功响应为 `200` 的 `{ lot: ... }` 安全投影，只确认撤拍和寄回义务入队，不表示远端已寄达；投递与领取证明由履约流程确认。

### Dashboard 与 readiness

A02 已接入 Admin session、严格空查询校验和 PostgreSQL 的 `service_heartbeats` 安全读取，health/process section 返回真实心跳聚合；Worker control 通过配置的 Unix socket 读取 live/ready/status，并将 unavailable/timeout/incompatible/rejected 映射为有限 condition，不暴露 transport 错误正文。mail、realtime、delivery section 分别读取全量轮询器、consumer lag/failure water、delivery pending/failed/manual-reconcile 水位及年龄字段；
fulfillment、recovery 复用 operations-alert source 的履约/恢复安全水位；credential aggregation 和 alerting section 也已接入安全投影。已登记 dashboard section 都有 owner；owner 未装配或读取失败返回 `unknown`，不伪造状态。三个进程装配根已有启动和周期心跳路径，SIGINT/SIGTERM 下停止周期任务并发布停止心跳；每轮周期心跳重新检查数据库和全部内嵌 migration，读取进程内饱和的审计写入失败计数，并由各自 tracing owner 提供安全状态；Public 同时检查 relay 新鲜度，Worker control 同时投影论坛循环的终止故障和可恢复降级。
数据库、schema、审计完整性或必须运行的 relay 异常令 readiness 为 false，tracing degraded 令 heartbeat status 降级但不单独改变应用 readiness。外部广播不属于当前 dashboard 契约。
Worker control 的 `ready/state` 来自同一原子快照；论坛恢复只清除论坛降级原因，不能覆盖其他故障或不可逆的 drain。

fulfillment 的 `pending` 只统计业务状态为 `ready` 且执行阶段为 `pending/prepared/enqueued` 的义务，与 Worker
可领取的业务状态一致；`oldest_backlog_age_seconds` 由这些义务及独立的未终结 `blocked` 义务计算。
尚未触发的 `contingent` 退款不计入待处理或积压年龄，原业务与执行状态保持不变；`blocked`、`failed`、
`manual_reconcile` 的独立计数保持原语义。Dashboard 与 operations alert 使用同一权威水位，不增加响应字段。

### 邮件审查与轮询

A16-A19 已接入 PostgreSQL 完整档：轮询状态和入站审查读取使用邮箱边界、严格查询校验与安全 DTO，列表读取保持同一读取快照；暂停/恢复使用 Admin session/CSRF、`command_id` 和 state-version fence，在 Serializable 事务内写入 `mail_poll_state`、`mail_admin_commands` 与追加审计。A17 列表字段仅增加可空 `sender`/`subject` 与 `has_message_display`，不返回正文；A18 详情返回同样的 `sender`、`subject`，`message_display` 只返回 schema/redaction version、已脱敏正文、附件外部 ID、Credits 和 COD；
被拒绝装备邮件还可返回非权威 `equipment_diagnostic`，含装备 ID、1-based 附件位置、stage/category、可选 detail_code、真实可用的 HTTP status、受限耗时、evidence UUID 和 retention status/error，不进入决定 metadata 或 cursor。详情还返回有界 `deliveries` 关联任务表，字段为 `id`、`operation`、`recipient`、`status`、`fencing_version`、`attempt_count`、`failure_code`、可省略的 `reason_code`、`external_reference`，只由 `metadata.delivery_ids` 批量查询，并同时匹配 mail_intake 来源、world、external MID、operation 和 origin_id；
A17 列表不增加该字段，不返回 payload、装备 key 或正文。上限为正文 64 KiB、JSON 128 KiB、附件 1000 条，装备 key 和认证邮件正文隐藏；该投影只用于敏感审查，不参与幂等或 cursor，响应 `no-store`，历史 NULL 不由普通 replay 补写。A20 在至少一个邮箱已装配时返回 `202 accepted`：请求经 Admin session/CSRF 与单个 `command_id` 校验后，经 Worker control 全局唤醒已装配 poller，返回 `command_id` 与 `pollers`；空快照返回 `409 poll_rejected`；
手动触发让已暂停邮箱执行一次领取，但不修改 `paused` 状态、不隐式恢复自动轮询，也不表示外部读取已完成。

### Runtime 配置与文件事务

A26/A27 已接入 PostgreSQL 完整档：GET 返回非敏感 TOML 文档的 JSON 投影和稳定 `sha256:` fingerprint，
POST `:validate` 只执行严格候选校验；PUT 以 `command_id` 与 fingerprint fence 写入 durable pending/applied/unknown 账本，
并通过同目录临时文件、权限校验、fsync、rename 和目录同步原子替换。文件替换后回读或账本确认失败，保留新文件并返回
`503 reconciliation_required`；写前读取和校验失败仍用原分类。同一命令重试或启动恢复按实际 pending/applied 状态向前收口，
不能把提交确认丢失当成可回滚旧文件。文件读取预算和锁身份由[配置手册](ops/config/README.md#runtime-共享与重启)定义。
候选写入复用当前部署启动侧 listener、规范 Origin、metrics、session 和 tracing 校验；已配置 Admin OTLP headers
时不能通过 runtime 关闭 tracing，须先同步敏感部署配置。该门禁只传递 headers 是否存在，不把秘密材料带入候选或文件。
启动恢复任何失败（含结果未知或需人工核对）都阻断 Admin 监听，仅恢复成功且文件指纹匹配启动快照才继续装配。成功更新返回 `restart_required`，不伪造热加载。运行时文件路径由三角色主 TOML 的 `runtime_config_file` 选择；Public 仅消费装备标色规则用于公开分组，Admin/Worker 继续按各自边界应用其余字段；文档还含兼容 main/isekai、发件人 allowlist、装备过滤和 14 个通知模板，Worker 对 MoogleMail 只把这些业务字段投影为启动快照；transport、凭据、mailbox identity 和全局启用开关仍由 Worker 主配置拥有，敏感 TOML、Cookie、密钥和连接串不进入该文档。
Admin 重启后 `allow_manual_early_open` 与 `allow_manual_early_settlement` 分别决定人工提前开放、人工提前结算 capability 和对应锁内校验，两者默认关闭，计划时间已到的正常开放仍允许。`[equipment_source]` 冻结为装备来源适配器的 host/path allowlist、响应上限、共享请求间隔、全请求超时及 Admin 手工录入的可交易装备开关（默认关闭且不放宽来源安全或邮件过滤）；固定 HTTPS origin、无重定向、精确 world 路径及流式有界读取始终失败关闭。Admin WebUI 把该闭集投影映射为分组开关、数字、列表与文本输入，保存仍提交完整候选文档，因此所有页面修改最终由 PUT 的原子文件替换边界写回，不存在只在浏览器内生效的旁路。
更新和启动恢复从首次读取到文件/账本收口持有同一跨进程 workflow 锁，锁竞争返回 `503 storage_unavailable`，客户端可复用原 command ID 重试。

GET `/api/v1/admin/config`、POST `/api/v1/admin/config:validate` 与 PUT `/api/v1/admin/config` 均接受可选
`include_equipment_taxonomy=true`，只扩展响应；重复、未知参数及其它值返回 `400 invalid_query`。
省略时维持原有 `document`、`fingerprint`、`restart_required` 三字段集合，兼容旧客户端严格解码。
显式选择时增加 `equipment_taxonomy`：`revision`、`categories`、`qualities[{name,legacy}]`、
`kinds[{name,category,material,slot,legacy,prefixes,suffixes}]` 和
`materials[{name,category,prefixes,suffixes}]`；`kinds` 的 `material`/`slot` 仅在 Cloth、Light、Heavy 同时有值，
武器、法杖、盾牌均为空；部位使用 `head/body/hands/legs/feet`。
词典条目由领域唯一提供：品质候选仅投影 `Peerless`、`Legendary`、`Magnificent`，名称解析仍保留其它品质的兼容识别；
`kinds` 保留完整基础名索引以关联旧精确规则及词缀；Admin 新种类候选仅取其中无部位的武器/法杖/盾牌名称，
加上 `materials` 提供的十二个护甲种类（如 `Phase`），不把 `Phase Robe` 等种类与部位的组合列为新候选。
有限 Chain/Reactive 语法不展开成确证基础名。
本次文档标色规则的 `equipment_type`/`equipment_types` 若包含合法有限兼容完整名，另附
`resolved_kinds[{name,category,material,slot,prefixes,suffixes}]`；仅由领域 `resolve_kind` 识别并规范化，
最多 16 项、忽略 ASCII 大小写去重，与确证种类及材质族名称互斥，`material`/`slot` 为非空字符串。
该集合为空时省略，旧投影缺省为空；GET 使用读取文档，校验和保存响应分别使用本次候选或已保存文档，
不只关联已保存的旧值。HTTP 用进程内 `OnceLock` 缓存固定词典主体，每次克隆后附加这份文档相关集合，
不缓存文档或改变既有 `no-store` 响应头；附加集合也不改变主词典 `revision`。
Admin 显式请求并校验该投影，缺失或无效返回页面既有加载错误；候选版本变化只更新推荐元数据，不清除草稿或已选历史值。
已选护甲完整名（含 `resolved_kinds`）继续回显并精确关联词缀，用户手动取消后不再列为新种类候选。
词典不进入 runtime 文档、配置指纹、命令摘要、持久化响应或装备快照，校验和更新请求体保持原形状。

`equipment_colors.rules[]` 增加可选数组 `categories`、`equipment_types`、`qualities`，分别承载装备类型、
具体种类和品质的多选条件；省略或空数组沿用该维旧的 `category`、`equipment_type`、`quality` 单值，
两者均为空表示不限。同维单值与非空数组不能同时提供；数组内任一项命中即可，不同维度之间必须同时满足。
新 `equipment_types` 数组也允许十二种护甲材质：Cotton、Gossamer、Ironsilk、Phase、Leather、Drakehide、
Kevlar、Shade、Plate、Chain、Reactive、Power；材质选项匹配所属护甲大类下该材质的全部部位，
具体装备名称仍须全名匹配，旧单值 `equipment_type` 保持原有精确匹配语义。
Admin 表单把旧单值恢复成一个已选项，保存多选时清空相应旧字段；不把一条规则拆成多条组合规则。
旧 `equipment_type` 恰为材质名时，未主动编辑该维度继续保存原精确单值；主动修改种类选择后才转换为数组条件。
表单切换类型或种类只刷新推荐候选，所有其它维度已选值继续可见并保留，只有用户显式取消才移除。
推荐词表不充当已有规则的合法性白名单，避免历史种类被清空成不限条件，或合法历史前后缀被自动删除。
空数组省略序列化以保留旧文档指纹和命令摘要，已有前后缀、颜色及优先级语义保持原样。
实际论坛渲染规则由 Worker 启动时载入；首次保存多选前，Admin、Worker 和读取 runtime 的 `auctionctl`
必须部署支持数组的同版程序，保存仍返回 `restart_required`。

### Runtime 六组接管

runtime 文档提供可选 `operations`，响应投影补齐六组候选参数：`economics`（后续新拍卖底价、增幅、软截止、取消与代理策略）、`schedule`（自动周排期）、`forum`（论坛开关与有界轮询）、`alerts`（告警阈值、通知目标与批次边界）、`worker`（生命周期、投递、履约、恢复循环的吞吐与重试）、`mail`（邮件 cron、读取批量/分页、HTTP 与批次处理预算、租约续期、读取/处理尝试和退避）。每组 `managed` 默认 `false`（沿用公共/角色 TOML），只有设为 `true` 才在对应进程重启时覆盖该组字段；首次 GET 的候选值不代表未接入 runtime 的 Worker 当前配置。
严格候选校验覆盖金额网格及 1B 底价上限、周时间顺序、分页请求预算、并发/批量、租约/超时、告警冷却与收件人；邮件 cron 校验复用 Worker 配置解析器并要求存在未来触发时刻，无效候选在新命令写入账本前拒绝。`mail` 完整字段、单位、默认值和范围见 `ops/config/examples/runtime/admin.toml`：读取与处理超时必须大于 HTTP 超时，续租周期必须小于租约，退避上限不能小于基础退避，`mail.batch_timeout_seconds` 范围 1–86400 秒且至少为 `fetch_timeout_seconds + item_timeout_seconds`，省略时 600 秒并以省略序列化保留旧 fingerprint，Admin 闭集 decoder 和表单接受缺省值；
该预算覆盖一轮拉取及业务处理，可在重试耗尽前结束，完成前缀与其 cursor 由 Worker 原子提交。缺少整个 `operations` 或其中 `mail` 的旧物理文件和 durable 候选保持原指纹，显式 `mail:null` 被拒绝；物化只发生在浏览器投影，不改写历史命令，API envelope、命令摘要版本和 `restart_required` 语义不变，不增加热加载路径。保存中断后 pending 命令与旧文件指纹一致时继续完成同一候选，不永久占用唯一未完成命令槽；若启动恢复补写了文件，启动校验要求重新加载恢复后的配置，未证明的冲突仍需人工核对。

### 数据库业务世界

A40/A41 将数据库 `worlds.enabled` 与 Admin runtime 中只控制 Worker/MoogleMail 入站的 world 开关明确分离。A40 是无 query 的认证只读目录；A41 使用严格 JSON、`command_id` 与 `expected_policy_revision`，在 PostgreSQL Serializable 事务中先锁 world，再拒绝任何 `draft`/`preview`/`live`/`final`/`settling` 拍卖或仍可领取的活动 `scheduled_rounds`，随后以 revision CAS 更新状态，并把通用命令账本和追加审计与状态同事务提交；相同状态是可重放的有审计 no-op，不递增 revision。

### 用户、改名与黑名单

A28 已接入 Admin session、CSRF、列表筛选分页、用户安全投影及黑名单次数/最近历史/创建时间列表字段、创建、改名、稳定论坛账号 ID 修改和 key 撤销/重签；名称是可变的 MM 收件人与页面显示值，`forum_account_id` 是唯一的规范十进制 `showuser` 账号 ID。A30 已接入改名记录校验、state-version fence、命令账本和名称占用冲突处理；A31 黑名单 PUT/DELETE 已接入黑名单当前状态、追加历史、版本 fence 和审计；三组命令的 key 明文不进入命令账本、审计或错误响应。A29 继续提供改名历史安全投影，并额外返回当前用户版本供 A30 建立 fence；管理端 `/users` 页面提供列表、创建、改名、论坛账号 ID 修改、key 撤销/重签、黑名单和改名回滚。回滚首次按精确 `user_id` 读取版本并冻结含理由、版本与 `command_id` 的完整请求；结果未知时继续重放同一请求，不按用户名子串或新读取版本重新拼装，成功或明确拒绝后才释放该请求。撤销或重签 Key 同事务永久撤销旧 Public 会话，邮件重签遵守相同语义。

### 外部凭据

A32 已接入 Admin session、origin/CSRF、`command_id` 和 `expected_state_version`，支持 `mail_cookie`/`forum_cookie` 的安全投影列表、导入、改名、启停和删除。secret 只在应用加密边界短暂存在，不进入响应、审计、命令结果或页面；该路由要求 PostgreSQL 与独立 `credential_data_key`，未配置密钥时返回 `501 feature_not_supported`；Worker 的 active 凭据读取不属于浏览器 DTO，`.keyenv` 只作兼容回退入口。`mail_cookie` 与 `forum_cookie` 各自全局最多一条 active，导入或重新启用同类第二条返回 `409 credential_conflict`，调用方必须先经同一路由停用旧记录并重启 Worker 完成交接。

A32 导入与安全投影支持可省略/nullable 的 `player_name`，仅 `mail_cookie` 接受非空完整游戏玩家名称（非数字 UID）：Some 不含控制字符、无首尾空白且最多 256 UTF-8 字节，不 trim、不折叠大小写或猜别名，forum 设置即拒绝；None 不改变旧导入命令摘要，旧账本/DTO 缺字段按 None 解码，改名及启停不修改绑定。这是预期声明，不是 verified 状态；现有凭据保持原记录与密文，本轮没有补绑定写入口。当前 gateway 即使预期名 None 也读取同 MM 认证 UID 与论坛目标资料名，有值精确匹配，缺证据返回内部稳定码 `identity_unproven`；内部提交要求数据库 active ID/version、generation 和组合证明，不新增身份核验 HTTP 路由。A44 的 Valid 仅为可用性观测，Item Shop 余额也不是自身名称证明，不能授权内部免寄。

A44 只校验已保存凭据，不接受原始 Cookie 或远端 URL。请求是严格 JSON `{ "expected_state_version": 1 }`，最多 4 KiB，无查询参数；保留 Admin session、Origin、CSRF 与 mutation 限流，尽管使用 POST 防止跨站/预取触发，操作只读，不使用 `command_id` 或持久化命令账本；active、disabled、invalid 均可检查，不改变启停、Worker 所有权、版本、审计或重启要求。响应使用标准 `ApiResponse`，`data` 的必填闭集为 `credential_id`、`state_version`、`checked_at`（UTC RFC3339）和 `checks`；
每项仅含 `world` 与 `status`，`status` 为 `valid`、`invalid`、`inconclusive`。论坛固定一项 `shared`，只证明登录态，不证明发帖权限；游戏按凭据范围检查 `main` 或 `isekai`，shared 固定按 main、isekai 顺序各返回一项。网络、超时、限流、战斗页、403/Cloudflare、未知页面或格式无法确认返回 `inconclusive`；只有明确登录失效证据才返回 `invalid`，两者仍为 HTTP 200，不触发 Admin 退出登录。探测固定 HTTPS 论坛用户控制台与游戏 Item Shop 余额页，不跟随重定向、不自动重试、不打开邮件或执行发帖、compose、send、take、return。
每个 Admin 进程最多一个校验在飞，各站点请求起始间隔至少 2 秒，每个范围连同等待最多 5 秒、响应最多 2 MiB；并发检查返回 `429 rate_limited`（`Retry-After: 2`）；探测前后复核数据库版本，变化返回 `409 stale_credential_state`，删除返回 404；密钥/校验适配器失败 503，未装配端口 `501 feature_not_supported`。结果为当次观测、响应 `no-store`，不落库、不回显 Cookie/账号/余额/HTML，不保证后续请求可用。

## 契约变更规则

1. Rust 路由必须先在本表登记，再实现 handler；已发布路径不得为了方便直接重命名。
2. DTO 必须在 auction-contracts 中独立定义；不得把 SQL 行或 Rust storage model 直接暴露给浏览器。
3. 所有改变状态的核心命令必须保留 command_id；同一 ID 搭配不同 request hash 必须返回 409。A24 v1 兼容请求没有该字段，由 HTTP 防腐层按 failure 与修正负载稳定派生，不得使用随机 ID。
4. 公开实时更新不以 HTTP 分页替代 WebSocket；无法保持长连接时按 events catch-up 语义恢复。SSE 尚无登记路由，不能将该扩展方向描述为现有客户端通道。
5. 本表的状态码、JSON 字段和错误 code 变化必须同时更新本文件、相关契约记录和对应 owner 的边界说明。
