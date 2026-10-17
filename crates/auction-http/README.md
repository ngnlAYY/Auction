# auction-http

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

Public/Admin HTTP 路由、认证、安全、DTO 映射和静态资源适配。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [路由与源码布局](#路由与源码布局)
- [职责边界](#职责边界)
- [命令结果与能力边界](#命令结果与能力边界)
- [实时连接与恢复](#实时连接与恢复)
- [静态产物内嵌](#静态产物内嵌)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/http/routes/surfaces.rs](src/http/routes/surfaces.rs) | 两个表面的路由注册闭集 |
| [src/http/routes](src/http/routes) | 健康、会话、核心竞价/生命周期及映射 |
| [src/http/public](src/http/public)、[src/http/admin](src/http/admin) | 各自业务处理器 |
| [src/http/shared](src/http/shared) | 请求边界、端口状态与实时 Hub |
| [src/http/security](src/http/security)、[src/http/metrics](src/http/metrics) | 安全策略与进程指标表面 |
| [src/assets](src/assets) | 静态入口、缓存、条件请求与 404 |

## 职责与实现边界

处理器只把 HTTP 输入映射为应用命令，把应用结果映射为 `auction-contracts` DTO。
竞价、结算、身份与状态机规则归领域/应用层，SQL 归存储层。正式 Admin 不挂载首次设置 API。

## 路由与源码布局

[admin/forum/status.rs](src/http/admin/forum/status.rs) 拥有 Admin 认证论坛状态 GET，复用应用只读端口和 contracts 安全 DTO。
两条状态路由均严格空查询、`no-store`，不访问远端论坛、不执行恢复授权；Public 既有读取路由保持不变。

`http/mod.rs` 与各 `mod.rs` 保持装配/导出职责，业务目录按查询、命令、映射及安全边界拆分。
`shared/state/{public,admin}.rs` 分别持有两个表面的窄端口，不能互用管理与公开业务状态。
[public/countdown](src/http/public/countdown) 拥有论坛动态 SVG 的 HTTP、固定英文视图及有界 CSS 动画，复用公开时间窄端口。
此图片不是静态 payload，无 JS、外链资源或会话依赖；GET/HEAD、可见性、降级和专用 CSP 见[图片契约](../../docs/contracts/routes.md#p22-自托管论坛倒计时)。

[HTTP 契约](../../docs/contracts/routes.md)是路由、状态码、DTO、分页和能力清单；
[assets/routes.rs](src/assets/routes.rs) 与 Dioxus Route 共同限定 15 条 Public、21 条正式 Admin 浏览器路径。
修改任一方必须同步其余两处；首次 `/setup` 只归独立设置服务。
Public 指定列表的 `page_size` 不设 200 条上限，仍严格检查正整数、offset 溢出及 cursor 互斥；适用路径和容量代价见[分页契约](../../docs/contracts/routes.md#public-分页世界与金额)。

[routes/public/watch.rs](src/http/routes/public/watch.rs) 拥有 P23/P24 当前会话装备标记列表和 CAS 写入。
复用会话、来源/CSRF、限流、strict JSON、JSON 405 与 no-store；不接收客户端 user_id。
只接受 mm；查询/UUID/JSON 词法错误 400，P24 负版本或渠道业务错误 422，命令/版本冲突 409，依赖缺失或完整集合超限 503。
不增加竞价 DTO 或实时事件；GET 沿既有账户读取边界，PUT 使用变更限流与来源门禁。

## 职责边界

请求边界统一处理合法 `X-Request-ID`、trace context、安全头、访问日志、panic 与超时。
普通请求默认处理预算 15 秒，WebSocket 豁免；连接层 HTTP/1 请求头预算固定 5 秒。
默认忽略 `X-Forwarded-For`，仅显式信任的 TCP/Unix 代理可提供客户端地址。
Unix listener 不伪造 TCP peer，Unix 信任不放宽 TCP；Cloudflare 地址头由受信 Caddy 清洗为 XFF，
应用不根据转发协议头推导 Origin 或 Secure Cookie，见[橙云部署](../../ops/README.md#cloudflare-整站代理)。

Public 竞价按限流 → Origin → 认证执行，Admin mutation 按限流 → Origin → CSRF → 会话执行，
之后才读取并校验 query/path/body。严格 JSON 保留各路由大小上限、媒体类型与字段闭集，业务 GET 不隐式开放 HEAD。
Axum 显式保留 HTTP/JSON、路由上下文、日志和 WebSocket feature，不启用未用的宏及 Form/Query 提取器；
query 仍由本层按闭集解析并拒绝重复键，Hyper 的 HTTP/2 与连接排空配置不变。
完整错误优先级、405/Allow 和缓存要求见[请求契约](../../docs/contracts/routes.md#通用响应与错误)。

## 命令结果与能力边界

- 写命令使用稳定 `command_id`、摘要与版本门禁；成功竞价/代理响应直接使用事务内冻结结果，不在提交后重读权限。
- 按已登记能力返回安全结果或明确不支持，不以空列表和默认值伪造成功。
- Public 装备链接只在受限 DTO 边界短暂包含完整 URL，P09/P11 固定 `no-store`。
- Admin 凭据、用户 key、邮件诊断和 dashboard 严格投影安全字段；生成登录 key 仅允许出现在创建/重签成功响应。
- Admin 重启撤销旧 Admin 会话；Public key 撤销/重签永久撤销对应旧会话。长连接复核不刷新 Public 会话。
- Runtime 保存结果与 Worker 唤醒接受结果不表示配置已热加载或远端任务完成；外部回执和业务世界启停由所属应用端口判断。

各业务命令的细节统一见[已实现路径约束](../../docs/contracts/routes.md#已实现路径的补充约束)。

## 实时连接与恢复

[public/realtime](src/http/public/realtime) 拥有 Catch-up、握手、事件校验、连接生命期和只读会话复核。
WebSocket 支持 HTTP/1.1 Upgrade 与 HTTP/2 扩展 CONNECT，只有服务端通知，不承载竞价命令。
握手先检查会话与公开范围，单拍卖范围拒绝 settled；配额许可随连接 RAII 释放。

Public 进程通过持久化 relay 向有界本地 Hub 分发；discovery 与单拍卖流分别恢复。
跳号、重连或 `refetch_required` 按[实时契约](../../docs/contracts/events.md)执行 Catch-up/权威快照恢复，
慢连接可被淘汰，不把本地 Hub 或事件 payload 当成权威状态。
会话在首次推送前及连接期间只读复核，失效与依赖故障使用不同关闭分类。

## 静态产物内嵌

Public/Admin 装配根分别持有 `RustEmbed` 类型，仅显式 `web-embed` 制品选择对应资源；API-only 使用固定空集合。
`just web-build-public` / `just web-build-admin` 通过 `dx bundle` 生成并复制到：

```text
target/dx/build/auction-web-public/web/public
target/dx/build/auction-web-admin/web/public
```

保留 `allow_missing = true`：无前端产物仍可编译，已登记浏览器路径返回 503；未知路径返回 404。
默认分体制品不包含另一端资源；合并二进制虽然链接两端，子进程仍只提供所属表面。

分派顺序为业务路由、精确静态文件、显式浏览器路由、404；未登记 `/api`/`/health` 不回退 SPA。
静态文件只接受 GET/HEAD，浏览器入口与直接 HTML `no-store`，稳定文件名（含 favicon 别名）`no-cache`，
仅非 HTML 内容指纹资源使用一年 `immutable`。正式边界默认给全部响应发送双 CDN `no-store` 头，
只有静态 handler 的私有标记可启用资源 CDN 策略；API 浏览器策略保持兼容，设置资源禁止 CDN 保存。
具体条件读取和各类 API 缓存见[静态契约](../../docs/contracts/routes.md#静态入口与缓存)。

## 维护与验证

```bash
cargo check --locked -p auction-http
```

仅静态编译；默认检查与测试范围见[开发指南](../../docs/development.md)，运行证据见[验证状态](../../docs/status.md)。
字段、路由或部署行为变化时同步对应契约和配置模板。
