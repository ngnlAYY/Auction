# HTTP 路由与响应契约

[文档导航](../README.md) · [路由注册](../../crates/auction-http/src/http/routes/surfaces.rs) · [稳定 DTO](../../crates/auction-contracts/src/contracts) · [浏览器入口](../../crates/auction-http/src/assets/routes.rs)

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

`auction-http` 拥有路径注册，`auction-contracts` 拥有稳定 DTO。当前 feature API 为
Public **21** 个、Admin **44** 个唯一路径；多方法共用路径只计一次，不计健康、静态资源或代理路径。
清单描述注册与契约，实际档位和运行证据见[验证状态](../status.md)。

首次部署另有独立进程的 `/setup`、部署码验证接口和两个配置接口，不属于 Public/Admin feature API 计数；
正式 Admin 进程不注册它们，`/setup` 在正式静态入口中返回 404。

进程级可观测性路径不计入上述 feature API 统计：Public 与 Admin 在主配置
`[metrics].enabled = true` 时提供 `GET/HEAD /metrics` Prometheus text scrape；Admin 若配置
`runtime_config_file`，以 runtime 文件的 `[metrics].enabled` 为准。指标关闭、
带 query 或路径变体均返回 404。Worker 不开启 TCP HTTP 监听；启用 Worker control 时，
同一经过 peer credential 校验的 Unix socket 提供 `GET/HEAD /metrics`，使用同一进程 registry。
control 未启用或指标关闭时该路径返回 404。

OTLP/HTTP tracing exporter 不是业务路由。Public/Worker 读主配置 `[tracing]`，Admin 配置 runtime
时读其 `[tracing]`；默认关闭，启用后使用有界异步队列。OTLP headers 只在敏感 TOML 中存在。
SIGINT/SIGTERM 先触发 10 秒业务/HTTP drain，再按配置刷新 exporter；第二次信号强制收口。

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

业务 code、字段与错误 code 由本表及 DTO 共同冻结。504 表示处理期限耗尽，503 表示其他依赖不可用；
未知路径/方法返回 404/405，SPA 仅允许显式浏览器路由。

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
400 `invalid_json`；A24 replay 保留兼容错误码 `invalid_replay`。所有业务错误固定含 `data`，
无业务数据时为 `null`，可附加安全 `request_id`。初始化 API 使用独立错误 DTO，见下文。

Feature API 的方法是严格闭集。GET 路由不会因为 Axum 默认行为额外接受 HEAD；未显式注册的方法
返回 JSON `405 method_not_allowed`，`Allow` 只列出该路径真实注册的方法并使用冻结排序。健康探针、
metrics 和静态文件是否接受 HEAD 由各自契约单独决定，不从业务路由推导。


### 请求边界

Public/Admin 统一 HTTP boundary 只接受 1 至 64 字节的 ASCII 字母数字/`-_.`
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

缓存策略适配 Dioxus 稳定 WASM 文件名：Public 普通公开读取（含 P15 forum）为 `no-cache`；
P21 `/forum/sync-status`、events/session/lot 列表与详情/账户/竞价/代理及全部 Admin 业务接口为 `no-store`，错误响应沿用相同策略；
浏览器路由入口为 `no-store`，直接静态文件中只有 `assets/` 指纹资源使用一年 `immutable`，
`index.html`、`wasm/*.js`、`*.wasm` 和其他稳定文件名使用 `no-cache`、`Last-Modified` 与
`If-Modified-Since`。未知 404/静态 405/产物缺失 503 均为 `no-store`。

## Public

| 编号 | 方法 | 路径 | 认证/边界 | 成功 | 明确错误状态 |
| --- | --- | --- | --- | --- | --- |
| P01 | GET | /api/v1/worlds | 公开 | 200 | 400, 503/504 |
| P02 | GET | /api/v1/rules | 公开；`lang` 可选 | 200 | 400, 503 |
| P03 | GET | /api/v1/events | 公开读取；由 auction boundary 校验；PostgreSQL/SQLite 从持久化事件表回放 | 200 | 400, 404, 503/504 |
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

Public 分页必须保留已发布的 page/page_size 与 cursor 语义。P07 拍卖编号页的 `total` 表示完整
筛选集总数，与当前页来自同一读取快照；越界空页不改为 0。cursor 页不执行或返回总数统计。
业务世界关闭后，Public 当前目录、
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

P16/P17 的活动代理能力投影不把“上限不足完整下一口”当作损坏：合法慢速等待仍返回
`waiting=true`，上限不高于降额绑定下限时返回 `can_lower=false`。例如默认 5% 加价、当前价
99,000、慢速上限 100,000 时允许等待并反超，不因下一口 104,000 返回 503；P19 的降额约束不放宽。


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
因为没有 settlement 事实链固定返回 `501 feature_not_supported`，不能把状态列或空集合当作成交证明。

P03/P04 的查询、事件 JSON、`after` 游标、缺口高水位、会话复核、配额与关闭 code/reason 统一由
[事件契约](events.md)登记。P03 是持久化 Catch-up，P04 是服务端单事件推送；发现流必须显式指定
`scope=auctions`。不得把 sequence 改成时间戳，或用 HTTP 分页替代长连接通知。


### Public 时间词法

所有 Public JSON 时间使用 RFC3339Nano 词法：数据库或当前实现生成的 UTC 时间使用 `Z`，小数秒
裁掉尾随零，整秒不附加 `.000000`。P18-P20 从历史命令账本重放时还必须
保留账本字符串原有的 offset 与纳秒精度，同时以解析后的 UTC instant 做一致性校验。该规则不
改变 PostgreSQL 关系字段的微秒精度，也不改变 Admin 既有的固定六位微秒投影。
SQLite 的当前拍卖、实时预检、lot 列表和详情按时间值而非文本词法比较开拍计划，兼容空格格式和
RFC3339。该比较只决定 `preview`/`opening` 展示，不赋予竞价权限，不改变生命周期写入。

## Admin

除 Admin session 本身外，下表路径都经过 Admin session；mutation 还经过 origin、CSRF、
限流和 actor 注入。HTTP boundary 规范化后的 `request_id` 已注入支持审计的管理命令和生命周期
命令；Worker 内部命令保持 `None`，避免把 `command_id` 冒充请求关联。具有多个 HTTP 方法的路径
必须按方法分别保留成功状态。下表还适用通用请求/方法边界错误，所有 mutation 受 `429 rate_limited` 约束。

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
| A26 | GET/PUT | /api/v1/admin/config | GET/PUT 200；Lite 501 | 400, 401, 403, 409, 413/415, 501, 503/504 |
| A27 | POST | /api/v1/admin/config:validate | 200；Lite 501 | 400, 401, 403, 413/415, 501, 503/504 |
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

## 首次部署初始化

PostgreSQL + `web-embed` 的 `auction-admin-api`/`auction-server` 初始化模式默认监听
`127.0.0.1:8091`，正式 Admin 不挂载这些入口。分体 Admin 仅在默认配置不存在且未设置
`AUCTION_CONFIG_FILE` 时无参数进入；已有目录、悬空链接仍算已有配置，元数据错误、显式配置缺失或
解析失败均阻止启动。合并入口按 `--config-dir`（默认 `auction-config`）判断，拒绝已设置的
`AUCTION_CONFIG_FILE`。分体保存后退出；合并入口只有明确确认并关闭设置服务后才初始化专用空库、
启动角色。普通启动不隐式迁移。

静态资源可匿名读取，三个 API 每次都须携带终端输出的 64 字符十六进制 `x-setup-token`。
正文上限 8 KiB，DTO 拒绝未知字段，令牌/非阻塞操作门禁先于正文。HTTP/1 请求头预算 5 秒，
正文与异步处理合计 10 秒，超时为 504 `request_timeout`。停止接收后最多排空 10 秒，超时或再次信号
取消连接；合并入口只接受无终止信号的确定 Completed，无法确认终态时失败关闭。

合并入口通过 `--allow-remote-setup`、`--setup-origin` 和 `--setup-proxy-ip` 明确允许远程 HTTPS 代理。
非 loopback 监听无此授权失败。HTTP 门禁检查 TCP peer、精确 Host、HTTPS 转发头与写请求 Origin；
本机模式也检查 loopback peer 与 Host/Origin，失败返回 403 `setup_origin`。所有应用响应（含拒绝、
框架错误和超时）统一设置 `Cache-Control: no-store`、`Referrer-Policy: no-referrer`，
复用正式表面的 CSP（含 `frame-ancestors 'none'`）、`X-Frame-Options: DENY`、`nosniff` 与
浏览器能力限制；保持既有 Dioxus WASM/解释器和内联样式所需策略，不放宽 Origin 或令牌门禁。
成功响应新增布尔 `initialize_and_start`，默认分体为 false、合并为 true；旧客户端可忽略新增字段。
配置 DTO 新增可省略的 `confirm_initialize`（默认 false）；合并 `complete` 要求 true，否则返回
400 `setup_confirmation_required`。网页只在用户主动勾选专用空库及启动授权后发送 true；
`completed` 只表示配置已落盘，绝不声称数据库初始化或三个角色已就绪。

`verify` 无请求体，只校验令牌和完成状态，不连接数据库、写文件、消费令牌或建立免鉴权会话。
三个 API 鉴权后共用操作锁，覆盖校验、数据库等待与目录发布；竞争为 409 `setup_busy`，完成后为
409 `setup_completed`。目录已发布但持久化无法确认时进入终态 500 `setup_outcome_unknown`，不再读库/写配置，
须停止进程并核对目录；输入、连接及发布前明确失败仍可重试。

页面先验证部署码，返回 `verified` 才展示配置。部署码/验证状态仅保存在内存；刷新重新验证，
`setup_auth` 清空部署码及密码并退回第一步，完成后清空并隐藏表单。密码确认仅在浏览器比较。
数字按原始输入校验，错误用五语言定位摘要展示并展开进阶字段；修改输入清除旧成功提示，
浏览器校验不替代配置 owner，`validated` 不表示落盘。

每个浏览器请求独立限时 10 秒且不自动重试；保存超时、断连或不可识别响应按 `setup_outcome_unknown`
处理。该码使页面清空秘密、锁定输入及全部提交回调，保留可翻译提示，不提供解锁/重试；操作者必须先
停止进程并核对目录，不能刷新重试。客户端中止请求不能证明服务端未写入，也不新增服务端状态码。

`validate`/`complete` 使用同一 `SetupRequest`：必填 `database`、`admin_actor`、`admin_secret`、
`public_origin`、`admin_origin`、`metrics_enabled`、`schedule_enabled`、`start_price`、`log_filter`、
`worker_concurrency`、`worker_poll_interval_seconds`、`realtime_poll_interval_millis`。
`database` 的必填字段为 `host`、`port`（1–65535 整数）、`username`、`password`、`name`。
账号/密码/库名按原文提交，不做 URL 编码；不接受 `database_url`。连接检查与落盘复用配置模型，
保存为敏感 TOML `[database]`；其余可省略字段及默认值如下。

`public_listener_port`/`admin_listener_port` 默认 8090/8091，均为不同的 1–65535 整数，非法为
400 `invalid_input`。只生成正式服务 `127.0.0.1:<port>`，Admin 角色与 runtime listener 同步；
不改变设置监听器、Origin、代理或 Cookie，不探测占用/权限，操作者须同步代理或隧道；runtime 文件为 `runtime/admin.toml`。
页面独立展示端口、按数值检查冲突。“恢复进阶默认值”只重置日志、Worker 并发/轮询、实时轮询并清旧提示，
保留凭据、Origin、端口及基础设置，不发请求/写配置，处理中或结果未知时禁用。

配置请求还接受可省略的 `schedule` 对象，启用状态仍由原必填布尔 `schedule_enabled` 决定。
整个对象或内部单项省略时使用下表默认值；显式空串、`null`、错误类型或未知字段失败关闭，不自动纠正。

| `schedule` 字段 | 默认值 | 首次设置约束 |
| --- | --- | --- |
| `timezone` | `UTC` | 有效 IANA 时区，不超过 128 个 UTF-8 字节 |
| `preview_day` / `preview_time` | `Thursday` / `22:00` | 预览星期与当地分钟时间 |
| `open_day` / `open_time` | `Friday` / `00:00` | 开拍星期与当地分钟时间 |
| `nominal_end_day` / `nominal_end_time` | `Saturday` / `12:00` | 名义结束星期与当地分钟时间 |

星期只接受 `Monday` 至 `Sunday` 的完整英文名称，时间必须为精确 24 小时制 `HH:MM`。
三个阶段在同一个周一至周日周期内按当地分钟排序，满足 `preview + 120 <= open < nominal_end`；
不自动跨周或使用浏览器时区。服务端在连接数据库前复用配置 owner 的排期校验，非法输入返回
400 `invalid_input`，即使 `schedule_enabled=false` 也必须保存合法候选；关闭不创建拍卖。
完整计划保存到 `common.toml` 的 `[auction.schedule]`，世界固定为 `main`、`isekai`，表单不接受
`worlds` 或 `managed`。生成的 runtime 明确保持 `operations.schedule.managed=false`，不覆盖公共计划；
后续仅影响 Worker 新建的周拍卖，不改已有拍卖、任务游标或租约。页面星期使用五语言下拉框但发送英文值，
时间使用分钟控件；格式/顺序错误可定位，修改排期或启用开关清除旧提示，恢复进阶默认值不影响排期。
浏览器仅做时区文本边界检查，IANA 有效性与实际周计划仍由服务端判定。

| 方法 | 路径 | 成功 | 明确失败 |
| --- | --- | --- | --- |
| GET/HEAD | `/setup` | 200，Admin WASM 初始化页 | 403 `setup_origin`，404/503（无资源） |
| POST | `/api/v1/setup/verify` | 200 `{"status":"verified","initialize_and_start":false}` | 403 `setup_auth`，409 `setup_busy`/`setup_completed`，500 `setup_outcome_unknown` |
| POST | `/api/v1/setup/validate` | 200 `{"status":"validated","initialize_and_start":false}` | 同 verify，另有 400 `invalid_input`/`configuration_invalid`，503 `database_unavailable` |
| POST | `/api/v1/setup/complete` | 200 `{"status":"completed","initialize_and_start":false}`；写出完整配置并关闭设置服务 | 同 validate，另有 409 `setup_locked`，500 `setup_io` |

数据库凭据、管理员密码只在请求与目标敏感 TOML 中存在；响应只返回稳定错误码，不返回输入、
SQLx 错误或 TOML 正文。数据库检查只确认连接，不建库、不迁移；保存后目标配置目录存在即禁止
再次首次设置。表中成功 JSON 以分体模式为例，合并模式布尔值为 true；所有接口可返回
403 `setup_origin` 或 504 `request_timeout`，合并 complete 还可返回 400 `setup_confirmation_required`。
保存的超时响应也由浏览器按 `setup_outcome_unknown` 处理，不新增自动重试或目录覆盖入口。
完整运行参数仍在正式 Admin `A26/A27` runtime 页面管理。

## 已实现路径的补充约束

### 公开读取与命令结果

P02 接受可选 `lang`，缺失语言回退英文；P05 为安全黑名单页码投影。P07 的数字/keyset 分页、
P10 world/lot 筛选及 P12 成交统计遵守上文分页契约。P09 保留 v1 preview/live 互斥数组、分类统计、
页码及拍卖上下文；P11 只返回当前允许公开的 lot。P13/P14 返回补偿池批次、余额守恒摘要、opaque
keyset cursor 与单场账本页，PostgreSQL 从同一 Serializable 快照读取，Lite 返回 501。

P09/P11 在 PostgreSQL 按 lot/equipment/world 校验密文后短暂解密，直接在既有 `game_equipment_url`
返回固定 HentaVerse origin 的完整 URL；10 位 key 可供用户主动导航，但不得写日志、审计、事件或持久化缓存，
响应 `no-store`、全局 `no-referrer`。Lite 保留 200 投影并留空 URL，不伪造 keyless 链接。

P15 读取本地论坛镜像的源/内容版本、真实连续楼层、同步/最近成功时间及有界安全错误，Lite 返回 501。
`/forum` 仅在 P21 明确启用后轮询 P15：`pending`/`publishing` 每次完成后等 2 秒，`published` 等 30 秒，
发布后显示“当前同步到 #xx”；失败暂停并提供人工重试。首页不轮询，不把本地镜像当作远端成功证明。

P06 会话、P16/P17 账户/代理读取和 P18–P20 竞价命令均有两后端适配。账户投影包含普通竞价、quota、
转换历史、最近 proxy bid 与 owner capability。P20 使用稳定 `command_id`/`request_hash`；P18/P19
返回完整 `{ authorization: ... }`，quota、转换历史和终止原因与命令结果同事务冻结，不在提交后另读账户，
也不暴露账本 payload。未知结果必须重放原命令，详见[幂等不变量](invariants.md#i-01-命令账本是权威幂等边界)。

### 管理拍卖与报表

A01 使用 Basic Auth、CSRF 和持久化会话。Admin 每次装配数据库会话服务先永久撤销全部旧会话，失败则
阻止启动；重启须重新登录。随机 token 摘要用 HMAC-SHA256 绑定当前 actor/secret，读取再核对 actor，
不额外存凭据摘要。换凭据或恢复旧配置都不能复活撤销 token，两后端复用原表/撤销标记，Public 不受影响。
同库多 Admin 必须使用相同凭据；任一实例启动会撤销既有会话，轮换须先停全部旧凭据实例。
浏览器每次成功确认后重新安排检查，即使绝对到期使 `expires_at` 不变；业务 401 触发确认，网络错误不当作退出。

除下文另述外，下列拍卖管理/报表为 PostgreSQL 完整档，Lite 返回 `501 feature_not_supported`：

| 接口 | 读取或写入契约 |
| --- | --- |
| 拍卖列表（A03 GET） | `world_id`/`status` 筛选、有界页码；生命周期与 lot 数量来自同一 Serializable 快照 |
| 创建（A03 POST） | 零版本 fence；同事务锁 world/intake target，写策略快照、命令账本、审计与 `auction.created` |
| 录入（A04） | URL/canonical JSON 录入，固定 HTTPS 来源、key 密文及 Serializable 原子写入；只返回拍卖/投稿/lot/装备安全事实和分类内 `lot_number`/`equipment_category` |
| 拍品列表（A06） | 同快照读取拍卖/lot/装备/投稿，body 固定 `auction/lots/pagination`；外部录入能力通过 `X-Auction-Capability-Manual-Intake` 头提供 |
| 代理历史（A15） | 页码/opaque cursor 代理历史，包含创建、降额与黑名单子命令的 `frozen` 转换；冻结不伪装成 bid |
| 报告（A07/A08） | 一致结算/恢复安全报告，lot 带 `equipment_category`；A07 拍卖字段闭集不含 `scheduled_end_at`/`effective_end_at`，lot 提供权威 `listed_price`、`hold_ppm`、带符号 `adjustment_ppm` |
| 结算目录（A42） | 仅列 `settled` 且有 `settled_at` 的拍卖；可选 `world_id`/`sequence` 与有界 `page/page_size`，count 与页同 Serializable 快照 |

A04 的 `equipment_source.require_tradeable` 默认关闭，允许手工 Soulbound；开启后在事务前限制可交易装备，
不改变 MoogleMail 过滤，也不回传来源 URL/canonical 正文/key。A07 的 `adjustment_ppm` 正为税费/惩罚，
负为折扣，页面不从税额反推持有比例；报告序列化冲突/死锁最多尝试 3 次（含首次）。A42 指定世界按期号
降序，否则按结算时间、世界、期号、UUID 稳定排序；结算页用第一页首项作为默认最新一期，再按 UUID 读报告。

A09–A12、A38 生命周期命令携带 `command_id`、`expected_state_version`；取消/提前结算另需非空理由。
原因最多 1024 字节且不含控制字符，其他动作不得携带 reason；状态、账本、动作 metadata、审计与事件同事务写入。人工 Final 将晚于事务时间加冻结
`final_phase_lead_seconds` 的 live lot 截止收紧至该窗口（默认 900 秒），不延长更早截止，之后仍允许软截止延长。
A38 仅在完整档且 `allow_manual_early_settlement=true` 时提供（默认关闭），接受 `live`/`final`，
将未到期 live lot 截止收紧到事务时间；成交、义务与履约仍由提交后的 settlement Worker 写入。
A38 未开启或缺少该能力返回 `403 manual_early_settlement_disabled`，不套用一般能力缺失的 501。

A39 只允许 `draft`/`preview`，请求含 `command_id`、`expected_state_version`、`preview_scheduled_at`、
`scheduled_open_at`、`scheduled_end_at`，满足 `preview < open < end`。Draft 三者须在数据库当前时间之后；
Preview 原样保留已发生的预览时间，其余在未来。完整档 Serializable 事务按 world、scheduled round、auction
加锁，同步计划列、结束策略快照、调度记录、账本、审计与 `auction.schedule_updated`。未过期且有 owner 的
租约拒绝修改；实际变更时清除过期租约或无 owner 的失败退避并递增 round 版本，阻断陈旧 Worker 写回。
事件 v1 字段不变，只唤醒权威快照重读；其他状态及 Lite 不提供修改。

A34–A37 为完整档 delivery 安全列表、状态筛选、数字页及 release/retry/reconcile，携带 fencing version 和
独立账本。reconcile 请求明确 `retry`/`succeeded`/`failed` 与对应外部引用/失败码；实际允许的动作受下节证明门禁约束。
列表从 send payload 投影 COD，从 outcome metadata 投影 `reason_code`、`take_required`、`returned_mail_id`，
不返回原文。`take_required=false` 省略，客户端缺失按 false，显式 null/非布尔仍无效。命令只推进本地状态，
不伪造远端回执；基础通知、结算 Send、入站 Return/Take 与 Sent/Balance 核对由 Worker 执行，缺 exact remote ID
进入人工复核。Lite 返回 501，未装配 owner 明确报告能力缺失。

### 投递对账与远端证明

Admin `reconcile` 的 `succeeded` 分支不能用 `payload.operation=send` 加裸 `external_reference`
完成状态；缺少 Sent exact payload proof 的请求由后端按既有 invalid 条件拒绝。Admin Web 隐藏该成功选项，
`retry`/`failed` 仍须由管理员提交显式命令并审计；Return/Take 继续保留独立人工核对路径。该防御不新增
HTTP 字段或错误类别。
Delivery 列表允许告警确定未发送的失败形状：仅 `operations_alert`、
`reason_code=operations_alert_batch_terminal`、错误码 `dependency_mismatch|retry_exhausted`，
attempt/fencing 为零、claimed/sending/lease/外部引用为空且完成时间有效。Storage 与 Admin Web
按同一闭集接受该形状，其他投递终态仍必须提供完整执行轨迹；HTTP 不新增 metadata 原文字段。
另一个具名例外是 `mail_intake` 的 `reason_code=mail_disposition_conflict`：历史待发送资产冲突可在没有
`sending_at` 的情况下进入 `manual_reconcile`，保留原 payload/命令和隔离原因。只允许人工确认 `failed`，
禁止确认成功、重试和失败后重新激活。收件详情的关联 delivery 同步返回可省略的 `reason_code`，
浏览器据此保留同一门禁；不暴露原始 metadata。
主动 ComposeSend 的成功 metadata 固定包含 `sent_verified=true`、`external_reference_proven=true`、
`proof=sent_exact_payload_v1` 与真实引用 `mooglemail:world:send:mid`；Return/Take 使用独立 action proof。
完整 baseline/scan、精确匹配和结果未知边界见[Sent 证明](../operations/mail-diagnostics.md#主动-composesend-的-sent-证明)，不新增 HTTP 路由。

### Recovery 与实时运维

A14 为 PostgreSQL 完整档的版本 fence、证据复核、账本、退款激活、guarded relist、审计及预览目录事件事务。
结算为获胜 lot 同事务建立 recovery case，Worker 按 `case → return → action → relist → refund` 持久化处理。
已付款退货的退款确认要求 handoff 义务为 `ready` 且未满足，仍须完整取回证据；不能以付款前 `blocked` 状态替代。
Delivery 人工 retry/reconcile-retry 在原命令事务恢复匹配的 Recovery action 与父任务阻断，不改变外部成功证明门禁。
Lite 返回 501；真实 MoogleMail 回执与部署运行证据见[验证状态](../status.md)，静态接入不等于外部流程验收。

A21–A24、A43 在两后端提供事件元数据、lag/failure 与 replay 安全边界，未装配端口返回 501。
A24 命令重放、兼容 ID 派生与追加事件/审计见[事件契约](events.md#worker-消费游标与重放边界)，不代表外部广播成功。
A25 的 GET、POST 候选校验和 PUT 发布由规则 owner 拥有；PUT 以 `command_id`、`expected_revision`、
请求摘要在 Serializable 事务做 revision CAS、账本与审计。Lite 只提供 GET，发布返回 501。

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
列表 A17 的展示字段仅增加可空 `sender`/`subject` 与 `has_message_display`，不返回正文；A18
详情返回同样的 `sender`、`subject`，并且 `message_display` 只返回 schema/redaction version、已脱敏正文、附件外部 ID、Credits 和 COD；
被拒绝装备邮件还可返回非权威 `equipment_diagnostic`，包含装备 ID、1-based 附件位置、stage/category、可选 detail_code、
真实可用的 HTTP status、受限耗时、evidence UUID 和 retention status/error。该诊断不进入决定 metadata 或 cursor。
详情还返回有界 `deliveries` 关联邮件任务表，字段为 `id`、`operation`、`recipient`、`status`、`fencing_version`、
`attempt_count`、`failure_code`、可省略的 `reason_code`、`external_reference`；数据只由 `metadata.delivery_ids` 批量查询，并同时匹配
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
启动恢复任何失败（包括结果未知或需人工核对）都阻断 Admin 监听；仅恢复成功且文件指纹匹配启动快照才继续装配。
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
回滚首次按精确 `user_id` 读取版本并冻结包含理由、版本与 `command_id` 的完整请求；
结果未知时继续重放同一请求，不按用户名子串或新读取版本重新拼装。成功或明确拒绝后才释放该请求。
撤销或重签 Key 同事务永久撤销旧 Public 会话，邮件重签遵守相同语义。


### 外部凭据

A32 `/api/v1/admin/credentials` 已接入 Admin session、origin/CSRF、`command_id` 和
`expected_state_version`，支持 `mail_cookie`/`forum_cookie` 的安全投影列表、导入、改名、启停
和删除。secret 只在应用加密边界短暂存在，不进入响应、审计、命令结果或页面；该路由要求
PostgreSQL 完整档与独立 `credential_data_key`，SQLite 或未配置密钥时返回
`501 feature_not_supported`。Worker 的 active 凭据读取不属于浏览器 DTO，`.keyenv` 只作为兼容
回退入口。`mail_cookie` 与 `forum_cookie` 各自全局最多一条 active 记录；导入或重新启用同类第二条
时返回 `409 credential_conflict`，调用方必须先通过同一路由停用旧记录并重启 Worker 完成交接。

A44 `POST /api/v1/admin/credentials/{credential_id}/check` 只校验已保存凭据，不接受原始 Cookie 或远端 URL。
请求是严格 JSON `{ "expected_state_version": 1 }`，最多 4 KiB，无查询参数。保留 Admin session、
Origin、CSRF 与 mutation 限流；尽管使用 POST 防止跨站/预取触发，操作只读，不使用 `command_id`
或持久化命令账本。active、disabled、invalid 均可检查，不改变启停、Worker 所有权、版本、审计或重启要求。

响应使用标准 `ApiResponse`；`data` 的必填闭集为 `credential_id`、`state_version`、
`checked_at`（UTC RFC3339）和 `checks`。每项仅含 `world` 与 `status`：
`status` 为 `valid`、`invalid`、`inconclusive`。论坛固定一项 `shared`，只证明登录态，不证明发帖权限；
游戏按凭据范围检查 `main` 或 `isekai`，shared 固定按 main、isekai 顺序各返回一项。
网络、超时、限流、战斗页、403/Cloudflare、未知页面或格式无法确认返回 `inconclusive`；
只有明确登录失效证据才返回 `invalid`，两者仍为 HTTP 200，不触发 Admin 退出登录。

探测固定 HTTPS 论坛用户控制台与游戏 Item Shop 余额页；不跟随重定向、不自动重试、
不打开邮件或执行发帖、compose、send、take、return。每个 Admin 进程最多一个校验在飞，
各站点请求起始间隔至少 2 秒，每个范围连同等待最多 5 秒、响应最多 2 MiB。
并发检查返回 `429 rate_limited`（`Retry-After: 2`）。探测前后复核数据库版本；
变化返回 `409 stale_credential_state`，删除返回 404。密钥/校验适配器失败返回 503，
SQLite 或未装配端口返回 `501 feature_not_supported`。
结果为当次观测、响应 `no-store`，不落库、不回显 Cookie/账号/余额/HTML，不保证后续请求可用。

## 契约变更规则

1. Rust 路由必须先在本表登记，再实现 handler；已发布路径不得为了方便直接重命名。
2. DTO 必须在 auction-contracts 中独立定义；不得把 SQL 行或 Rust storage model 直接暴露给浏览器。
3. 所有改变状态的核心命令必须保留 command_id；同一 ID 搭配不同 request hash 必须返回 409。
   A24 v1 兼容请求没有该字段，由 HTTP 防腐层按 failure 与修正负载稳定派生，不得使用随机 ID。
4. 公开实时更新不以 HTTP 分页替代 WebSocket；无法保持长连接时按 events catch-up 语义恢复。
   SSE 尚无登记路由，不能将该扩展方向描述为现有客户端通道。
5. 本表的状态码、JSON 字段和错误 code 变化必须同时更新本文件、相关契约记录和
   对应 owner 的边界说明。
