# HTTP 路由与响应契约

[文档导航](../README.md) · [路由注册](../../crates/auction-http/src/http/routes/surfaces.rs) · [稳定 DTO](../../crates/auction-contracts/src/contracts) · [浏览器入口](../../crates/auction-http/src/assets/routes.rs)

路由、方法和状态码的冻结清单；请求/响应 DTO 与 handler 必须同步维护。

## 本页导航

- [基线与统计口径](#基线与统计口径)
- [进程健康探针](#进程健康探针)
- [通用响应与错误](#通用响应与错误)
- [Public](#public)
- [Admin](#admin)
- [已实现路径的补充约束](#已实现路径的补充约束)
- [契约变更规则](#契约变更规则)

## 基线与统计口径

本清单是 Auction Rust 的 HTTP 权威契约登记表。路径注册由 `auction-http` 持有，稳定请求、响应与
错误 DTO 由 `auction-contracts` 持有；两者必须与本表同步，不能依赖仓库外的路由清单或实现源码。

本表统计 feature API 的唯一路径，不把进程健康检查、静态 Web 资源和反向代理自身的路径
计入。当前冻结统计是：

- Public：21 个唯一路径；
- Admin：43 个唯一路径。

A38-A43 是独立登记的 Admin 产品能力，已经包含在 43 条统计中。路径数量只证明路由注册闭集，不
代表对应应用端口、存储能力或运行证据已经完成。

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

健康探针不计入 Public/Admin feature API 数量；两个 HTTP surface 都提供：

| 方法 | 路径 | 成功 | 未就绪 | 数据 |
| --- | --- | --- | --- | --- |
| GET/HEAD | /health/live | 200 | — | `{"live": true}` |
| GET/HEAD | /health/ready | 200 | 503 | `{"ready": true/false}` |
| GET/HEAD | /health、/api/v1/health | 200 | — | `HealthResponse`：status、service、version |

live/ready 两条路径均使用统一 `{code,message,data}` 外壳。`/health/live` 不访问下游依赖；`/health/ready`
通过应用层 readiness 端口只读检查当前二进制的必需依赖。Public 配置 realtime relay 时还会检查
relay 新鲜度；数据库未配置、迁移不完整或 relay 停滞时返回 503，不把启动骨架伪装成可接收业务
流量。Admin 继续以数据库连接和当前二进制的全部 SQLx 迁移作为 readiness 边界。


## 通用响应与错误

`/health` 与 `/api/v1/health` 是兼容的进程信息入口，返回 status/service/version，不能替代 readiness。

所有业务 API 使用统一响应外壳：

    {"code": "...", "message": "...", "data": ...}

成功响应的业务 code、JSON 字段和错误 code 以本表及 `auction-contracts` 为准。下表是当前
handler 明确产生的稳定集合；依赖错误中的 504 表示 context deadline，503 表示
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
| 501 | 当前部署档位或端口不提供该能力 |
| 503/504 | 下游依赖不可用或处理超时 |


### 严格 JSON 与方法闭集

JSON 写边界属于 v1 契约：请求必须使用 `application/json` 媒体类型（允许合法参数），
body 受每条路由自己的上限约束，解码只接受一个 JSON 值，且请求 DTO 拒绝未知字段。非 JSON、超限、
语法/未知字段/尾随值分别稳定映射为 415 `unsupported_media_type`、413 `body_too_large` 和
400 `invalid_json`。所有错误外壳固定包含 `data`；无业务错误数据时序列化为 `null`，Rust 可继续
附加不含敏感内容的 `request_id`。

Feature API 的方法是严格闭集。GET 路由不会因为 Axum 默认行为额外接受 HEAD；未显式注册的方法
返回 JSON `405 method_not_allowed`，`Allow` 只列出该路径真实注册的方法并使用冻结排序。健康探针、
metrics 和静态文件是否接受 HEAD 由各自契约单独决定，不从业务路由推导。


### 请求边界

Public/Admin 已恢复统一 HTTP boundary：只接受 1 至 64 字节的 ASCII 字母数字/`-_.`
`X-Request-ID`，缺失时生成并在响应回传，重复或非法值返回 `400 invalid_request_id`；同一值会进入
Public 竞价/代理、Admin 生命周期和管理 mutation 的追加式审计，但不参与命令请求哈希。边界还
提取 W3C trace context，记录请求级访问日志，隔离 panic，设置安全响应头，执行默认 15 秒处理
超时和固定 5 秒 HTTP/1 请求头读取超时。`/api/v1/ws` 不受处理超时限制。客户端地址只在 TCP peer
命中配置的规范 CIDR，或 Public/Admin Unix listener 显式启用可信上游时解析 `X-Forwarded-For`；非法链
整体丢弃。CSP 只允许同源脚本及 WASM 编译；内联样式例外用于 Dioxus 首屏防闪样式和组件 style
属性。登录/变更限流器按该地址工作，最多保留 4096 项并确定性淘汰最久未访问项。


### 静态入口与缓存

未注册的 `/api`、`/health` 保留路径固定返回 `404 not_found` JSON；其他未知请求在 `Accept` 明确
允许 HTML 时返回最小页面 404，否则返回同一 JSON 404。静态文件和批准的浏览器路径只接受
GET/HEAD，其他方法返回带 `Allow: GET, HEAD` 的 JSON 405。Public 15 条（含独立 `/forum` 页面）、
Admin 21 条浏览器路由由两个 Dioxus `Route` 枚举与
`auction-http/src/assets/routes.rs` 同步维护，只有这些路径可以读取 `index.html`；批准路径缺少
构建产物时返回 503。

缓存策略是当前 HTTP 安全边界，并适配 Dioxus 稳定 WASM 文件名：Public 非身份化读取为 `no-cache`；
Public forum/sync-status、events/session/lot 列表与详情/账户/竞价/代理与全部 Admin 业务接口为 `no-store`，错误响应使用相同策略；
浏览器路由入口为 `no-store`，直接静态文件中只有 `assets/` 指纹资源使用一年 `immutable`，
`index.html`、`wasm/*.js`、`*.wasm` 和其他稳定文件名使用 `no-cache`、`Last-Modified` 与
`If-Modified-Since`。未知 404/静态 405/产物缺失 503 均为 `no-store`。

## Public

| 编号 | 方法 | 路径 | 认证/边界 | 成功 | 明确错误状态 |
| --- | --- | --- | --- | --- | --- |
| P01 | GET | /api/v1/worlds | 公开 | 200 | 400, 503/504 |
| P02 | GET | /api/v1/rules | 公开；`lang` 可选 | 200 | 400, 503 |
| P03 | GET | /api/v1/events | 公开读取；由 auction boundary 校验；PostgreSQL/SQLite 从持久化事件表回放 | 200 | 400, 404, 503/504 |
| P04 | GET | /api/v1/ws | Public session；只读服务端推送；连接范围必须明确 | 101 | 400, 403, 404, 429, 503/504 |
| P05 | GET | /api/v1/blacklist | 公开；页码分页 | 200 | 400, 503/504 |
| P06 | POST/GET/DELETE | /api/v1/session | POST 登录；GET/DELETE 会话边界 | 200 | 400, 401, 403, 409, 429, 503/504 |
| P07 | GET | /api/v1/auctions | 公开 | 200 | 400, 503/504 |
| P08 | GET | /api/v1/auctions/current | 公开 | 200 | 400, 503/504 |
| P09 | GET | /api/v1/auctions/{auction_id}/lots | 公开；PostgreSQL 直接返回完整装备 URL；响应 `no-store` | 200 | 400, 404, 503/504 |
| P10 | GET | /api/v1/auctions/{auction_id}/bid-log | 公开 | 200 | 400, 404, 503/504 |
| P11 | GET | /api/v1/lots/{lot_id} | 公开；PostgreSQL 直接返回完整装备 URL；响应 `no-store` | 200 | 400, 404, 503/504 |
| P12 | GET | /api/v1/price-history | 公开；成交事实仅由 PostgreSQL 完整档提供 | 200 | 400, 501, 503/504 |
| P13 | GET | /api/v1/compensation-pool | 公开 | 200 | 400, 501, 503/504 |
| P14 | GET | /api/v1/compensation-pool/{auction_id}/movements | 公开 | 200 | 400, 404, 501, 503/504 |
| P15 | GET | /api/v1/auctions/{auction_id}/forum | 公开 | 200 | 400, 404, 501, 503/504 |
| P16 | GET | /api/v1/me/bids | Public session | 200 | 400, 401, 503/504 |
| P17 | GET | /api/v1/lots/{lot_id}/proxy-authorizations/mine | Public session | 200 | 400, 401, 404, 503/504 |
| P18 | POST | /api/v1/lots/{lot_id}/proxy-authorizations | Public session、origin、限流 | 200 | 400, 401, 403, 409, 413/415, 429, 503/504 |
| P19 | POST | /api/v1/lots/{lot_id}/proxy-authorizations/{authorization_id}/lower | Public session、origin、限流 | 200 | 400, 401, 403, 409, 413/415, 429, 503/504 |
| P20 | POST | /api/v1/lots/{lot_id}/bids | Public session、origin、限流 | 200 | 400, 401, 403, 409, 413/415, 429, 503/504 |
| P21 | GET | /api/v1/forum/sync-status | 公开；不接受 query；响应 `no-store` | 200 | 400, 501, 503/504 |


### P21 论坛同步开关

P21 使用 `PublicForumSyncStatusResponse`，成功响应固定为 `code=ok`、`data={"enabled": true/false}`，
不依赖当前世界或当前拍卖是否存在。布尔值来自 Worker 按 `[worker.forum].enabled` 发布的非敏感心跳
字段，仅采纳数据库时钟下未过期且非 `stopping` 的 Worker：任一明确 true 表示启用，所有有效实例
均明确 false 才表示关闭。没有实例、旧版心跳缺字段或无法证明开关值时返回 `503 forum_sync_unavailable`；
未装配存储或查询失败返回 `503 storage_unavailable`，SQLite Lite 返回 `501 feature_not_supported`。
启用状态不表示主题已经发布或扫描成功；真实楼层仍由 P15 的 `processed_through_post_number` 提供。
Public `/forum` 每 30 秒重读 P21，关闭提示优先于无拍卖/未开拍状态，且关闭时不挂载 P15 轮询；
未知和请求失败显示不可用并允许重试，不解释为关闭。升级须同时更新 Worker、Public API 与公开端产物。


### Public 分页、世界与金额

Public 分页必须保留已发布的 page/page_size 与 cursor 语义。业务世界关闭后，Public 当前目录、
preview/live/final/settling 投影、写入入口和 discovery 增量都必须隐藏该世界；只允许已经
`settled` 的拍卖、lot、竞价/账户历史、价格历史与补偿事实继续通过历史读取边界访问，不能把
`cancelled` 或禁用世界中的活动拍卖顺带公开。P09 的 lot 投影允许读取仍属于公开
安全边界的 settled 拍卖；响应上下文固定包含 `auction_id`、`world`、`number`、状态、展示模式、
开标时间、可选有效结束时间和 `preview_revision`，Public 历史页面据此打开指定轮次而不读取数据库行。
P18-P20 新提交的普通出价、代理创建/降低金额最高为 1B（1,000,000,000 Credits），仍要求 1000
网格；新命令由领域层校验并沿用 `invalid_amount`/`invalid_maximum_amount` 错误，既有 command_id
的成功重放不被重新解释。P09/P11 的 `next_bid` 保留数学最低价和严格抬价关系；超过 1B 时表示
没有可提交的新出价，浏览器显示“已达上限”，不能把值压低到 1B 后提交。普通输入继续使用整数 K；
代理输入扩展为 K/M/B，其中 M/B 最多三/六位小数，均须精确落在 1K 网格且不超过 1B。


### 标的编号、来源与历史读取

所有 lot/竞价/账户/成交投影中的 `lot_number` 都是装备分类内从 1 开始的用户可见编号，必须与
`equipment.category` 组合解释为 `One01`、`Two01` 等编码；它不是全局身份。详情和写命令继续只以
`lot_id` UUID 定位，价格历史的 opaque cursor 继续绑定内部场次顺序，不能由可重复的分类编号重建。
P10 竞价日志额外保留
`world` 与 `lot` 筛选；`lot` 可以是 lot UUID，也可以是 `One01`、`Two01` 一类分类编码，二者在
内部请求中互斥，并在 storage 的全局结果集分页前应用，不能只筛选当前页面。其条目按
`created_at`、`lot_id`、`accepted_state_version`、
`fact_index`、`id` 降序排列，cursor 是绑定这五个字段的版本化 opaque 值。带 cursor 的返回使用
next_cursor/has_more，不得把未知总数伪装成精确 total。条目的可选 `source` 只允许 `forum`/`proxy`：
论坛普通竞价仅在账本保存了真实楼层时同时返回 `source_post_number`；若同一账本还能证明真实
thread/post ID，则增加规范 `source_post_url`。链接身份不得由楼层号或数字论坛账号 ID 推算，玩家
展示继续使用名称。代理事实不区分 fast/slow 且不携带楼层或帖子链接；普通 Web 出价三者都为空。
历史论坛事实楼层未知时只标记论坛来源，不从 post ID 猜测。
P09/P11 的 preview/live 装备投影可选返回按 `Strength, Dexterity, Agility, Endurance,
Intelligence, Wisdom` 固定顺序生成的 `primary_attribute_initials`，缺少对应属性时省略而不是伪造。
P12 在 PostgreSQL 完整档保留 `name`、`world`、
日期、价格、buyer/seller 筛选，编号页附带完整过滤集 `stats`，cursor 页不返回统计；SQLite Lite
因为没有 settlement 事实链固定返回 `501 feature_not_supported`，不能把状态列或空集合当作成交证明。实时事件的 after 也是
非负 stream sequence，不得改成时间戳游标。P03 的 v1 单拍卖读取要求规范 `auction_id`，可带
`after`、`page_size`，默认页大小 100、上限 200；响应除 `events`、`next_after`、`has_more`、
`refetch_required` 外必须返回相同 `auction_id`。发现流是显式 `scope=auctions` 扩展，不能靠省略
`auction_id` 隐式进入。HTTP 外层 envelope 允许非破坏性扩展，但 `data`、事件和 payload 使用精确字段
闭集；缺口/坏投影会隐藏坏事件、置 refetch 并继续扫描有界原始页，`next_after` 指向原始高水位。
P04 查询只能含一个 `auction_id` 或 `scope=auctions`，每帧发送一个不含内部 `stream_key` 的 v1 事件对象；
启用 allowlist 时要求恰好一个可信 `Origin`，独立连接尝试限流之后按 origin→query→session 返回错误。
frame/message 上限均为 4 KiB，Ping/Pong/写入期限为 50/60/5 秒；1008、1013、1002、1001 及 reason
分别表示客户端写入、慢消费者、事件合同错误和服务 drain；`MessageTooLong` 读超限尝试发送空 reason 的
1009 close，其余 Pong 超时及底层读写失败直接断开。
只有 discovery 使用每进程 128 槽和 429 `discovery_busy`；单拍卖新连接拒绝 settled，但既有连接可接收
`auction.settled`。断线补发只走 P03，refetch 保留恢复游标和现有 socket。


### Public 时间词法

所有 Public JSON 时间使用 RFC3339Nano 词法：数据库或当前实现生成的 UTC 时间使用 `Z`，小数秒
裁掉尾随零，整秒不附加 `.000000`。P18-P20 从历史命令账本重放时还必须
保留账本字符串原有的 offset 与纳秒精度，同时以解析后的 UTC instant 做一致性校验。该规则不
改变 PostgreSQL 关系字段的微秒精度，也不改变 Admin 既有的固定六位微秒投影。

## Admin

除 Admin session 本身外，下表路径都经过 Admin session；mutation 还经过 origin、CSRF、
限流和 actor 注入。HTTP boundary 规范化后的 `request_id` 已注入支持审计的管理命令和生命周期
命令；Worker 内部命令保持 `None`，避免把 `command_id` 冒充请求关联。具有多个 HTTP 方法的路径
必须按方法分别保留成功状态。

| 编号 | 方法 | 路径 | 成功 | 明确错误状态 |
| --- | --- | --- | --- | --- |
| A01 | POST/GET/DELETE | /api/v1/admin/session | POST/GET/DELETE 200 | 400, 401, 403, 429, 503/504 |
| A02 | GET | /api/v1/admin/dashboard | 200 | 400, 401, 503/504 |
| A03 | GET/POST | /api/v1/admin/auctions | GET 200；POST 201 | GET: 400, 401, 501, 503/504；POST: 400, 401, 403, 409, 413/415, 501, 503/504 |
| A04 | POST | /api/v1/admin/worlds/{world_id}/auction-intake/items | 201 | 400, 401, 403, 409, 413/415, 503/504 |
| A05 | POST | /api/v1/admin/auction-forum/preview | 200 | 400, 401, 403, 413/415, 503/504 |
| A06 | GET | /api/v1/admin/auctions/{auction_id}/lots | 200 | 400, 401, 404, 501, 503/504 |
| A07 | GET | /api/v1/admin/auctions/{auction_id}/settlement-report | 200 | 400, 401, 404, 501, 503/504 |
| A08 | GET | /api/v1/admin/auctions/{auction_id}/recovery-report | 200 | 400, 401, 404, 501, 503/504 |
| A09 | POST | /api/v1/admin/auctions/{auction_id}/preview | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A10 | POST | /api/v1/admin/auctions/{auction_id}/open | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A11 | POST | /api/v1/admin/auctions/{auction_id}/cancel | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A12 | POST | /api/v1/admin/auctions/{auction_id}/final-phase | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A13 | PATCH | /api/v1/admin/lots/{lot_id} | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
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
| A26 | GET/PUT | /api/v1/admin/config | GET/PUT 200；Lite 501 | 400, 401, 403, 409, 413/415, 501, 503/504 |
| A27 | POST | /api/v1/admin/config:validate | 200；Lite 501 | 400, 401, 403, 413/415, 501, 503/504 |
| A28 | GET/POST/PATCH | /api/v1/admin/users | GET 200；POST 201；PATCH 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A29 | GET | /api/v1/admin/renames | 200 | 400, 401, 503/504 |
| A30 | POST | /api/v1/admin/renames/{rename_id}/revert | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A31 | PUT/DELETE | /api/v1/admin/users/{user_id}/blacklist | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A32 | GET/POST/PATCH/DELETE | /api/v1/admin/credentials | GET 200；POST 201；PATCH/DELETE 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A33 | GET | /api/v1/admin/audit | 200 | 400, 401, 503/504 |
| A34 | GET | /api/v1/admin/deliveries | 200 | 400, 401, 503/504 |
| A35 | POST | /api/v1/admin/deliveries/{delivery_id}/release | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A36 | POST | /api/v1/admin/deliveries/{delivery_id}/retry | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A37 | POST | /api/v1/admin/deliveries/{delivery_id}/reconcile | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A38 | POST | /api/v1/admin/auctions/{auction_id}/early-settlement | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 |
| A39 | PATCH | /api/v1/admin/auctions/{auction_id}/schedule | 200 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 |
| A40 | GET | /api/v1/admin/worlds | 200 | 400, 401, 503/504 |
| A41 | PATCH | /api/v1/admin/worlds/{world_id} | 200 | 400, 401, 403, 404, 409, 413/415, 429, 501, 503/504 |
| A42 | GET | /api/v1/admin/settlement-catalog | 200 | 400, 401, 501, 503/504 |
| A43 | GET | /api/v1/admin/realtime/events | 200 | 400, 401, 500, 501, 503/504 |

## 已实现路径的补充约束

### 公开读取与命令结果

当前 P01、P02、P05、P06、P07、P08、P09、P10、P11、P12、P13、P14、P15、P16、P17、P18、P19、P20、P21、A03 GET/POST、A04、A06、A15、A25 GET/POST/PUT、A40-A43 已接入应用端口；P02 保留 `lang` 语言选择和缺失语言回退英文，P05 保留公开安全投影和页码分页；P07 保留
`page`/`page_size`/`total` 分页和 `cursor`/`next_cursor`/`has_more` keyset 分页，P09
保留 v1 preview/live 互斥 lot 数组、分类统计和页码字段，P11 保留当前公开 projection
下的 lot 详情边界，P10 保留 world/lot 筛选、编号页与 keyset cursor 两种分页形状以及
公开 bid fact 字段，P12 保留成交条目、筛选条件和编号页统计；P13/P14 保留补偿池批次、余额守恒摘要、opaque keyset cursor 和单场账本分页，PostgreSQL 完整档从同一 Serializable 快照读取，SQLite Lite 明确返回 `501 feature_not_supported`；P15 读取本地论坛同步镜像，返回源版本、内容版本、由完整主题扫描证明的真实连续处理楼层、同步时间、最近成功时间及有界安全的最近错误，未把远程网关状态伪造成成功，SQLite Lite 同样明确返回 `501 feature_not_supported`。Public `/forum` 页面在 P21 明确启用后消费 P15：发布成功后只显示“当前同步到 #xx”，`pending`/`publishing` 每 2 秒刷新，`published` 每 30 秒刷新，`failed` 或请求错误后暂停帖子轮询并提供人工重试；首页不再读取或轮询该投影。P09/P11 在 PostgreSQL 完整档按已校验的 lot/equipment/world 批量读取密文信封并短暂解密，把完整 HentaVerse URL 直接写入既有 `game_equipment_url` 字段；10 位 key 因用户主动导航需求允许出现在这两条 JSON 中，但不得进入日志、审计、事件或持久化缓存，响应固定 `no-store`。SQLite Lite 保留 P09/P11 的 200 lot 投影并把该字段留空。P06 的 Public session 已接入
配置、应用端口和 PostgreSQL/SQLite 存储；P16/P17 已接入 Public session、账户读取端口以及
PostgreSQL/SQLite 的普通竞价、代理 quota、转换历史、最近 proxy bid 和 owner capability 投影；P20 的普通竞价已接入 Public session、稳定
`command_id`/`request_hash`、应用竞价事务和 v1 竞价结果投影。
### 管理拍卖与报表

Admin 的 `/api/v1/admin/session` 已接入 Basic Auth、CSRF 和持久化会话端口。A03 GET
`/api/v1/admin/auctions` 已接入 Admin 会话保护、`world_id`/`status` 筛选、有限页码和
PostgreSQL Serializable 生命周期/lot 数量安全投影；创建拍卖 POST 已接入零版本 fence、命令账本、world/intake target 锁、策略快照、审计和 `auction.created` 事件；SQLite Lite 的创建与列表 GET 均明确返回 `501 feature_not_supported`。A06
`/api/v1/admin/auctions/{auction_id}/lots` 已接入 Admin 会话保护、有限页码、PostgreSQL
Serializable 同快照的拍卖/lot/装备/投稿安全投影；响应 body 保持 v1
`auction/lots/pagination` 闭集，内嵌 Web 通过非破坏性的
`X-Auction-Capability-Manual-Intake` 响应头读取外部写入能力。SQLite Lite 因缺少完整 Admin 生命周期字段
明确返回 `501 feature_not_supported`，不填充伪造的默认生命周期数据。A04
`/api/v1/admin/worlds/{world_id}/auction-intake/items` 已接入 URL/canonical JSON 录入、
固定 HTTPS 来源边界、可选的可交易装备限制、key 密文信封和 PostgreSQL 同一 Serializable 写事务；
`equipment_source.require_tradeable` 默认关闭，允许 Admin 手工录入 Soulbound 装备；开启后在事务前
恢复只接受可交易装备的限制，且不改变 MoogleMail 邮件过滤。成功响应只返回拍卖、投稿、lot 与
装备的安全事实，其中 `lot_number` 是对应 `equipment_category` 内独立递增的编号；不返回原始 URL、canonical 正文或 key，SQLite Lite 明确
返回 `501 feature_not_supported`。A15
`/api/v1/admin/lots/{lot_id}/proxy-events` 已接入 Admin 会话保护、页码/opaque cursor 两种分页、
PostgreSQL Serializable 命令账本快照和代理安全事件投影；投影包含创建、降低上限，以及用户被
列入黑名单时由子命令产生的 `frozen` 转换，不把冻结伪装成竞价事实。SQLite Lite 明确返回
`501 feature_not_supported`，管理端 lot 页面已提供历史读取入口。A07
`/api/v1/admin/auctions/{auction_id}/settlement-report` 已接入 PostgreSQL 完整档的一致
只读报告；报告中的 `auction` 保持 v1 基础字段闭集，不额外发送该版本字段闭集之外的
`scheduled_end_at`/`effective_end_at`；lot 明细新增权威 `listed_price`、`hold_ppm` 与带符号
`adjustment_ppm`，正值表示惩罚/税费、负值表示折扣，界面不从税额反推持有比例。读取遇到
序列化冲突或死锁时最多尝试三次（含首次）。A07/A08 的 lot 明细同时返回 `equipment_category`，管理端据此
显示分类编码并允许不同分类各自存在编号 1。A08 recovery-report
也已接入 PostgreSQL 完整档的一致安全投影；SQLite Lite 对两项
能力明确返回 `501 feature_not_supported`，不伪造空报告。A42 只列出 `status=settled` 且有
`settled_at` 的 PostgreSQL 拍卖，接受可选 `world_id`、`sequence` 与有界 `page`/`page_size`。
指定世界时按期号降序稳定排序；全世界时按结算时间、世界、期号和 UUID 稳定排序，count 与当前页
来自同一 Serializable 快照。Admin 结算页以第一页第一项作为权威默认最新一期，再按 UUID 请求
A07/A08；SQLite Lite 对目录与报告都返回 501。P18/P19 已接入同一竞价事务、稳定
命令账本、代理状态校验和 v1 `{ authorization: ... }` 完整命令投影；quota、完整转换历史和
终止原因随命令结果在同一事务内冻结，HTTP 不再增加提交后账户读取窗口，也不暴露原始账本
payload。A09-A12 与 A38 的
Admin 拍卖生命周期命令已接入应用端口；请求携带 `command_id` 和 `expected_state_version`，取消动作
与提前结算额外要求非空原因，并在同一数据库事务内追加命令账本、状态版本、动作元数据、审计记录和
注册表内的 lifecycle realtime 事件。Admin 人工进入 Final 会把尚晚于当前时间加冻结
`final_phase_lead_seconds` 的 live lot 有效截止时间原子收紧到该窗口；默认配置为 900 秒，已经更早结束的
lot 不会被反向延长，之后的竞价仍可按冻结软截止策略延期。A38 只在 PostgreSQL 完整档且
`allow_manual_early_settlement = true` 时声明 capability；开关默认关闭。命令接受 `live`/`final`，把所有
尚未到期的 live lot 有效截止时间收紧到事务时间并交给既有 settlement scheduler/Worker，HTTP 事务不
直接写成交、义务或履约事实。A39 只允许 `draft`/`preview`，请求携带 `command_id`、
`expected_state_version`、`preview_scheduled_at`、`scheduled_open_at` 与 `scheduled_end_at`，并要求
`preview < open < end`。Draft 的三项时间都必须位于数据库当前时间之后；Preview 必须原样保留已经
发生的预览时间，且开拍、结束仍在未来。PostgreSQL 在同一 Serializable 事务内按 world、scheduled
round、auction 加锁，同步拍卖计划列、结束策略快照与持久调度记录；有 owner 且未过期的活动租约
拒绝修改，过期租约或无 owner 的失败退避会在实际变更时清除并递增 round 版本，使陈旧 Worker 的
完成/失败写回失效。命令账本、审计和 `auction.schedule_updated` 事件同事务追加；事件 v1 保持原字段，
只唤醒客户端重读权威快照。已经正式开放、开始结算、结算或取消的拍卖不能修改，SQLite Lite
明确返回 `501 feature_not_supported`。A34-A37 delivery Admin 列表、状态筛选、数字分页、release、retry 和
reconcile 已接入 PostgreSQL 的安全投影、fencing version 和独立命令账本；reconcile 的 retry、succeeded、failed
决策以及对应的外部引用/失败代码由请求体明确表达；列表投影还提供从版本化 send payload
派生的 COD 数值，以及从结果 metadata 派生的 `reason_code`、`take_required`、`returned_mail_id`，
但不返回 payload 或 metadata 原文。`take_required=false` 在响应中省略，客户端缺失时按 `false`
解码，显式 `null` 或非布尔类型仍视为无效响应；SQLite 明确返回
`501 feature_not_supported`。这些动作只推进本地 outbox/履约状态，不会伪造外部 MoogleMail
回执；Worker 已接入基础通知、结算 send/return transport 以及 Sent/Balance 回执对账，满足
证明会写入履约 proof，无法取得 exact remote ID 则进入人工复核；其余未登记的 Admin 查询/命令
仍返回明确的能力缺失，不会被空结果伪装成已接入。
### 投递对账与远端证明

Admin `reconcile` 的 `succeeded` 分支不能用 `payload.operation=send` 加裸 `external_reference`
完成状态；缺少 Sent exact payload proof 的请求由后端按既有 invalid 条件拒绝。Admin Web 隐藏该成功选项，
`retry`/`failed` 仍须由管理员提交显式命令并审计；Return/Take 继续保留独立人工核对路径。该防御不新增
HTTP 字段或错误类别。
Delivery 列表允许一种确定未发送的失败形状：仅 `operations_alert`、
`reason_code=operations_alert_batch_terminal`、错误码 `dependency_mismatch|retry_exhausted`，
attempt/fencing 为零、claimed/sending/lease/外部引用为空且完成时间有效。Storage 与 Admin Web
按同一闭集接受该形状，其他投递终态仍必须提供完整执行轨迹；HTTP 不新增 metadata 原文字段。
主动 MoogleMail ComposeSend 的 Sent baseline、POST 后独立 Sent scan 和 exact payload proof 属于 Worker
delivery 内部契约，不新增 HTTP 路由。所有通知、注册/重绑 key、告警及结算/返还金钱/装备的成功都必须写入
`sent_verified=true`、`external_reference_proven=true`、`proof=sent_exact_payload_v1` 和真实
`mooglemail:world:send:mid`；不满足唯一新增 MID、同 world/recipient/subject/body digest、Credits/COD/装备
精确匹配时只返回/投影 `manual_reconcile`，不能把成功页或本地 UUID 当作证明。Return/Take 的 action proof
仍独立处理。
### Recovery 与实时运维

A14 recovery 确认已接入 PostgreSQL 完整档的版本 fence、证据重校验、命令账本、退款激活、guarded relist、
审计和预览目录 realtime 事件；结算事务已为获胜 lot 在同一事务中建立 recovery case；SQLite Lite
明确返回 `501 feature_not_supported`。Recovery Worker 已按已登记的
`case → return → action → relist → refund` 顺序接入持久化调度、回执证明、guarded relist 和买家退款
收口；尚缺实际 MoogleMail 回执与目标部署运行证据，不能把静态接入误称为外部流程已经验收。
A21-A24 与 A43 realtime 运维面已接入 PostgreSQL/SQLite 的事件元数据、lag/failure 安全投影和 replay 事务；A24
使用 `command_id`、CSRF、命令摘要和审计，重试同一命令返回同一 replay 事件。没有数据库端口的
骨架制品明确返回 `501 feature_not_supported`；这组接口仍只处理本地 realtime 失败边界，外部
广播 sink 未冻结时不会伪造远程投递成功。A25 Admin 规则 GET、候选 POST 校验和 PUT 发布已接入
规则 owner；PUT 保留 `command_id`、`expected_revision`、命令摘要、Serializable revision CAS、
幂等账本和审计，SQLite Lite 只提供 GET，发布明确返回 `501 feature_not_supported`。


### 论坛预览与审计读取

A05 `/api/v1/admin/auction-forum/preview` 已接入 Admin session、origin/CSRF 边界和 2 MiB
请求体上限；请求必须在 `html` 与 `text` 中恰好选择一个非空输入，由本地论坛解析器返回
catalog、posts 和 warnings。该接口只做内存预览，不抓取远程页面、不发布回帖、不写数据库；
输入无效或解析失败返回 400。远程主题发布/读取、回帖竞价导入和通知由 Worker 的独立论坛 owner
负责，已经接入逐主题持久化调度/健康、命令账本和不确定结果停靠；历史世界级回帖 cursor 只保留
历史 schema，不是当前写入 owner。这些能力不属于该预览接口的职责。
A33 `/api/v1/admin/audit` 已接入 Admin session 和只读安全投影，保留 actor/action/entity/source/result
等精确筛选、occurred 时间范围、keyset opaque cursor 与 `page` 数字分页；数字页在同一数据库
读取事务内取得 total 与记录，keyset 页不伪造 total。metadata 仅允许 JSON object，SQLite Lite
也从同名追加式审计表读取。

### 拍品管理与撤拍义务

A13 `PATCH /api/v1/admin/lots/{lot_id}` 已接入 Admin session、CSRF、空查询校验和稳定
`command_id`；请求的 `action` 只允许 `withdraw` 与 `set_start_price`，后者必须提供正的
`start_price`，并携带 `expected_state_version`。PostgreSQL 完整档在同一 Serializable 事务
中锁定 world/auction/lot，完成 lot 版本 CAS、命令账本、审计和预览 realtime 事件；withdraw
还会创建 `withdrawn_return` 义务及 pending/send fulfillment，由 Worker 在事务提交后将该 lot
的原装备在原世界单独寄给原卖家，Credits/COD 均为零。成功响应只确认撤拍和寄回义务入队，
不表示远端已经寄达；投递与领取证明继续由履约流程确认。成功响应为 `200` 的
`{ lot: ... }` 安全投影，SQLite Lite 明确返回 `501 feature_not_supported`。

### Dashboard 与 readiness

A02 `/api/v1/admin/dashboard` 已接入 Admin session、严格空查询校验和两套数据库的
`service_heartbeats` 安全读取，health/process section 返回真实心跳聚合；Worker control 通过
配置的 Unix socket 读取 live/ready/status，并将 unavailable/timeout/incompatible/rejected
映射为有限 condition，不暴露 transport 错误正文。mail、realtime、delivery section 已分别
读取全量轮询器、consumer lag/failure water、delivery pending/failed/manual-reconcile 水位及年龄字段；fulfillment、recovery
复用 operations-alert source 的履约/恢复安全水位；credential aggregation 和 alerting
section 也已接入安全投影。所有已登记 dashboard section 都有 owner；owner 未装配或读取失败时返回
`unknown`，不伪造状态。三个进程装配根已有启动和周期心跳路径，SIGINT/SIGTERM 下也会停止周期
任务并发布停止心跳。每轮周期心跳都会重新检查数据库和全部内嵌 migration，读取进程内饱和的
审计写入失败计数，并由 Public/Admin/Worker 各自的 tracing owner 提供安全状态；Public 同时检查
已配置 relay 的新鲜度，Worker control 同时投影论坛循环的终止故障和可恢复降级。数据库、schema、
审计完整性或必须运行的 relay 异常会令 readiness 为 false，tracing degraded 会令 heartbeat status 降级，但不会
单独改变应用 readiness。启动和停止心跳也携带当时的审计计数与 tracing 安全投影，不再发布
固定占位值。外部广播不属于当前 dashboard 契约。


### 邮件审查与轮询

A16-A19 Admin 邮件首片已接入 PostgreSQL 完整档：轮询状态和入站审查读取使用邮箱边界、
严格查询校验与安全 DTO，列表读取保持同一读取快照；暂停/恢复使用 Admin session/CSRF、
`command_id` 和 state-version fence，在 Serializable 事务内写入 `mail_poll_state`、
`mail_admin_commands` 与追加审计。SQLite Lite 对这组能力明确返回 `501 feature_not_supported`。
列表 A17 只返回 `sender`、`subject` 这两个可空 scalar 与 `has_message_display`，不返回正文；A18
详情返回同样的 `sender`、`subject`，并且 `message_display` 只返回 schema/redaction version、已脱敏正文、附件外部 ID、Credits 和 COD；
被拒绝装备邮件还可返回非权威 `equipment_diagnostic`，包含装备 ID、1-based 附件位置、stage/category、可选 detail_code、
真实可用的 HTTP status、受限耗时、evidence UUID 和 retention status/error。该诊断不进入决定 metadata 或 cursor。
详情还返回有界 `deliveries` 关联邮件任务表，字段为 `id`、`operation`、`recipient`、`status`、`fencing_version`、
`attempt_count`、`failure_code`、`external_reference`；数据只由 `metadata.delivery_ids` 批量查询，并同时匹配
mail_intake 来源、world、external MID、operation 和 origin_id。列表 A17 不增加该字段；不返回 payload、装备 key 或正文。
上限为正文 64 KiB、JSON 128 KiB、附件 1000 条，装备 key 和认证邮件正文隐藏。该展示投影只用于敏感审查，
不参与幂等或 cursor，响应继续使用 `no-store`；历史 NULL 不由普通 replay 补写。
A20 远程轮询触发在至少一个邮箱已装配时返回 `202 accepted`：请求经过 Admin session/CSRF
与单个 `command_id` 校验后，经 Worker control 全局唤醒已装配 poller，并返回 `command_id` 与
`pollers`。空快照时返回 `409 poll_rejected`；手动触发会让已暂停邮箱执行一次领取，但不会修改
`paused` 状态，也不会隐式恢复自动轮询。该响应只表示当前 Worker 接受了本轮调度唤醒，不表示外部读取已经完成；Worker
后续仍以持久化 lease 执行实际 MoogleMail GET。SQLite Lite 明确返回
`501 feature_not_supported`。


### Runtime 配置与文件事务

A26/A27 Admin runtime 配置已接入 PostgreSQL 完整档：GET 返回非敏感 TOML 文档的 JSON 投影
和稳定 `sha256:` fingerprint，POST `:validate` 只执行严格候选校验，PUT 使用
`command_id` 与 fingerprint fence 写入 durable pending/applied/unknown 命令账本，并通过
同目录临时文件、权限校验、fsync、rename 和目录同步完成原子替换。文件替换成功后若账本应用
出现错误，保留新文件并返回 `503 reconciliation_required`，表示结果仍需核对；同一命令重试或
启动恢复按数据库实际 pending/applied 状态向前收口，不能把提交确认丢失误当成可回滚旧文件。
成功更新明确返回 `restart_required`，不伪造热加载。Admin
运行时文件路径由 Admin/Worker 主 TOML 的 `runtime_config_file` 选择；文档还包含兼容
main/isekai、发件人 allowlist、装备过滤和 14 个通知模板，Worker 对 MoogleMail 只把这些业务字段投影为
启动快照。MoogleMail transport、凭据、mailbox identity 和全局启用开关仍由 Worker 主配置
拥有；敏感 TOML、Cookie、密钥和数据库连接串不进入该文档。SQLite Lite 不拥有这项完整命令账本/原子更新能力，相关接口返回
`501 feature_not_supported`。Admin 重启后，`allow_manual_early_open` 与
`allow_manual_early_settlement` 会分别决定人工提前开放、人工提前结算 capability 和对应的锁内校验；
两个开关都默认关闭，计划时间已到的正常开放仍然允许，SQLite Lite 不声明人工提前结算 capability。
`[equipment_source]` 会冻结为
装备来源适配器的 host/path allowlist、响应上限、共享请求间隔、全请求超时，以及 Admin 手工录入的
可交易装备开关；后者默认关闭且不放宽来源安全或邮件过滤。固定 HTTPS origin、无重定向、精确 world
路径及流式有界读取始终失败关闭。更新成功返回的 `restart_required` 因而是
真实生效边界，不是热加载提示；业务区块更新要求重启受影响的 Admin 与 Worker。
Admin WebUI 将这份闭集投影映射成分组开关、数字、列表与文本输入；保存仍提交完整候选文档，
因此所有页面修改最终由 PUT 的原子文件替换边界写回配置文件，不存在只在浏览器内生效的旁路。
更新和启动恢复从首次读取到文件/账本收口持有同一跨进程 workflow 锁；锁竞争返回既有
`503 storage_unavailable`，客户端可复用原 command ID 重试，不能在另一流程处理中绕过锁。


### Runtime 六组接管

runtime 文档提供可选 `operations`，响应投影会补齐六组候选参数：`economics`（后续新拍卖的
底价、增幅、软截止、取消与代理策略）、`schedule`（自动周排期）、`forum`（论坛开关与有界轮询）、
`alerts`（告警阈值、通知目标与批次边界）、`worker`（生命周期、投递、履约、恢复循环的吞吐与重试）、
`mail`（邮件 cron、读取批量/分页、HTTP 与批次处理预算、租约续期、读取/处理尝试和退避）。
每组的 `managed` 默认 `false`，表示沿用公共/角色 TOML；只有设为 `true` 才在对应进程重启时覆盖
该组字段。首次 GET 的候选值不代表未接入 runtime 的 Worker 当前配置。严格候选校验覆盖金额网格及
1B 底价上限、周时间顺序、分页请求预算、并发/批量、租约/超时、告警冷却与收件人。邮件 cron 校验
复用 Worker 配置解析器，并要求存在未来触发时刻；无效候选在新命令写入账本前拒绝。`mail` 完整字段、
单位、默认值和范围见 `ops/config/examples/runtime/admin.toml`，读取与处理超时必须大于 HTTP 超时，
续租周期必须小于租约，退避上限不能小于基础退避。
`mail.batch_timeout_seconds` 范围为 1–86400 秒，至少为 `fetch_timeout_seconds + item_timeout_seconds`，
省略时为 600 秒；该默认值省略序列化以保留旧 fingerprint，Admin 闭集 decoder 和表单接受缺省值。
此预算覆盖一轮拉取及业务处理，可在重试耗尽前结束；完成前缀与其 cursor 由 Worker 原子提交。
缺少整个 `operations` 或其中 `mail` 的旧物理文件和 durable 候选继续保持原指纹；显式 `mail:null`
被拒绝。物化只发生在浏览器投影，
不改写历史命令。API envelope、命令摘要版本和 `restart_required` 语义不变，不增加热加载路径。
保存中断后，pending 命令与旧文件指纹一致时继续完成同一候选，不永久占用唯一未完成命令槽。
若 Admin 启动恢复补写了文件，启动校验会要求重新加载恢复后的配置；未证明的冲突仍需人工核对。


### 数据库业务世界

A40/A41 将数据库 `worlds.enabled` 与 Admin runtime 中只控制 Worker/MoogleMail 入站的 world
开关明确分离。A40 是无 query 的认证只读目录；A41 使用严格 JSON、`command_id` 与
`expected_policy_revision`，在 PostgreSQL Serializable 事务中先锁 world，再拒绝任何
`draft`/`preview`/`live`/`final`/`settling` 拍卖或仍可领取的活动 `scheduled_rounds`，随后以
revision CAS 更新状态，并把通用命令账本和追加审计与状态同事务提交。相同状态是可重放的有审计
no-op，不递增 revision；SQLite Lite 只提供目录读取，启停明确返回 `501 feature_not_supported`。


### 用户、改名与黑名单

Admin A28 `/api/v1/admin/users` 已接入 Admin session、CSRF、列表筛选分页、用户安全投影及黑名单
次数/最近历史/创建时间列表字段、创建、改名、稳定论坛账号 ID
修改和 key 撤销/重签；名称是可变的 MM 收件人与页面显示值，`forum_account_id` 是唯一的规范十进制
`showuser` 账号 ID。A30 `/api/v1/admin/renames/{rename_id}/revert` 已接入改名记录校验、
state-version fence、命令账本和名称占用冲突处理；A31 黑名单 PUT/DELETE 已接入黑名单当前状态、
追加历史、版本 fence 和审计。三组命令的 key 明文不会进入命令账本、审计或错误响应。A29
`GET /api/v1/admin/renames` 继续提供 PostgreSQL/SQLite 的改名历史安全投影，并额外返回当前
用户版本供 A30 建立 fence；管理端 `/users` 页面已提供列表、创建、改名、论坛账号 ID 修改、key
撤销/重签、黑名单和改名回滚操作。


### 外部凭据

A32 `/api/v1/admin/credentials` 已接入 Admin session、origin/CSRF、`command_id` 和
`expected_state_version`，支持 `mail_cookie`/`forum_cookie` 的安全投影列表、导入、改名、启停
和删除。secret 只在应用加密边界短暂存在，不进入响应、审计、命令结果或页面；该路由要求
PostgreSQL 完整档与独立 `credential_data_key`，SQLite 或未配置密钥时返回
`501 feature_not_supported`。Worker 的 active 凭据读取不属于浏览器 DTO，`.keyenv` 只作为兼容
回退入口。`mail_cookie` 与 `forum_cookie` 各自全局最多一条 active 记录；导入或重新启用同类第二条
时返回 `409 credential_conflict`，调用方必须先通过同一路由停用旧记录并重启 Worker 完成交接。

## 契约变更规则

1. Rust 路由必须先在本表登记，再实现 handler；已发布路径不得为了方便直接重命名。
2. DTO 必须在 auction-contracts 中独立定义；不得把 SQL 行或 Rust storage model 直接暴露给浏览器。
3. 所有改变状态的核心命令必须保留 command_id；同一 ID 搭配不同 request hash 必须返回 409。
   A24 v1 兼容请求没有该字段，由 HTTP 防腐层按 failure 与修正负载稳定派生，不得使用随机 ID。
4. 公开实时更新不以 HTTP 分页替代 WebSocket；无法保持长连接时按 events catch-up 语义恢复。
   SSE 尚无登记路由，不能将该扩展方向描述为现有客户端通道。
5. 本表的状态码、JSON 字段和错误 code 变化必须同时更新本文件、相关契约记录和
   对应 owner 的边界说明。
