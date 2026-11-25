# auction-http

[docs/README.md](../../docs/README.md) · [docs/contracts/routes.md](../../docs/contracts/routes.md) · [docs/contracts/realtime.md](../../docs/contracts/realtime.md)

Public 与 Admin 两个 HTTP 表面的适配器：路由表、请求边界、会话 Cookie 与 CSRF、origin allowlist、进程内限流、浏览器安全头、DTO 映射、WebSocket 实时 Hub、内嵌浏览器资源与 SPA 路由、健康探针与 Prometheus 端点，以及 TCP/Unix listener 的连接服务。业务能力全部通过应用端口注入（`PublicApiState`、`AdminApiState`），端口缺失时返回明确的能力不可用错误。

## 对外入口

| 入口 | 用途 |
| --- | --- |
| `public_router_with_web_script_policy::<Assets>(version, boundary, anonymous_read_gate, policy)` / `admin_router_with_web_script_policy::<Assets>(version, boundary, policy)` | 构建两个表面的 axum `Router`，并由装配根按 surface 的实际前端技术栈显式传入 `WebScriptPolicy`，同一策略用于 CSP 与静态资源的指纹命名判定；`Assets` 为装配根选择的 `RustEmbed` 类型，匿名读闸门句柄取自 `PublicApiState::anonymous_read_gate` |
| `WebScriptPolicy::{Dioxus, Solid}` | 闭集脚本策略：Dioxus 为 `script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'`，Solid 为 `script-src 'self'`；两者都保留 `style-src 'self' 'unsafe-inline'` |
| `PublicApiState` / `AdminApiState` | 端口集合（`Option<Arc<dyn …>>`），以 builder 追加可选能力 |
| `PublicHttpSecurity` / `AdminHttpSecurity` | origin、Cookie、限流、可信代理与请求超时策略，启动时校验 |
| `HttpListener` / `serve_http` / `UnixHttpListener` | TCP/Unix 已绑定 listener 与连接服务（5 秒请求头读取超时、优雅关闭）；Unix 变体只在 Linux 编译。Worker control socket 也用 `UnixHttpListener` 绑定，实例锁与 peer 过滤、路由见[crates/auction-jobs/README.md](../auction-jobs/README.md) |
| `spawn_metrics_listener` | 三角色附加指标出口；只持有同一 `ProcessMetrics` 与 `RuntimeSwitches`，绑定/服务失败只降级采集 |
| `RealtimeHub` | Public 进程内 WebSocket 广播 Hub，每连接有界缓冲，慢连接被淘汰 |
| `serve_admin_setup_asset` | 首次设置页面复用 Admin 内嵌资源；CSP、入口 `Link` 与静态资源回退随 Admin 的实际技术栈（装配根传入策略），全部响应禁止缓存 |

监听器归属、`TaskGuard` 生命周期与关闭顺序由装配根决定；`serve_http` 不解析配置、不做认证或限流。

## 请求边界

- **请求 ID 与来源**：读取或生成 `x-request-id` 并始终回传，非法或重复输入拒绝；只有来自 `trusted_proxy_cidrs` 的 TCP 对端或启用 `trusted_unix_proxy` 的 Unix 上游才采信 `X-Forwarded-For`，限流按真实客户端地址计数。
- **origin 与限流**：启用 allowlist 时状态变更请求可省略 `Origin`，一旦提供必须唯一且精确匹配；WebSocket 升级必须且只能携带一个可信 Origin。固定窗口按地址计数，登录、状态变更、realtime 尝试、匿名公开读取与浏览器错误信标使用独立账本（会话读取 `GET /api/v1/session` 与其余身份读取不计入任何账本），超限返回 `429 rate_limited` 与 `Retry-After`；匿名公开读取另有进程内并发上限，无许可时立即返回 `429 read_busy`（同样带 `Retry-After`）。
- **严格解析**：JSON 写接口拒绝未知字段与非 JSON 类型；方法严格匹配，JSON GET 不隐式接受 HEAD；普通请求与升级前握手受 `request_timeout_seconds` 限制，不限制升级后的连接寿命。
- **安全头**：`X-Frame-Options: DENY`、禁止类型嗅探、来源策略与 CSP（同源脚本、表单只提交同源；Dioxus WASM 版本放行 `unsafe-eval`/`wasm-unsafe-eval`，Solid 版本只允许同源脚本；两者都保留内联样式例外，理由见[docs/architecture.md#安全边界](../../docs/architecture.md#安全边界)）。策略由装配根按 surface 显式选择，合并服务里 Public 与 Admin 可以不同。HSTS 只在确认浏览器经 HTTPS 访问时发送，代理后只采信可信上游的 `X-Forwarded-Proto`。
- **指标**：全局边界按路由模板记录响应量、状态码与生成耗时；`route` 来自 `MatchedPath`，缺少模板固定 `__unmatched__`，不把原始路径、query、用户、IP 或 Cookie 放进 label。口径见[crates/auction-observability/README.md#http-与连接口径](../auction-observability/README.md#http-与连接口径)。

逐项状态码、头与顺序的完整契约见[docs/contracts/routes.md](../../docs/contracts/routes.md)。

## 会话

| 表面 | Cookie | 说明 |
| --- | --- | --- |
| Public | `auction_public_session` | 登录 key 换取不透明 token，HttpOnly；时长由账号偏好决定 |
| Admin | `auction_admin_session` + `auction_admin_csrf` | 写操作须在 `x-csrf-token` 头回传 CSRF token |

`cookie_secure = true` 时 Cookie 带 `Secure`，生产 HTTPS 必须开启。Admin 写守卫先检查限流、Origin 与 CSRF，再读取权威会话；已核对的 CSRF 值在同一请求中复用。WebSocket 每连接发送队列有界，队列满时 Hub 淘汰发送端并以 `1013` 关闭。

## 浏览器资源与健康

`web-embed` 构建的资源来自 `dx bundle`（含默认语言之外、带内容指纹的 FTL 译文），`web-solid` 构建的资源来自 `web/` 的 Vite 产物；接入另一套技术栈时，静态资源层按装配根传入的 `WebScriptPolicy` 判定指纹命名与不可变缓存，不按文件名猜测。Dioxus 使用 `-dxh<8–16 位十六进制>` 命名；Solid 对 `assets/` 下每个文件（含子目录）的最后一级文件名匹配 `名字-8位指纹.扩展名`。另一套产物的命名不会获得长期缓存。两套产物都须产出唯一的 `assets/favicon-*.svg` 供 `/favicon.ico` 映射。可压缩资源带构建期 brotli 副本，按 `Accept-Encoding` 协商并声明 `Vary`，`.br` 路径不对外提供。API-only 构建使用固定空资源目录。除精确静态资源外，只有 [crates/auction-http/src/assets/routes.rs](src/assets/routes.rs) 登记的参数路径返回 `index.html`，未知路径 404，资源缺失时页面路由 503；入口回退的 `Link` 中，Dioxus 保持旧的 `<link>` 逐条 `rel=preload` 转写，Solid 另识别 `modulepreload` 与入口模块脚本，精确 `/index.html` 不添加。新增或删除页面时同步前端 `Route`、该清单与[docs/contracts/routes.md](../../docs/contracts/routes.md)。

| 路径 | 含义 |
| --- | --- |
| `/health/live`、`/health/ready` | 进程存活；Public 为数据库与 realtime relay 新鲜，Admin 为数据库就绪 |
| `/health`、`/api/v1/health` | 服务名、状态与运行进程版本 |
| `/metrics` | 直连 GET/HEAD 的 Prometheus 文本；默认拒绝任何转发头（优先于 request-id 400），loopback/Unix 限制与 Caddy 404 规则见[ops/README.md#指标](../../ops/README.md#指标) |

`HttpReadinessProbe` 默认只缓存最终布尔值一秒并合并并发，正负都缓存、过期不返回旧成功；关闭缓存恢复逐次检查。策略语义见[docs/contracts/routes.md#健康与指标publicadmin](../../docs/contracts/routes.md#健康与指标publicadmin)与[ops/config/README.md#就绪策略](../../ops/config/README.md#就绪策略)。`RealtimeHub` 经 `RealtimeRecorder` 记录 push/relay 观测，关闭 metrics 时 no-op，线协议见[docs/contracts/realtime.md#推送观测边界](../../docs/contracts/realtime.md#推送观测边界)。

## 源码结构

| 路径 | 内容 |
| --- | --- |
| `crates/auction-http/src/server/` | 监听与连接服务、Unix listener、健康、readiness、构建号、指标出口与路由表（`surfaces.rs`） |
| `crates/auction-http/src/middleware/` | 请求边界：origin/限流/安全头（`security/`）、转发来源拒绝、严格 JSON、方法严格性、读闸与响应缓存 |
| `crates/auction-http/src/admin/` · `public/` | 两个 surface 的 handler、路由装配与只有单一调用方的 DTO 映射（如 `admin/reports/` 的结算与 recovery 报告校验、`public/account.rs` 的账户页投影、`public/competition/mapping.rs` 的代理命令结果）；`lifecycle`、`recovery`、`reports`、`session`、`alerts`、`runtime_switches`、`mail_templates`、`account`、`competition`、`watch` 等为各 surface 路由装配 |
| `crates/auction-http/src/public/consignment/` · `crates/auction-http/src/admin/consignment/` | 一口价挂单的参数、DTO 映射、处理器与路由组。两端由 `with_consignment` 分别注入 `ConsignmentCapabilities` 和 `AdminConsignmentCapabilities`；`public/consignment/deposit/` 提供 v2 状态、本人寄件指引、预约签发与代管件撤回，`admin/consignment/quarantine/` 提供隔离详情、重检、解除隔离、运营止售与解除。`public/consignment/purchase.rs` 与 `orders.rs` 提供公开 v2 目录、购买与本人订单，`recovery/` 提供卖家重列与本人损失回执，`rights/` 提供不经登录会话的受限权利六条路由（`with_purchase`、`with_orders`、`with_recovery`、`with_rights` 注入），`admin/consignment/orders.rs` 与 `loss/` 提供订单读取、损失案件与灭失确认（`with_orders`、`with_loss`、`with_loss_confirmation` 注入）。Solid 专属页面地址见[浏览器入口](../../docs/contracts/routes.md#浏览器入口)。路由与错误见[一口价挂单](../../docs/contracts/routes.md#一口价挂单) |
| `crates/auction-http/src/errors/` · `query/` · `mapping/` | 跨 handler 共用的稳定错误外壳（`api_error`、`query_rejection`、`feature_unavailable` 等）；查询白名单、正整数（含 Public 列表共用的 `page_size` 上限解析）、页码偏移与规范 UUID 解析；Admin/Public 时间戳格式，以及 Public 与 Admin 共用 v1 定长帧（`[版本][类型][正文]` 无填充 base64url）的五种 keyset 游标编解码与金标测试。handler 从定义处导入；`admin/`、`public/` 的模块门面不转出这些 helper，也不替子模块转出 `session/`、`state/` 的守卫与状态 |
| `crates/auction-http/src/session/` · `state/` · `reads/` · `realtime_hub/` | Cookie/CSRF/守卫；API state；Public 只读处理器及其 DTO 映射（`reads/mapping.rs`）；WebSocket Hub |
| `crates/auction-http/src/assets/` · `diagnostics/` | 内嵌资源与浏览器路由清单；诊断上下文与安全投影 |

三个 crate（`auction-http`、`auction-web-admin`、`auction-web-public`）的模块树与目录层级一致，不使用 `#[path]` 重排文件位置。

## 安全错误记录

HTTP 层错误投影覆盖类型化错误映射、5xx helper、请求 panic/超时、listener/连接与 WebSocket；在原错误折叠时先投影再返回原响应。诊断字段为固定 owner `kind`、语义 `phase`、闭集 `safe_kind` 与业务 UUID，`route` 只来自 `MatchedPath`。500/不变量/panic 用 ERROR，依赖/超时/未确认结果用 WARN，正常 4xx 不记 ERROR。完整规则见[crates/auction-observability/README.md#未持久化错误](../auction-observability/README.md#未持久化错误)与[ops/README.md#安全错误与未持久化指标](../../ops/README.md#安全错误与未持久化指标)。
