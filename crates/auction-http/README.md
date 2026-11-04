# auction-http

[文档导航](../../docs/README.md) · [HTTP 路由](../../docs/contracts/routes.md) · [实时事件](../../docs/contracts/realtime.md)

Public 与 Admin 两个 HTTP 表面的适配器：路由表、请求边界、会话 Cookie 与 CSRF、origin allowlist、进程内速率限制、浏览器安全头、DTO 映射、WebSocket 实时 Hub、内嵌浏览器资源与 SPA 路由、健康探针与 Prometheus 端点，以及 TCP/Unix listener 的连接服务。业务能力全部通过应用端口注入（`PublicApiState`、`AdminApiState`），端口缺失时对应接口返回明确的能力不可用错误，不伪造空数据。

## 对外入口

| 入口 | 用途 |
| --- | --- |
| `public_router::<Assets>(version, boundary)` / `admin_router::<Assets>(…)` | 构建两个表面的 axum `Router`；`Assets` 为装配根选择的 `RustEmbed` 资源类型 |
| `PublicApiState` / `AdminApiState` | 端口集合（`Option<Arc<dyn …>>`），以 builder 方式追加可选能力 |
| `PublicHttpSecurity` / `AdminHttpSecurity` | origin、Cookie、限流、可信代理与请求超时策略，启动时校验 |
| `serve_http` | 基于 hyper-util 的 HTTP/1.1 与 HTTP/2 连接服务，带优雅关闭 |
| `UnixHttpListener` | Linux Unix socket listener：受信运行目录、实例锁文件、socket 组与 0660 权限、stale socket 安全清理 |
| `RealtimeHub` | Public 进程内的 WebSocket 广播 Hub，每连接有界缓冲，慢连接被淘汰 |
| `serve_admin_setup_asset` | 首次设置页面复用 Admin 内嵌资源 |

## 请求边界

- **请求 ID**：读取或生成 `x-request-id` 并始终在响应头回传，非法或重复输入拒绝；指标转发拒绝的 404 优先于 request-id 400，仍回传安全生成的标识；常规 handler 错误的 JSON `request_id` 字段可能为 null。
- **客户端地址**：只有来自 `trusted_proxy_cidrs` 的 TCP 对端或启用 `trusted_unix_proxy` 的 Unix 上游，才采信 `X-Forwarded-For`；限流按真实客户端地址计数。
- **origin**：启用 allowlist 时，普通状态变更请求允许省略 `Origin`；一旦提供，必须是唯一且精确匹配的值。WebSocket 升级必须且只能携带一个可信 Origin。
- **限流**：固定窗口、按地址计数，登录、状态变更与 realtime 连接尝试使用独立账本；超限返回 `429`、`code = "rate_limited"` 与 `Retry-After` 头。
- **超时**：普通请求与 WebSocket 升级前握手受 `request_timeout_seconds` 限制，不限制升级后的连接寿命。
- **严格解析**：JSON 写接口拒绝未知字段与非 JSON 内容类型；空 PUT 会话同步、Basic 登录等遵循各自协议。Public/Admin 业务路由严格匹配方法，JSON GET 不隐式接受 HEAD，倒计时显式支持 GET/HEAD；健康、指标与静态资源由各自 owner 定义方法语义。
- **安全头**：`X-Frame-Options: DENY`、内容类型嗅探禁止、来源策略与 CSP（为 Dioxus WASM 放行 `wasm-unsafe-eval` 与内联样式）；响应缓存策略按路由冻结（`no-store`/`no-cache`）。
- **指标**：全局边界按路由模板记录响应量、状态码与响应生成耗时，包含健康、指标与资源 fallback；连接与频率拒绝分别在成功升级和 guard 实际拒绝点记录。标签、桶与生命周期见 [auction-observability](../auction-observability/README.md#http-与连接口径)。

## 会话

| 表面 | Cookie | 说明 |
| --- | --- | --- |
| Public | `auction_public_session` | 登录 key 换取不透明 token，HttpOnly；时长由账号偏好决定 |
| Admin | `auction_admin_session` + `auction_admin_csrf` | 写操作须在 `x-csrf-token` 头回传 CSRF token |

`cookie_secure = true` 时 Cookie 带 `Secure`，生产 HTTPS 必须开启。

Admin 写守卫先检查限流、Origin 与 CSRF，再读取权威会话；CSRF Cookie 的重复项、空值和畸形 Cookie 仍拒绝。已核对的 CSRF 值在同一请求中复用，避免再次扫描并复制 Cookie。Basic 登录复用解码缓冲区承载密码，Public 拍卖筛选和游标直接转移已解码字符串；这些优化不改变认证顺序、请求分类或响应字段。

WebSocket 每连接的发送队列有界。队列满时 Hub 淘汰发送端，连接下一次接收即丢弃积压事件并以 `1013` 关闭，避免逐帧发送旧缓冲而长期占用连接配额；配额仍在升级任务真正结束时归还。

## 浏览器资源

`web-embed` 构建中，资源来自 `dx bundle` 产物；API-only 构建使用固定的空资源目录。除精确存在的静态资源外，只有 [`crates/auction-http/src/assets/routes.rs`](src/assets/routes.rs) 中登记的浏览器路径（Public 参数路径 `/lots/{lot_code}`、Admin 参数路径 `/auctions/{auction_id}/lots`）返回 `index.html`，未知路径为 404；资源缺失时页面路由返回 503 与提示文本。新增或删除页面时须同时修改前端 `Route` 枚举、该清单与[路由文档](../../docs/contracts/routes.md)。

## 健康与指标

| 路径 | 含义 |
| --- | --- |
| `/health/live` | 进程存活 |
| `/health/ready` | Public：数据库与 realtime relay 新鲜；Admin：数据库就绪 |
| `/health`、`/api/v1/health` | 服务名、状态与运行进程版本 |
| `/metrics` | 直连 GET/HEAD 的 Prometheus 文本；需启用且 listener 为 loopback 或 Unix；其他方法 405，默认携带转发头的任意方法优先返回空 404 |

`/metrics` 通过 `any(...)` 挂载，由共用处理器显式保留 GET/HEAD 和直连 405，避免框架向转发拒绝追加 `Allow`。默认拒绝 `Forwarded`、全部 `X-Forwarded-*`、`X-Real-IP`、`Via` 的存在，不依赖代理是否可信，也不解析或记录头值；拒绝经 `finish_response` 补安全头与 `no-store`，优先于 request-id 400 和方法错误。CDN 专用来源头待 W0.12 核实后补充。完整边界见[路由契约](../../docs/contracts/routes.md#健康与指标publicadmin)。

部署层 `common.toml [metrics] reject_forwarded_requests` 默认 true；设为 false 并重启 Public/Admin 回到旧访问行为。该字段不进入 Admin runtime 文档；runtime 只覆盖 `metrics.enabled`，保留部署层拒绝策略。listener 限制和 Caddy 的公网 404 规则继续生效，直接采集与 SSH 隧道见[运维手册](../../ops/README.md#指标)。

`serve_metrics` 为三个装配根提供仅含 `/metrics` 的附加 HTTP 适配，
接收已绑定 TCP listener，复用上述 boundary 和指标渲染函数。
只持有同一个 `ProcessMetrics`，不引入业务 state、control 或资源路由；
服务异常返回装配根，按固定类别记录 ERROR 并仅降级采集。
完整契约见[三角色附加出口](../../docs/contracts/routes.md#三角色附加指标出口loopback-tcp)。

`HttpReadinessProbe` 由装配根包装完整角色判定，默认只缓存最终布尔值一秒并合并并发；每轮限时 750ms，请求断开不取消共享刷新，panic/取消/超时均发布未就绪并结束单飞。正负都缓存，过期后不继续返回旧成功，响应保持 `no-store`。关闭缓存恢复逐次检查与原请求预算；心跳不复用此缓存。转换延迟及模式语义由[路由契约](../../docs/contracts/routes.md#健康与指标publicadmin)定义，配置见[就绪策略](../../ops/config/README.md#就绪策略)。

## 其他

- `GET|HEAD /api/v1/auctions/{auction_id}/countdown/{timer}` 输出自托管的论坛倒计时 SVG 图片。
- Admin lot 列表在响应头 `x-auction-capability-manual-intake` 中声明手工录入能力是否可用。

## 源码结构

| 路径 | 内容 |
| --- | --- |
| `crates/auction-http/src/http/routes/` | 路由表（`crates/auction-http/src/http/routes/surfaces.rs`）、会话、Admin 生命周期/结算/恢复处理器、读接口 DTO 映射、健康 |
| `crates/auction-http/src/http/public/`、`crates/auction-http/src/http/admin/` | 各功能的 HTTP 适配器与错误映射 |
| `crates/auction-http/src/http/security/` | origin、限流、Cookie 策略与安全头 |
| `crates/auction-http/src/http/shared/` | 请求边界、严格 JSON、方法严格性、响应缓存策略、连接服务、Unix listener |
| `crates/auction-http/src/http/metrics/` | Prometheus scrape 路由 |
| `crates/auction-http/src/assets/` | 内嵌资源与浏览器路由清单 |


## 运行时就绪策略

`HttpReadinessProbe` 使用共享策略身份（revision、覆盖期限状态与有效值）隔离短缓存和单飞任务；策略改变后旧结果不能用于新缓存，等待者也不能返回过期策略的成功。认证只读 `GET /api/v1/admin/runtime-switches` 显式映射数据库意图和 Admin 本地快照，成功及读取失败 no-store，不依赖 runtime 文件。见[路由契约](../../docs/contracts/routes.md#运行时开关只读接口)。


## 实时 SLI 观测

Public 装配根注册 `RealtimeRecorder` 并注入 Hub；关闭 metrics 时记录器为 no-op。
`ObservedFrame` 仅在进程内携带创建时间与 scope，正文仍是原来的已编码 Utf8Bytes。
WS 事件 send 成功后才记 push；Hub sink 交接记 relay，均不改变原控制流。
公开 `RealtimeSubscription::recv` 继续返回原文本帧，内部 WS 使用带观测信息的接收。
线协议、补读、游标、缓冲与淘汰边界见[实时契约](../../docs/contracts/realtime.md#推送观测边界)。

## 安全错误记录

W0.2 I-B 覆盖类型化错误映射、5xx helper、直接 5xx、请求 panic/超时、
listener/连接、WebSocket、就绪缓存与 Admin 运行时开关读取失败。
在原错误即将折叠时先投影，再返回原响应；统一响应观察只检查状态码，
不读取或修改正文、JSON、错误码、响应头、会话或命令。

诊断 target 为 `auction::diagnostics`，字段为固定 owner `kind`、语义 `phase`、
闭集 `safe_kind` 与已有业务 UUID。`route` 来自框架 `MatchedPath` 的路由模板，
绝不使用原始路径、URL、query 或请求头。无法匹配时固定为 `__unmatched__`；
无请求的监听器、连接与后台路径分别使用 `__listener__`、`__connection__`、`__background__`。
请求内复用既有 span 的规范化 `request_id`，不另生成 ID；升级任务和就绪共享刷新显式继承
安全路由与原 span，不保留整个请求。日志门不缓存 route、request-id 或业务标识。

500、内部不变量与 panic 用 ERROR；依赖、传输、超时和未确认结果用 WARN；
未启用能力与 501 用 INFO，正常 4xx 拒绝不记 ERROR。
JSON、游标、参数、认证格式等闭集输入解析继续固定拒绝，不记录输入或底层内容。
`api_error` 与请求结束的 5xx 观察按状态去掉重复日志，类型化原因在折叠前单独保留。

Public/Admin 装配根在原服务生命周期中安装诊断日志门、刷新到期窗口并在关闭时 flush/close；
不增加后台任务。HTTP 不设置 Unpersisted 处置，不增加 Worker W0.3 计数。
窗口、回退方式见[可观测性说明](../auction-observability/README.md#重复错误去重)，
owner 与过滤例子见[排障手册](../../ops/README.md#安全错误与未持久化指标)。
持久化值、HTTP 状态码、错误码、JSON、超时预算、缓存判定与请求控制流全部保持原契约。
