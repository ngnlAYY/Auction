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

- **请求 ID**：读取或生成 `x-request-id` 并始终在响应头回传，非法或重复输入拒绝；常规 handler 错误的 JSON `request_id` 字段可能为 null。
- **客户端地址**：只有来自 `trusted_proxy_cidrs` 的 TCP 对端或启用 `trusted_unix_proxy` 的 Unix 上游，才采信 `X-Forwarded-For`；限流按真实客户端地址计数。
- **origin**：启用 allowlist 时，普通状态变更请求允许省略 `Origin`；一旦提供，必须是唯一且精确匹配的值。WebSocket 升级必须且只能携带一个可信 Origin。
- **限流**：固定窗口、按地址计数，登录、状态变更与 realtime 连接尝试使用独立账本；超限返回 `429`、`code = "rate_limited"` 与 `Retry-After` 头。
- **超时**：普通请求与 WebSocket 升级前握手受 `request_timeout_seconds` 限制，不限制升级后的连接寿命。
- **严格解析**：JSON 写接口拒绝未知字段与非 JSON 内容类型；空 PUT 会话同步、Basic 登录等遵循各自协议。Public/Admin 业务路由严格匹配方法，JSON GET 不隐式接受 HEAD，倒计时显式支持 GET/HEAD；健康、指标与静态资源由各自 owner 定义方法语义。
- **安全头**：`X-Frame-Options: DENY`、内容类型嗅探禁止、来源策略与 CSP（为 Dioxus WASM 放行 `wasm-unsafe-eval` 与内联样式）；响应缓存策略按路由冻结（`no-store`/`no-cache`）。

## 会话

| 表面 | Cookie | 说明 |
| --- | --- | --- |
| Public | `auction_public_session` | 登录 key 换取不透明 token，HttpOnly；时长由账号偏好决定 |
| Admin | `auction_admin_session` + `auction_admin_csrf` | 写操作须在 `x-csrf-token` 头回传 CSRF token |

`cookie_secure = true` 时 Cookie 带 `Secure`，生产 HTTPS 必须开启。

Admin 写守卫先检查限流、Origin 与 CSRF，再读取权威会话；CSRF Cookie 的重复项、空值和畸形 Cookie 仍拒绝。已核对的 CSRF 值在同一请求中复用，避免再次扫描并复制 Cookie。Basic 登录复用解码缓冲区承载密码，Public 拍卖筛选和游标直接转移已解码字符串；这些优化不改变认证顺序、请求分类或响应字段。

## 浏览器资源

`web-embed` 构建中，资源来自 `dx bundle` 产物；API-only 构建使用固定的空资源目录。除精确存在的静态资源外，只有 [`crates/auction-http/src/assets/routes.rs`](src/assets/routes.rs) 中登记的浏览器路径（Public 参数路径 `/lots/{lot_code}`、Admin 参数路径 `/auctions/{auction_id}/lots`）返回 `index.html`，未知路径为 404；资源缺失时页面路由返回 503 与提示文本。新增或删除页面时须同时修改前端 `Route` 枚举、该清单与[路由文档](../../docs/contracts/routes.md)。

## 健康与指标

| 路径 | 含义 |
| --- | --- |
| `/health/live` | 进程存活 |
| `/health/ready` | Public：数据库与 realtime relay 新鲜；Admin：数据库就绪 |
| `/health`、`/api/v1/health` | 服务名、状态与运行进程版本 |
| `/metrics` | Prometheus 文本；需配置启用，且只允许 loopback 或 Unix listener |

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
