# HTTP 契约清单

## 基线与统计口径

本清单冻结于 2026-08-22，行为基线为相邻旧仓库 Auction-BF 的提交
f109146e246c6bbfefd7f4749ac2bba087bcfa0b。来源是旧仓库
apps/backend/internal/app/public/features/features.go、apps/backend/internal/app/admin/features.go
以及各 feature 的 publichttp/adminhttp 注册代码。

本表统计 feature API 的唯一路径，不把进程健康检查、静态 Web 资源和反向代理自身的路径
计入。逐项核对后的数量是：

- Public：20 个唯一路径；
- Admin：37 个唯一路径。

迁移实施计划中原来的 Admin 35 条是旧统计低估，不能继续作为验收数字。

进程级可观测性路径不计入上述 feature API 统计：Public 与 Admin 在主配置
`[metrics].enabled = true` 时提供 `GET/HEAD /metrics` Prometheus text scrape；Admin 若配置
`runtime_config_file`，以 runtime 文件的 `[metrics].enabled` 为准。指标关闭、
带 query 或路径变体均返回 404。Worker 不开启 TCP HTTP 监听；启用 Worker control 时，
同一经过 peer credential 校验的 Unix socket 提供 `GET/HEAD /metrics`，使用同一进程 registry。
control 未启用或指标关闭时该路径返回 404。

OTLP/HTTP tracing exporter 不是业务 HTTP 路由，不计入上述统计。Public/Worker 从主配置的
`[tracing]` 读取，Admin 在配置 `runtime_config_file` 时以 runtime 的 `[tracing]` 为准；角色专属
OTLP headers 只能放在敏感 TOML 中。tracing 默认关闭，启用时使用有界异步队列，并在进程停止
时按限定等待时间刷新和关闭；exporter 不会把 headers 写入日志、指标或 runtime 文件。三类进程
已经统一订阅 SIGINT/SIGTERM，先执行 10 秒有界业务/HTTP drain，再进入配置限定的 exporter
刷新；排空期间第二次信号会强制收口。

## 进程健康探针

健康探针不计入 Public/Admin feature API 数量，但属于旧版部署契约，两个 HTTP surface 都提供：

| 方法 | 路径 | 成功 | 未就绪 | 数据 |
| --- | --- | --- | --- | --- |
| GET/HEAD | /health/live | 200 | — | `{"live": true}` |
| GET/HEAD | /health/ready | 200 | 503 | `{"ready": true/false}` |

两条路径均使用统一 `{code,message,data}` 外壳。`/health/live` 不访问下游依赖；`/health/ready`
通过应用层 readiness 端口只读检查数据库连接和当前二进制的全部 SQLx 迁移。未配置数据库或迁移
不完整时返回 503，不把启动骨架伪装成可接收业务流量。

旧 HTTP 层使用统一响应外壳：

    {"code": "...", "message": "...", "data": ...}

成功响应的业务 code、JSON 字段和错误 code 以旧 handler 为准。下表的错误状态码是该
handler 当前明确产生的稳定集合；依赖错误中的 504 表示 context deadline，503 表示
其他依赖不可用。冻结契约要求路由器对未注册路径/方法返回 404/405；Rust 路由现已按显式浏览器
清单封闭 SPA 入口，不再用宽泛 fallback 掩盖未知路径。

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
| 429 | 限流或有界连接资源耗尽 |
| 500 | 服务器内部无法安全完成请求 |
| 503/504 | 下游依赖不可用或处理超时 |

旧版 JSON 写边界同样属于 v1 契约：请求必须使用 `application/json` 媒体类型（允许合法参数），
body 受每条路由自己的上限约束，解码只接受一个 JSON 值，且请求 DTO 拒绝未知字段。非 JSON、超限、
语法/未知字段/尾随值分别稳定映射为 415 `unsupported_media_type`、413 `body_too_large` 和
400 `invalid_json`。所有错误外壳固定包含 `data`；无业务错误数据时序列化为 `null`，Rust 可继续
附加不含敏感内容的 `request_id`。

Feature API 的方法是严格闭集。GET 路由不会因为 Axum 默认行为额外接受 HEAD；未显式注册的方法
返回 JSON `405 method_not_allowed`，`Allow` 只列出该路径真实注册的方法并沿用旧版排序。健康探针、
metrics 和静态文件是否接受 HEAD 由各自契约单独决定，不从业务路由推导。

Public/Admin 已恢复统一 HTTP boundary：只接受 1 至 64 字节的 ASCII 字母数字/`-_.`
`X-Request-ID`，缺失时生成并在响应回传，重复或非法值返回 `400 invalid_request_id`；同一值会进入
Public 竞价/代理、Admin 生命周期和管理 mutation 的追加式审计，但不参与命令请求哈希。边界还
提取 W3C trace context，记录请求级访问日志，隔离 panic，设置安全响应头，执行默认 15 秒处理
超时和固定 5 秒 HTTP/1 请求头读取超时。`/api/v1/ws` 不受处理超时限制。客户端地址只在 TCP peer
命中配置的规范 CIDR，或 Public/Admin Unix listener 显式启用可信上游时解析 `X-Forwarded-For`；非法链
整体丢弃。CSP 只允许同源脚本及 WASM 编译；内联样式例外用于 Dioxus 首屏防闪样式和组件 style
属性。登录/变更限流器按该地址工作，最多保留 4096 项并确定性淘汰最久未访问项。

未注册的 `/api`、`/health` 保留路径固定返回 `404 not_found` JSON；其他未知请求在 `Accept` 明确
允许 HTML 时返回最小页面 404，否则返回同一 JSON 404。静态文件和批准的浏览器路径只接受
GET/HEAD，其他方法返回带 `Allow: GET, HEAD` 的 JSON 405。Public 15 条（14 条旧版路径加独立
`/forum` 页面）、Admin 21 条浏览器路由由两个 Dioxus `Route` 枚举与
`auction-http/src/assets/routes.rs` 同步维护，只有这些路径可以读取 `index.html`；批准路径缺少
构建产物时返回 503。

缓存策略沿用旧版安全边界，并适配 Dioxus 稳定 WASM 文件名：Public 非身份化读取为 `no-cache`；
Public events/session/账户/竞价/代理与全部 Admin 业务接口为 `no-store`，错误响应使用相同策略；
浏览器路由入口为 `no-store`，直接静态文件中只有 `assets/` 指纹资源使用一年 `immutable`，
`index.html`、`wasm/*.js`、`*.wasm` 和其他稳定文件名使用 `no-cache`、`Last-Modified` 与
`If-Modified-Since`。未知 404/静态 405/产物缺失 503 均为 `no-store`。

## Public

| 编号 | 方法 | 路径 | 认证/边界 | 成功 | 明确错误状态 | 旧代码来源 |
| --- | --- | --- | --- | --- | --- | --- |
| P01 | GET | /api/v1/worlds | 公开 | 200 | 400, 503/504 | features/world/publichttp/adapter.go:36 |
| P02 | GET | /api/v1/rules | 公开；`lang` 可选 | 200 | 400, 503 | features/rules/publichttp/adapter.go:40 |
| P03 | GET | /api/v1/events | 公开读取；由 auction boundary 校验；PostgreSQL/SQLite 从持久化事件表回放 | 200 | 400, 404, 503/504 | features/realtime/publichttp/adapter.go:148 |
| P04 | GET | /api/v1/ws | Public session；只读服务端推送；连接范围必须明确 | 101 | 400, 403, 404, 429, 503/504 | features/realtime/publichttp/adapter.go:151 |
| P05 | GET | /api/v1/blacklist | 公开；页码分页 | 200 | 400, 503/504 | features/users/publichttp/adapter.go:43 |
| P06 | POST/GET/DELETE | /api/v1/session | POST 登录；GET/DELETE 会话边界 | 200 | 400, 401, 403, 409, 429, 503/504 | features/sessions/publichttp/adapter.go:89 |
| P07 | GET | /api/v1/auctions | 公开 | 200 | 400, 503/504 | features/auction/publichttp/adapter.go:58 |
| P08 | GET | /api/v1/auctions/current | 公开 | 200 | 400, 503/504 | features/auction/publichttp/adapter.go:59 |
| P09 | GET | /api/v1/auctions/{auction_id}/lots | 公开 | 200 | 400, 404, 503/504 | features/auction/publichttp/adapter.go:60 |
| P10 | GET | /api/v1/auctions/{auction_id}/bid-log | 公开 | 200 | 400, 404, 503/504 | features/auction/publichttp/adapter.go:61 |
| P11 | GET | /api/v1/lots/{lot_id} | 公开 | 200 | 400, 404, 503/504 | features/auction/publichttp/adapter.go:62 |
| P12 | GET | /api/v1/price-history | 公开；成交事实仅由 PostgreSQL 完整档提供 | 200 | 400, 501, 503/504 | features/auction/publichttp/adapter.go:63 |
| P13 | GET | /api/v1/compensation-pool | 公开 | 200 | 400, 501, 503/504 | features/auction/publichttp/adapter.go:64 |
| P14 | GET | /api/v1/compensation-pool/{auction_id}/movements | 公开 | 200 | 400, 404, 501, 503/504 | features/auction/publichttp/adapter.go:65 |
| P15 | GET | /api/v1/auctions/{auction_id}/forum | 公开 | 200 | 400, 404, 501, 503/504 | features/auction/publichttp/forum.go:37 |
| P16 | GET | /api/v1/me/bids | Public session | 200 | 400, 401, 503/504 | features/auction/publichttp/account.go:90 |
| P17 | GET | /api/v1/lots/{lot_id}/proxy-authorizations/mine | Public session | 200 | 400, 401, 404, 503/504 | features/auction/publichttp/account.go:97 |
| P18 | POST | /api/v1/lots/{lot_id}/proxy-authorizations | Public session、origin、限流 | 200 | 400, 401, 403, 409, 413/415, 429, 503/504 | features/auction/publichttp/account.go:104 |
| P19 | POST | /api/v1/lots/{lot_id}/proxy-authorizations/{authorization_id}/lower | Public session、origin、限流 | 200 | 400, 401, 403, 409, 413/415, 429, 503/504 | features/auction/publichttp/account.go:111 |
| P20 | POST | /api/v1/lots/{lot_id}/bids | Public session、origin、限流 | 200 | 400, 401, 403, 409, 413/415, 429, 503/504 | features/auction/publichttp/account.go:118 |

Public 分页必须保留旧的 page/page_size 与 cursor 语义。P09 的 lot 投影允许读取仍属于公开
安全边界的 settled 拍卖；响应上下文固定包含 `auction_id`、`world`、`number`、状态、展示模式、
开标时间、可选有效结束时间和 `preview_revision`，Public 历史页面据此打开指定轮次而不读取数据库行。
P10 竞价日志额外保留
`world` 与 `lot` 筛选；其条目按 `created_at`、`lot_id`、`accepted_state_version`、
`fact_index`、`id` 降序排列，cursor 是绑定这五个字段的版本化 opaque 值。带 cursor 的返回使用
next_cursor/has_more，不得把未知总数伪装成精确 total。P12 在 PostgreSQL 完整档保留 `name`、`world`、
日期、价格、buyer/seller 筛选，编号页附带完整过滤集 `stats`，cursor 页不返回统计；SQLite Lite
因为没有 settlement 事实链固定返回 `501 feature_not_supported`，不能把状态列或空集合当作成交证明。实时事件的 after 也是
非负 stream sequence，不得改成时间戳游标。P03 的旧版单拍卖读取要求规范 `auction_id`，可带
`after`、`page_size`，默认页大小 100、上限 200；响应除 `events`、`next_after`、`has_more`、
`refetch_required` 外必须返回相同 `auction_id`。发现流是显式 `scope=auctions` 扩展，不能靠省略
`auction_id` 隐式进入。HTTP 外层 envelope 允许非破坏性扩展，但 `data`、事件和 payload 使用精确字段
闭集；缺口/坏投影会隐藏坏事件、置 refetch 并继续扫描有界原始页，`next_after` 指向原始高水位。
P04 查询只能含一个 `auction_id` 或 `scope=auctions`，每帧发送一个不含内部 `stream_key` 的旧版事件对象；
启用 allowlist 时要求恰好一个可信 `Origin`，独立连接尝试限流之后按 origin→query→session 返回错误。
frame/message 上限均为 4 KiB，Ping/Pong/写入期限为 50/60/5 秒；1008、1013、1002、1001 及 reason
分别表示客户端写入、慢消费者、事件合同错误和服务 drain，其余读超限、Pong 超时及底层读写失败直接断开。
只有 discovery 使用每进程 128 槽和 429 `discovery_busy`；单拍卖新连接拒绝 settled，但既有连接可接收
`auction.settled`。断线补发只走 P03，refetch 保留恢复游标和现有 socket。

所有 Public JSON 时间沿用旧 Go `time.Time` 的 RFC3339Nano 词法：数据库或新版生成的 UTC
时间使用 `Z`，小数秒裁掉尾随零，整秒不附加 `.000000`。P18-P20 从旧命令账本重放时还必须
保留账本字符串原有的 offset 与纳秒精度，同时以解析后的 UTC instant 做一致性校验。该规则不
改变 PostgreSQL 关系字段的微秒精度，也不改变 Admin 既有的固定六位微秒投影。

## Admin

除 Admin session 本身外，下表路径都经过 Admin session；mutation 还经过 origin、CSRF、
限流和 actor 注入。HTTP boundary 规范化后的 `request_id` 已注入支持审计的管理命令和生命周期
命令；Worker 内部命令保持 `None`，避免把 `command_id` 冒充请求关联。具有多个 HTTP 方法的路径
必须按方法分别保留成功状态。

| 编号 | 方法 | 路径 | 成功 | 明确错误状态 | 旧代码来源 |
| --- | --- | --- | --- | --- | --- |
| A01 | POST/GET/DELETE | /api/v1/admin/session | POST/GET/DELETE 200 | 400, 401, 403, 429, 503/504 | features/sessions/adminhttp/adapter.go:89 |
| A02 | GET | /api/v1/admin/dashboard | 200 | 400, 401, 503/504 | features/servicehealth/adminhttp/adapter.go:112 |
| A03 | GET/POST | /api/v1/admin/auctions | GET 200；POST 201 | GET: 400, 401, 501, 503/504；POST: 400, 401, 403, 409, 413/415, 501, 503/504 | features/auction/adminhttp/adapter.go:186 |
| A04 | POST | /api/v1/admin/worlds/{world_id}/auction-intake/items | 201 | 400, 401, 403, 409, 413/415, 503/504 | features/auction/adminhttp/adapter.go:191 |
| A05 | POST | /api/v1/admin/auction-forum/preview | 200 | 400, 401, 403, 413/415, 503/504 | features/auction/adminhttp/adapter.go:197 |
| A06 | GET | /api/v1/admin/auctions/{auction_id}/lots | 200 | 400, 401, 404, 501, 503/504 | features/auction/adminhttp/adapter.go:203 |
| A07 | GET | /api/v1/admin/auctions/{auction_id}/settlement-report | 200 | 400, 401, 404, 501, 503/504 | features/auction/adminhttp/adapter.go:208 |
| A08 | GET | /api/v1/admin/auctions/{auction_id}/recovery-report | 200 | 400, 401, 404, 501, 503/504 | features/auction/adminhttp/adapter.go:214 |
| A09 | POST | /api/v1/admin/auctions/{auction_id}/preview | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/auction/adminhttp/adapter.go:220 |
| A10 | POST | /api/v1/admin/auctions/{auction_id}/open | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/auction/adminhttp/adapter.go:225 |
| A11 | POST | /api/v1/admin/auctions/{auction_id}/cancel | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/auction/adminhttp/adapter.go:230 |
| A12 | POST | /api/v1/admin/auctions/{auction_id}/final-phase | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/auction/adminhttp/adapter.go:235 |
| A13 | PATCH | /api/v1/admin/lots/{lot_id} | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/auction/adminhttp/adapter.go:241 |
| A14 | POST | /api/v1/admin/recoveries/{recovery_id}/confirm | 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 | features/auction/adminhttp/adapter.go:246 |
| A15 | GET | /api/v1/admin/lots/{lot_id}/proxy-events | 200 | 400, 401, 404, 501, 503/504 | features/auction/adminhttp/adapter.go:252 |
| A16 | GET | /api/v1/admin/mail | 200 | 400, 401, 404, 501, 503/504 | features/mail/adminhttp/adapter.go:244 |
| A17 | GET | /api/v1/admin/mail/intake | 200 | 400, 401, 501, 503/504 | features/mail/adminhttp/adapter.go:247 |
| A18 | GET | /api/v1/admin/mail/intake/{intake_id} | 200 | 400, 401, 404, 501, 503/504 | features/mail/adminhttp/adapter.go:250 |
| A19 | PUT | /api/v1/admin/mail/polling | 200 | 400, 401, 403, 409, 413/415, 501, 503/504 | features/mail/adminhttp/adapter.go:253 |
| A20 | POST | /api/v1/admin/mail/polls:trigger | 202 | 400, 401, 403, 409, 413/415, 501, 503/504 | features/mail/adminhttp/adapter.go:256 |
| A21 | GET | /api/v1/admin/realtime/lag | 200 | 400, 401, 501, 503/504 | features/realtime/adminhttp/adapter.go:70 |
| A22 | GET | /api/v1/admin/realtime/failures | 200 | 400, 401, 501, 503/504 | features/realtime/adminhttp/adapter.go:73 |
| A23 | GET | /api/v1/admin/realtime/failures/{failure_id} | 200 | 400, 401, 404, 501, 503/504 | features/realtime/adminhttp/adapter.go:76 |
| A24 | POST | /api/v1/admin/realtime/failures/{failure_id}/replay | 200 | 400, 401, 403, 404, 409, 413, 415, 500, 501, 503/504 | features/realtime/adminhttp/adapter.go:83 |
| A25 | GET/POST/PUT | /api/v1/admin/rules | GET 可带可选 `lang`；GET/POST/PUT 200；创建时 201 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 | features/rules/adminhttp/adapter.go:91 |
| A26 | GET/PUT | /api/v1/admin/config | GET/PUT 200；Lite 501 | 400, 401, 403, 409, 413/415, 501, 503/504 | features/adminconfig/adminhttp/adapter.go:74 |
| A27 | POST | /api/v1/admin/config:validate | 200；Lite 501 | 400, 401, 403, 413/415, 501, 503/504 | features/adminconfig/adminhttp/adapter.go:87 |
| A28 | GET/POST/PATCH | /api/v1/admin/users | GET 200；POST 201；PATCH 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/users/adminhttp/adapter.go:105 |
| A29 | GET | /api/v1/admin/renames | 200 | 400, 401, 503/504 | features/users/adminhttp/adapter.go:112 |
| A30 | POST | /api/v1/admin/renames/{rename_id}/revert | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/users/adminhttp/adapter.go:117 |
| A31 | PUT/DELETE | /api/v1/admin/users/{user_id}/blacklist | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/users/adminhttp/adapter.go:122 |
| A32 | GET/POST/PATCH/DELETE | /api/v1/admin/credentials | GET 200；POST 201；PATCH/DELETE 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 | features/keymanagement/adminhttp/adapter.go:95 |
| A33 | GET | /api/v1/admin/audit | 200 | 400, 401, 503/504 | features/audit/adminhttp/adapter.go:62 |
| A34 | GET | /api/v1/admin/deliveries | 200 | 400, 401, 503/504 | features/delivery/adminhttp/adapter.go:78 |
| A35 | POST | /api/v1/admin/deliveries/{delivery_id}/release | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/delivery/adminhttp/adapter.go:83 |
| A36 | POST | /api/v1/admin/deliveries/{delivery_id}/retry | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/delivery/adminhttp/adapter.go:89 |
| A37 | POST | /api/v1/admin/deliveries/{delivery_id}/reconcile | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/delivery/adminhttp/adapter.go:95 |
| A38 | POST | /api/v1/admin/auctions/{auction_id}/early-settlement | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | 新增能力，旧代码无对应路径 |

## 迁移冻结规则

1. Rust 路由必须先在本表登记，再实现 handler；迁移期间不得为了方便重命名旧路径。
2. DTO 必须在 auction-contracts 中重新定义；不得把旧 SQL 行或 Rust storage model 直接暴露给浏览器。
3. 所有改变状态的核心命令必须保留 command_id；同一 ID 搭配不同 request hash 必须返回 409。
   旧版 A24 wire 没有该字段，由 HTTP 防腐层按 failure 与修正负载稳定派生，不得使用随机 ID。
4. 公开实时更新不以 HTTP 分页替代 WebSocket；无法保持长连接时只允许按 events catch-up
   语义退化为轮询或 SSE。
5. 本表的状态码、JSON 字段和错误 code 变化必须同时更新 migration plan、契约变更记录和
   对应 owner 的边界说明。

截至当前迁移切片，P01、P02、P05、P06、P07、P08、P09、P10、P11、P12、P13、P14、P15、P16、P17、P18、P19、P20、A03 GET/POST、A04、A06、A15、A25 GET/POST/PUT 已接入应用端口；P02 保留 `lang` 语言选择和缺失语言回退英文，P05 保留公开安全投影和页码分页；P07 保留
`page`/`page_size`/`total` 分页和 `cursor`/`next_cursor`/`has_more` keyset 分页，P09
保留旧的 preview/live 互斥 lot 数组、分类统计和页码字段，P11 保留当前公开 projection
下的 lot 详情边界，P10 保留 world/lot 筛选、编号页与 keyset cursor 两种分页形状以及
公开 bid fact 字段，P12 保留成交条目、筛选条件和编号页统计；P13/P14 保留补偿池批次、余额守恒摘要、opaque keyset cursor 和单场账本分页，PostgreSQL 完整档从同一 Serializable 快照读取，SQLite Lite 明确返回 `501 feature_not_supported`；P15 读取本地论坛同步镜像，未把远程网关状态伪造成成功，SQLite Lite 同样明确返回 `501 feature_not_supported`。Public `/forum` 页面单独消费 P15，展示状态、源版本、更新时间和已发布帖子内容；首页不再读取或轮询该投影。P06 的 Public session 已接入
配置、应用端口和 PostgreSQL/SQLite 存储；P16/P17 已接入 Public session、账户读取端口以及
PostgreSQL/SQLite 的普通竞价、代理 quota、转换历史、最近 proxy bid 和 owner capability 投影；P20 的普通竞价已接入 Public session、稳定
`command_id`/`request_hash`、应用竞价事务和旧版竞价结果投影。
Admin 的 `/api/v1/admin/session` 已接入 Basic Auth、CSRF 和持久化会话端口。A03 GET
`/api/v1/admin/auctions` 已接入 Admin 会话保护、`world_id`/`status` 筛选、有限页码和
PostgreSQL Serializable 生命周期/lot 数量安全投影；创建拍卖 POST 已接入零版本 fence、命令账本、world/intake target 锁、策略快照、审计和 `auction.created` 事件；SQLite Lite 的创建与列表 GET 均明确返回 `501 feature_not_supported`。A06
`/api/v1/admin/auctions/{auction_id}/lots` 已接入 Admin 会话保护、有限页码、PostgreSQL
Serializable 同快照的拍卖/lot/装备/投稿安全投影；响应 body 保持旧版
`auction/lots/pagination` 闭集，内嵌新版 Web 通过非破坏性的
`X-Auction-Capability-Manual-Intake` 响应头读取外部写入能力。SQLite Lite 因缺少完整 Admin 生命周期字段
明确返回 `501 feature_not_supported`，不填充伪造的默认生命周期数据。A04
`/api/v1/admin/worlds/{world_id}/auction-intake/items` 已接入 URL/canonical JSON 录入、
固定 HTTPS 来源边界、可选的可交易装备限制、key 密文信封和 PostgreSQL 同一 Serializable 写事务；
`equipment_source.require_tradeable` 默认关闭，允许 Admin 手工录入 Soulbound 装备；开启后在事务前
恢复只接受可交易装备的限制，且不改变 MoogleMail 邮件过滤。成功响应只返回拍卖、投稿、lot 与
装备的安全事实，不返回原始 URL、canonical 正文或 key，SQLite Lite 明确
返回 `501 feature_not_supported`。A15
`/api/v1/admin/lots/{lot_id}/proxy-events` 已接入 Admin 会话保护、页码/opaque cursor 两种分页、
PostgreSQL Serializable 命令账本快照和代理安全事件投影；SQLite Lite 明确返回
`501 feature_not_supported`，管理端 lot 页面已提供历史读取入口。A07
`/api/v1/admin/auctions/{auction_id}/settlement-report` 已接入 PostgreSQL 完整档的一致
只读报告；报告中的 `auction` 保持旧版基础字段闭集，不额外发送严格旧 Web 客户端会拒绝的
`scheduled_end_at`/`effective_end_at`，读取遇到序列化冲突或死锁时最多重试三次。A08 recovery-report
也已接入 PostgreSQL 完整档的一致安全投影；SQLite Lite 对两项
能力明确返回 `501 feature_not_supported`，不伪造空报告。P18/P19 已接入同一竞价事务、稳定
命令账本、代理状态校验和旧版 `{ authorization: ... }` 完整命令投影；quota、完整转换历史和
终止原因随命令结果在同一事务内冻结，HTTP 不再增加提交后账户读取窗口，也不暴露原始账本
payload。A09-A12 与 A38 的
Admin 拍卖生命周期命令已接入应用端口；请求携带 `command_id` 和 `expected_state_version`，取消动作
与提前结算额外要求非空原因，并在同一数据库事务内追加命令账本、状态版本、动作元数据、审计记录和
注册表内的 lifecycle realtime 事件。Admin 人工进入 Final 会把尚晚于当前时间加冻结
`final_phase_lead_seconds` 的 live lot 有效截止时间原子收紧到该窗口；默认配置为 900 秒，已经更早结束的
lot 不会被反向延长，之后的竞价仍可按冻结软截止策略延期。A38 只在 PostgreSQL 完整档且
`allow_manual_early_settlement = true` 时声明 capability；开关默认关闭。命令接受 `live`/`final`，把所有
尚未到期的 live lot 有效截止时间收紧到事务时间并交给既有 settlement scheduler/Worker，HTTP 事务不
直接写成交、义务或履约事实。A34-A37 delivery Admin 列表、状态筛选、数字分页、release、retry 和
reconcile 已接入 PostgreSQL 的安全投影、fencing version 和独立命令账本；reconcile 的 retry、succeeded、failed
决策以及对应的外部引用/失败代码由请求体明确表达；列表投影还提供从版本化 send payload
派生的 COD 数值，以及从结果 metadata 派生的 `reason_code`、`take_required`、`returned_mail_id`，
但不返回 payload 或 metadata 原文；SQLite 明确返回
`501 feature_not_supported`。这些动作只推进本地 outbox/履约状态，不会伪造外部 MoogleMail
回执；Worker 已接入基础通知、结算 send/return transport 以及 Sent/Balance 回执对账，满足
证明会写入履约 proof，无法取得 exact remote ID 则进入人工复核；其余未登记的 Admin 查询/命令
仍返回明确的能力缺失，不会被空结果伪装成已接入。
A14 recovery 确认已接入 PostgreSQL 完整档的版本 fence、证据重校验、命令账本、退款激活、guarded relist、
审计和预览目录 realtime 事件；结算事务已为获胜 lot 在同一事务中建立 recovery case；SQLite Lite
明确返回 `501 feature_not_supported`。Recovery Worker 已按旧版 `case → return → action → relist → refund`
顺序接入持久化调度、回执证明、guarded relist 和买家退款收口；尚缺的是实际 MoogleMail 回执与切换前
行为对照，不能把静态接入误称为外部流程已经验收。
A21-A24 realtime 运维面已接入 PostgreSQL/SQLite 的安全 lag/failure 投影和 replay 事务；A24
使用 `command_id`、CSRF、命令摘要和审计，重试同一命令返回同一 replay 事件。没有数据库端口的
骨架制品明确返回 `501 feature_not_supported`；这组接口仍只处理本地 realtime 失败边界，外部
广播 sink 未冻结时不会伪造远程投递成功。A25 Admin 规则 GET、候选 POST 校验和 PUT 发布已接入
规则 owner；PUT 保留 `command_id`、`expected_revision`、命令摘要、Serializable revision CAS、
幂等账本和审计，SQLite Lite 只提供 GET，发布明确返回 `501 feature_not_supported`。

A05 `/api/v1/admin/auction-forum/preview` 已接入 Admin session、origin/CSRF 边界和 2 MiB
请求体上限；请求必须在 `html` 与 `text` 中恰好选择一个非空输入，由本地论坛解析器返回
catalog、posts 和 warnings。该接口只做内存预览，不抓取远程页面、不发布回帖、不写数据库；
输入无效或解析失败返回 400。远程主题发布/读取、回帖竞价导入和通知由 Worker 的独立论坛 owner
负责，已经接入持久化游标、命令账本和不确定结果停靠，不属于该预览接口的职责。
A33 `/api/v1/admin/audit` 已接入 Admin session 和只读安全投影，保留 actor/action/entity/source/result
等精确筛选、occurred 时间范围、keyset opaque cursor 与 `page` 数字分页；数字页在同一数据库
读取事务内取得 total 与记录，keyset 页不伪造 total。metadata 仅允许 JSON object，SQLite Lite
也从同名追加式审计表读取。
A13 `PATCH /api/v1/admin/lots/{lot_id}` 已接入 Admin session、CSRF、空查询校验和稳定
`command_id`；请求的 `action` 只允许 `withdraw` 与 `set_start_price`，后者必须提供正的
`start_price`，并携带 `expected_state_version`。PostgreSQL 完整档在同一 Serializable 事务
中锁定 world/auction/lot，完成 lot 版本 CAS、命令账本、审计和预览 realtime 事件；withdraw
还会创建 `withdrawn_return` 义务及 pending fulfillment。成功响应为 `200` 的
`{ lot: ... }` 安全投影，SQLite Lite 明确返回 `501 feature_not_supported`。
A02 `/api/v1/admin/dashboard` 已接入 Admin session、严格空查询校验和两套数据库的
`service_heartbeats` 安全读取，health/process section 返回真实心跳聚合；Worker control 通过
配置的 Unix socket 读取 live/ready/status，并将 unavailable/timeout/incompatible/rejected
映射为有限 condition，不暴露 transport 错误正文。mail、realtime、delivery section 已分别
读取全量轮询器、consumer lag/failure water、delivery pending/failed/manual-reconcile 水位及年龄字段；fulfillment、recovery
复用 operations-alert source 的履约/恢复安全水位；credential aggregation 和 alerting
section 也已接入安全投影。旧版所有 dashboard section 已有 owner；owner 未装配或读取失败时返回
`unknown`，不伪造状态。三个进程装配根已有启动和周期心跳路径，SIGINT/SIGTERM 下也会停止周期
任务并发布停止心跳。每轮周期心跳都会重新检查数据库和全部内嵌 migration，读取进程内饱和的
审计写入失败计数，并由 Public/Admin/Worker 各自的 tracing owner 提供安全状态；数据库、schema
或审计完整性异常会令 readiness 为 false，tracing degraded 会令 heartbeat status 降级，但不会
单独改变应用 readiness。启动和停止心跳也携带当时的审计计数与 tracing 安全投影，不再发布
固定占位值。外部广播不属于旧版 dashboard 契约。

A16-A19 Admin 邮件首片已接入 PostgreSQL 完整档：轮询状态和入站审查读取使用邮箱边界、
严格查询校验与安全 DTO，列表读取保持同一读取快照；暂停/恢复使用 Admin session/CSRF、
`command_id` 和 state-version fence，在 Serializable 事务内写入 `mail_poll_state`、
`mail_admin_commands` 与追加审计。SQLite Lite 对这组能力明确返回 `501 feature_not_supported`。
A20 远程轮询触发已返回 `202 accepted`：请求经过 Admin session/CSRF 与单个 `command_id`
校验后，经 Worker control 全局唤醒已装配 poller，并返回 `command_id` 与 `pollers`；手动唤醒
只影响当前 Worker 本轮调度，不表示外部读取已经完成。Worker 后续仍以持久化 lease 执行实际
MoogleMail GET；SQLite Lite 明确返回 `501 feature_not_supported`。

A26/A27 Admin runtime 配置已接入 PostgreSQL 完整档：GET 返回非敏感 TOML 文档的 JSON 投影
和稳定 `sha256:` fingerprint，POST `:validate` 只执行严格候选校验，PUT 使用
`command_id` 与 fingerprint fence 写入 durable pending/applied/unknown 命令账本，并通过
同目录临时文件、权限校验、fsync、rename 和目录同步完成原子替换。文件替换与账本提交
之间出现不确定结果时会优先恢复旧文件，无法证明恢复成功则锁存 `unknown` 并返回
`reconciliation_required`；成功更新明确返回 `restart_required`，不伪造热加载。Admin
运行时文件路径由 Admin/Worker 主 TOML 的 `runtime_config_file` 选择；文档还包含旧版
main/isekai、发件人 allowlist、装备过滤和 14 个通知模板，Worker 重启时只把这些业务字段投影为
MoogleMail 启动快照。transport、凭据、调度、mailbox identity 和全局启用开关仍由 Worker 主配置
拥有；敏感 TOML、Cookie、密钥和数据库连接串不进入该文档。SQLite Lite 不拥有这项完整命令账本/原子更新能力，相关接口返回
`501 feature_not_supported`。Admin 重启后，`allow_manual_early_open` 与
`allow_manual_early_settlement` 会分别决定人工提前开放、人工提前结算 capability 和对应的锁内校验；
两个开关都默认关闭，计划时间已到的正常开放仍然允许，SQLite Lite 不声明人工提前结算 capability。
`[equipment_source]` 会冻结为
装备来源适配器的 host/path allowlist、响应上限、共享请求间隔、全请求超时，以及 Admin 手工录入的
可交易装备开关；后者默认关闭且不放宽来源安全或邮件过滤。固定 HTTPS origin、无重定向、精确 world
路径及流式有界读取始终失败关闭。更新成功返回的 `restart_required` 因而是
真实生效边界，不是热加载提示；业务区块更新要求重启受影响的 Admin 与 Worker。

Admin A28 `/api/v1/admin/users` 已接入 Admin session、CSRF、列表筛选分页、用户安全投影及黑名单
次数/最近历史/创建时间列表字段、创建、改名、游戏
身份修改和 key 撤销/重签；A30 `/api/v1/admin/renames/{rename_id}/revert` 已接入改名记录校验、
state-version fence、命令账本和名称占用冲突处理；A31 黑名单 PUT/DELETE 已接入黑名单当前状态、
追加历史、版本 fence 和审计。三组命令的 key 明文不会进入命令账本、审计或错误响应。A29
`GET /api/v1/admin/renames` 继续提供 PostgreSQL/SQLite 的改名历史安全投影，并额外返回当前
用户版本供 A30 建立 fence；管理端 `/users` 页面已提供列表、创建、改名、游戏身份修改、key
撤销/重签、黑名单和改名回滚操作。

A32 `/api/v1/admin/credentials` 已接入 Admin session、origin/CSRF、`command_id` 和
`expected_state_version`，支持 `mail_cookie`/`forum_cookie` 的安全投影列表、导入、改名、启停
和删除。secret 只在应用加密边界短暂存在，不进入响应、审计、命令结果或页面；该路由要求
PostgreSQL 完整档与独立 `credential_data_key`，SQLite 或未配置密钥时返回
`501 feature_not_supported`。Worker 的 active 凭据读取不属于浏览器 DTO，`.keyenv` 只作为兼容
回退入口。
