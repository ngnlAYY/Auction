# auction-http

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

Public/Admin HTTP 路由、认证、安全、DTO 映射和静态资源适配。处理器只把 HTTP 输入映射为应用命令、把应用结果映射为
`auction-contracts` DTO；竞价、结算、身份与状态机规则归领域/应用层，SQL 归存储层，正式 Admin 不挂载首次设置 API。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/http/routes/surfaces.rs](crates/auction-http/src/http/routes/surfaces.rs) | 两个表面的路由注册闭集 |
| [src/http/routes](crates/auction-http/src/http/routes) | 健康、会话、核心竞价/生命周期及映射 |
| [src/http/public](crates/auction-http/src/http/public)、[src/http/admin](crates/auction-http/src/http/admin) | 各自业务处理器 |
| [src/http/shared](crates/auction-http/src/http/shared) | 请求边界、端口状态与实时 Hub |
| [src/http/security](crates/auction-http/src/http/security)、[src/http/metrics](crates/auction-http/src/http/metrics) | 安全策略与进程指标表面 |
| [src/assets](crates/auction-http/src/assets) | 静态入口、缓存、条件请求与 404 |

## 路由与源码布局

P09 接收 `equipment_group=rare|fusion` 并交给应用读取端口，在分页前与类型组合筛选；P09/P11 映射服务端分组到
正式装备、预览摘要和预览详情 DTO，边界检查返回行与请求分组一致。具体字段及统计约定见
[装备分组契约](docs/contracts/routes.md#p09p11-装备分组)。

[HTTP 契约](docs/contracts/routes.md)是路由、状态码、DTO、分页和能力清单；Public 指定列表的 `page_size` 不设 200 条上限，仍严格检查正整数、
offset 溢出及 cursor 互斥；[assets/routes.rs](crates/auction-http/src/assets/routes.rs)
与 Dioxus Route 共同限定 15 条 Public、21 条正式 Admin 浏览器路径，修改任一方必须同步其余两处。
P12 成交价编号页的 `sort` 仅接受 `closed_desc`（默认）、`closed_asc`、`price_desc`、`price_asc`；
名称同时接受 Unicode 空白及英文逗号分词，空词忽略，原有总长 1024 字节、最多八词及每词 128 字节边界保留；
旧 cursor 仅配默认排序，其他组合返回 400。可选 `stats.median_exact` 保留精确半个 Credit，原整数 `median` 继续兼容。
`http/mod.rs` 与各 `mod.rs` 只做装配/导出，`shared/state/{public,admin}.rs` 分别持有两个表面的窄端口、不能互用。

[public/countdown](crates/auction-http/src/http/public/countdown) 拥有论坛动态 SVG 的 HTTP、固定英文视图及有界 CSS 动画，
复用公开时间窄端口，无 JS、外链资源或会话依赖（见[图片契约](docs/contracts/routes.md#p22-自托管论坛倒计时)）。
[routes/public/watch.rs](crates/auction-http/src/http/routes/public/watch.rs) 拥有 P23/P24 装备标记列表与 CAS 写入，
[routes/public/watch_preference.rs](crates/auction-http/src/http/routes/public/watch_preference.rs) 拥有 P25/P26 账号渠道
GET/PUT：两者复用会话、来源/CSRF、限流、strict JSON、JSON 405 与 `no-store`，不接收客户端 user_id；
词法错误 400、业务错误 422、命令/版本冲突 409、依赖缺失或集合超限 503，旧 channel 只接受 mm 占位。
[admin/forum/status.rs](crates/auction-http/src/http/admin/forum/status.rs) 拥有 Admin 认证论坛状态 GET，
严格空查询、`no-store`、不访远端论坛、不执行恢复授权。
[admin/runtime_config/taxonomy.rs](crates/auction-http/src/http/admin/runtime_config/taxonomy.rs) 把领域统一装备词典
投影为只读 DTO；GET/validate/PUT 仅在显式 `include_equipment_taxonomy=true` 时附带该字段，
旧请求保持原响应字段集合，查询不进入配置指纹或命令摘要。映射器不复制分类词表、不执行收件策略。
品质候选从领域词典筛选为 `Peerless`、`Legendary`、`Magnificent`；领域名称解析仍保留其它品质的兼容识别。
`kinds` 保留历史完整基础名索引及护甲种类/部位，`materials` 提供无部位的护甲种类；Admin 按此区分新候选和历史精确值。
固定词典主体使用进程内 `OnceLock` 缓存公开 DTO；每次响应克隆主体，再从本次读取、校验或保存文档的标色规则
单值及数组中，经领域 `resolve_kind` 为合法有限兼容种类补充独立 `resolved_kinds`（最多 16 项）。
缓存不持有配置或草稿，不改变既有 `no-store` 响应头、词典 revision、文档指纹或命令重放结果。

## 请求边界与能力边界

请求边界统一处理合法 `X-Request-ID`、trace context、安全头、访问日志、panic 与超时；普通请求处理预算 15 秒
（WebSocket 豁免），连接层 HTTP/1 请求头预算固定 5 秒。默认忽略 `X-Forwarded-For`，仅显式信任的 TCP/Unix 代理
可提供客户端地址；Unix listener 不伪造 TCP peer，应用不根据转发协议头推导 Origin 或 Secure Cookie
（见[橙云部署](ops/README.md#cloudflare-整站代理)）。Public 竞价按限流 → Origin → 认证执行，Admin mutation 按
限流 → Origin → CSRF → 会话执行，之后才读取校验 query/path/body；严格 JSON 保留各路由大小上限与字段闭集。
Public 竞价、代理、标记、警告渠道和注销在 Cookie 认证后核对可选 `x-auction-expected-user-id`；规范非零 UUID
与当前身份不符返回 409 `session_changed`，无效或重复头返回 400 `invalid_expected_user_id`。省略该头保留旧 v1，读取路由行为不变。
错误优先级、405/Allow 和缓存要求见[通用响应与错误](docs/contracts/routes.md#通用响应与错误)。
Admin 结算报告读取或不变量校验失败时记录 `admin_settlement_report_failed`，字段限定为 `auction_id` 与
闭集 `failure_code`，通过请求 span 关联请求 ID；底层错误文本、SQL 和敏感值不进入该诊断事件。

- 写命令使用稳定 `command_id`、摘要与版本门禁；成功竞价/代理响应直接使用事务内冻结结果，不在提交后重读权限。
- 按已登记能力返回安全结果或明确不支持，不以空列表和默认值伪造成功。
- Public 装备链接只在受限 DTO 边界短暂包含完整 URL，P09/P11 固定 `no-store`。
- Admin 凭据、用户 key、邮件诊断和 dashboard 严格投影安全字段，登录 key 只出现在创建/重签成功响应。
- Runtime 保存与 Worker 唤醒接受结果不表示配置已热加载或远端任务完成。

## 实时连接与恢复

[public/realtime](crates/auction-http/src/http/public/realtime) 拥有 Catch-up、握手、事件校验、连接生命期和只读会话复核；
WebSocket 支持 HTTP/1.1 Upgrade 与 HTTP/2 扩展 CONNECT，只有服务端通知，不承载竞价命令。握手先检查会话与公开范围，
单拍卖范围拒绝 settled，配额许可随连接 RAII 释放。Public 进程通过持久化 relay 向有界本地 Hub 分发；
跳号、重连或 `refetch_required` 按[实时契约](docs/contracts/events.md)执行 Catch-up/权威快照恢复，
慢连接可被淘汰，不把本地 Hub 或事件 payload 当成权威状态。

## 静态产物内嵌

Public/Admin 装配根分别持有 `RustEmbed` 类型，仅显式 `web-embed` 制品选择对应资源，API-only 使用固定空集合。
`bash ci/release/web-bundle.sh public|admin` 通过 `dx bundle` 生成并复制到 `target/dx/build/auction-web-public/web/public`
与 `target/dx/build/auction-web-admin/web/public`。保留 `allow_missing = true`：无前端产物仍可编译，已登记浏览器路径
返回 503，未知路径返回 404；默认分体制品不包含另一端资源，合并二进制的子进程也只提供所属表面。

分派顺序为业务路由、精确静态文件、显式浏览器路由、404，未登记 `/api`/`/health` 不回退 SPA。静态文件只接受
GET/HEAD，浏览器入口与直接 HTML `no-store`，稳定文件名（含 favicon 别名）`no-cache`，仅非 HTML 内容指纹资源使用
一年 `immutable`；正式边界默认给全部响应双 CDN `no-store` 头，只有静态 handler 私有标记可启用资源 CDN 策略。
条件读取与各类 API 缓存见[静态契约](docs/contracts/routes.md#静态入口与缓存)。

## 维护与验证

```bash
cargo check --locked -p auction-http
```

仅静态编译；默认检查与测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
字段、路由或部署行为变化时同步对应契约和配置模板。
