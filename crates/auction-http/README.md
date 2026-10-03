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
| [src/http/routes/surfaces.rs](src/http/routes/surfaces.rs) | 完整路由注册闭集 |
| [src/http/public](src/http/public) | Public handlers |
| [src/http/admin](src/http/admin) | Admin handlers |
| [src/http/shared](src/http/shared) | 边界中间件及 surface 状态 |
| [src/assets](src/assets) | 静态入口、缓存与错误分派 |

## 职责与实现边界

Public 与 Admin 的 HTTP 表面适配器。

## 路由与源码布局

- [src/http/routes/mod.rs](src/http/routes/mod.rs)：HTTP 子模块装配、稳定导出和跨 surface 共享类型；`routes/surfaces.rs`
  持有 `public_router()` / `admin_router()` 与冻结契约路径注册，`routes/health.rs` 持有健康检查；
  `routes/admin.rs` 只保留稳定出口，Admin 会话、生命周期、结算/恢复报告和恢复确认分别位于
  `routes/admin/{session,lifecycle,reports,recovery}.rs`；业务世界启停与 settled-only 报表目录分别由
  `src/http/admin/{worlds,settlement_catalog}.rs` 持有，不塞入通用配置或拍卖列表。`routes/public/`
  按会话、账户读取和竞价命令拆分 Public handler，
  其中竞价与代理命令位于 `routes/public/competition/{bid,proxy}.rs`；
  `routes/session/` 按 `guards.rs`（限流/认证）、`errors.rs`（错误映射）、`cookies.rs`
  （Cookie/Basic Auth/CSRF）、`responses.rs`（会话响应）和 `metrics.rs`（业务指标）拆分共享会话
  安全逻辑；Basic Auth 保持首个冒号分隔账号与口令，配置和会话服务初始化拒绝含冒号的 actor；Admin 写入统一由守卫保持限流 → Origin → CSRF → session 顺序，Admin 读取不消耗
  mutation 限流额度。`routes/session.rs` 只负责装配与稳定出口；`routes/reads/` 按世界/拍卖目录、竞价与价格历史、
  lot 列表/详情拆分 Public 只读处理器，`reads/mod.rs` 只负责装配与稳定出口；
  `routes/mapping/` 按查询、Public、Admin 和目录投影拆分契约映射。[src/http/mod.rs](src/http/mod.rs) 只负责
  surface 导出边界。[src/http/public/realtime/](src/http/public/realtime/) 再按列表读取、WebSocket 生命周期、目标解析和事件校验/映射
  拆分为 `{mod,list,websocket,target,mapping}.rs`：单拍卖回放保留旧版必填 `auction_id` 和响应信封，
  发现回放使用显式 `scope=auctions` 扩展，WebSocket 逐帧发送旧版单事件，并拥有 4 KiB 读取、
  50/60 秒 Ping/Pong、5 秒写入、稳定 close reason、settled 预检和 discovery 专属容量边界。
- [src/http/public/](src/http/public/)、[src/http/admin/](src/http/admin/)：分别持有两个 surface 的 handler；跨 surface 状态和
  realtime Hub 只放在 [src/http/shared/](src/http/shared/)。
- [src/http/shared/state/](src/http/shared/state/)：`public.rs` 与 `admin.rs` 分别持有两个 surface 的端口状态容器，
  `mod.rs` 只保留 `PublicApiState`/`AdminApiState` 稳定导出；状态容器不跨 surface 共用业务端口。
- [src/http/shared/boundary.rs](src/http/shared/boundary.rs)：请求 ID、trace context、可信代理、安全头、超时、panic 隔离和
  访问日志；业务 API 路由额外挂接默认纯文本拒绝的 JSON 外壳转换，400/413/500 分别使用
  `invalid_query`/`body_too_large`/`internal_error`，保留状态码和已映射 JSON，不改认证或读取正文的
  顺序；替换正文时移除旧实体描述头，HEAD 保持空正文，健康检查、metrics 和静态资源不接入。
  `server.rs`：固定请求头读取超时和连接级优雅排空；`response_cache.rs`：冻结路由的
  `no-cache`/`no-store` 响应策略；`strict_json.rs`：旧版写路由的媒体类型、body 上限和单 JSON
  值边界，并允许 Public 竞价命令在限流、Origin 和认证之后延迟解码 body；`method_strict.rs`：业务
  路由显式 HEAD/405 与 `Allow` 契约。
- [src/http/security/headers.rs](src/http/security/headers.rs)：正式表面与首次设置共享的 CSP、防嵌入、
  Referrer、MIME 与浏览器能力限制；缓存、认证和超时继续由各表面独立拥有。
- [src/http/security/](src/http/security/)、[src/http/metrics/](src/http/metrics/)：origin/Cookie/有界限流策略与进程指标表面。
- [src/http/admin/dashboard/](src/http/admin/dashboard/)：`mod.rs` 只负责认证后的 dashboard 请求编排；`sections/{mail,realtime,
  operations,credentials,alerting,fallback}.rs` 分别负责邮件、realtime、履约/投递/recovery、凭据、
  alerting 和 unknown 降级 section，`mapping.rs` 负责 health/process/worker 与稳定 Admin 响应 DTO 映射，
  不改变 JSON 字段或状态码契约。
- [src/http/admin/users/](src/http/admin/users/)：用户列表、创建/属性命令、黑名单与改名状态动作、DTO 映射和共享错误边界
  分别由 `list.rs`、`commands.rs`、`status.rs`、`mapping.rs` 与 `common.rs` 持有，`mod.rs` 只负责
  稳定处理器出口；Admin Users 的 session、CSRF、命令 ID 和版本 fence 契约保持不变。
- [src/http/admin/mail/](src/http/admin/mail/)：轮询状态、入站列表/详情、暂停/恢复命令、Worker 唤醒和错误映射分别由
  `state.rs`、`intake.rs`、`commands.rs`、`trigger.rs` 与 `common.rs` 持有，`mod.rs` 只负责稳定
  处理器出口；邮箱筛选、状态版本、CSRF 和 Worker control 契约保持不变。
- [src/http/admin/realtime/](src/http/admin/realtime/)：最近事件元数据、lag/failure 读取、replay 命令、安全投影映射和共享
  分页/错误边界分别由 `reads.rs`、`replay.rs`、`mapping.rs` 与 `common.rs` 持有，`mod.rs` 只负责
  稳定处理器出口；`GET /api/v1/admin/realtime/events` 是不含 payload 的新增只读端点，三类列表共用
  有界页大小和最大偏移量，realtime 旧版嵌套分页、CSRF、request hash 和 replay 状态契约保持不变；旧 replay wire 缺少
  `command_id` 时稳定派生，新版显式 ID 仍直接进入核心幂等账本。
- [src/http/admin/credentials/](src/http/admin/credentials/)：凭据分页读取、导入/改名/启停/删除命令、无 secret DTO 映射和共享
  CSRF/分页/错误边界分别由 `list.rs`、`commands.rs`、`mapping.rs` 与 `common.rs` 持有，`mod.rs` 只
  负责稳定处理器出口；凭据密封、版本 fence、幂等和 PostgreSQL/Lite 能力边界保持不变。
- [src/http/admin/delivery/](src/http/admin/delivery/)：交付列表、release/retry/reconcile 命令、快照映射和共享能力/错误边界
  分别由 `list.rs`、`commands.rs`、`mapping.rs` 与 `common.rs` 持有，`mod.rs` 只负责稳定处理器出口；
  动作闭集、CSRF、fencing、request hash 和外部结果未知对账边界保持不变。
- [src/http/admin/auctions/](src/http/admin/auctions/)：拍卖列表、创建命令、开拍前计划时间修改、DTO 映射和共享错误边界分别由
  `list.rs`、`commands.rs`、`schedule.rs`、`mapping.rs` 与 `common.rs` 持有，`mod.rs` 只负责稳定
  处理器出口；旧分页、能力投影、零版本创建 fence、Draft/Preview 修改边界、命令幂等和事务错误
  契约保持不变。
- [src/assets/mod.rs](src/assets/mod.rs)：前端静态产物、条件读取和 404 分派；[src/assets/routes.rs](src/assets/routes.rs)：15 + 21 条允许获得
  SPA 入口的显式浏览器路由。

## 职责边界

只做 HTTP 输入到应用命令、应用输出到稳定契约的映射。处理器**不得**包含竞价、结算或状态机
规则——那些属于领域与应用层。当前 `/api/v1/worlds`、`/api/v1/auctions`、
`/api/v1/auctions/current`、`/api/v1/auctions/{auction_id}/lots`、
`/api/v1/auctions/{auction_id}/bid-log` 和 `/api/v1/lots/{lot_id}` 已通过只读应用端口读取
权威数据库目录/投影；PostgreSQL 完整档的 `/api/v1/price-history` 已接入筛选、结算事实成交投影、
编号页统计和 keyset cursor，SQLite Lite 对同一路由稳定返回 `501 feature_not_supported`。
Public session 的登录、当前会话和注销，以及 `/api/v1/me/bids`、
`/api/v1/lots/{lot_id}/proxy-authorizations/mine` 和 `/api/v1/lots/{lot_id}/bids` 的账户读取/普通竞价
命令也已接入应用端口；普通竞价使用持久化 `command_id` 幂等账本、稳定请求摘要和数据库
事务返回结果。两条身份化账户读取先认证、再校验 query/path，且不占用写请求限流额度；公开路径与
查询中的 UUID 只接受规范小写、带连字符且非零的旧 v1 文本。账户读取会在 PostgreSQL/SQLite 快照内返回普通竞价、代理 quota、追加转换历史、
最近代理 bid 和 owner capability；两个代理 mutation 路径接入同一事务，并直接消费应用端口在事务内
冻结的完整授权、quota、转换历史、终态原因和本次代理 bid，不在提交后重新读取账户投影。Public
competition 的普通依赖失败使用旧版 503 `temporarily_unavailable`，全局请求 deadline 仍由 HTTP 边界
输出 504 `timeout`；账户只读路径继续保留自己的存储错误分类。
P09/P11 在 PostgreSQL 完整档校验 lot UUID、公开投影和内部 equipment owner 后批量解析完整装备
URL，并直接覆盖既有 `game_equipment_url` DTO 字段；两条路由固定 `no-store`，完整 key 不进入日志、
审计、事件或持久化缓存。PostgreSQL 缺密钥/依赖时返回 503；SQLite Lite 仍返回 lot 数据，但把链接
字段留空。

`GET /api/v1/forum/sync-status` 通过 `PublicForumReader` 独立读取 Worker 启用状态，响应固定
`no-store`，只接受空 query 并返回 `{enabled: bool}`。没有可靠观测时返回
`503 forum_sync_unavailable`，未装配/存储错误为 `503 storage_unavailable`，SQLite Lite 为 501。
现有 P15 镜像响应保持原字段闭集，公开页面仅在明确启用后读取并展示真实同步楼层。

## 命令结果与能力边界

Admin `/api/v1/admin/session` 已接入 Basic Auth 登录交换、独立 session/CSRF Cookie、
`X-CSRF-Token` 校验、滑动刷新、actor 会话轮换和注销；原始 token 不进入数据库，PostgreSQL/SQLite
只保存摘要。应用服务在启动时永久撤销全部旧 Admin 会话，并把 token 摘要绑定当前 actor/secret；
重启后需要重新登录，旧会话保持 401，Public 会话与 Cookie/CSRF wire 不变。Admin 的拍卖 `preview`、`open`、`cancel`、`final-phase` 和 `early-settlement` 命令也已接入
生命周期应用端口：命令账本、拍卖状态版本、追加审计记录和已注册 lifecycle realtime 事件
在同一事务内提交。人工 Final 收紧较晚的 live lot 截止时间但保留竞价和软截止；提前结算受默认关闭的
runtime capability 保护，只在 PostgreSQL 完整档要求理由并把到期事实交给既有 Worker，HTTP 不直接
写结算结果。Public `/api/v1/events` 已接入两套数据库的持久化
Catch-up，`/api/v1/ws` 通过 HTTP/1.1 GET Upgrade 或 HTTP/2 扩展 CONNECT 提供带 Public session 的只读服务端推送；
两种握手共用认证和范围门禁，其他方法返回 405；连接尝试与 mutation 独立限流，
只有 discovery 使用每进程 128 槽，单拍卖连接通过权威公开状态预检拒绝 settled。已认证会话的
发现流由 Public 外壳订阅，拍卖页面订阅对应单拍卖流；`/status` 仅显示静态说明，不建立额外订阅。
跳号或 `refetch_required` 会触发回放/权威快照恢复，后者保留恢复高水位与现有 socket。Admin 结算报告已接入 PostgreSQL 完整档的一致只读
快照，并把 lot 挂牌价、持有 PPM 与带符号调整 PPM 映射为稳定安全字段；Admin recovery 报告也已接入同一 PostgreSQL 完整档读取边界；SQLite Lite 对这两项能力
返回 `501 feature_not_supported`。A14 recovery 确认已接入 PostgreSQL 完整档的版本 fence、
证据重校验、命令账本、退款激活、guarded relist、审计和预览目录 realtime 事件；SQLite 对
该能力返回 `501 feature_not_supported`。Admin `/api/v1/admin/auction-forum/preview` 已
接入有界的本地 HTML/文本解析，输入上限为 2 MiB，解析结果只返回 catalog、posts 和 warnings，
不会抓取远程页面或写数据库。外部论坛发布/读取与回帖导入、MoogleMail 邮件及结算
履约 outbox 已由 Worker/集成 owner 接入，Sent/Balance 回执会写入履约 proof。未装配对应 owner 或 SQLite
明确不支持的 Admin 能力才返回 501，已登记且已接入的路径不再使用占位响应。
Admin `/api/v1/admin/audit` 已接入 Admin session、精确筛选、时间范围、opaque keyset cursor 和
数字分页；响应只包含追加式审计的安全字段与 JSON object metadata，不暴露原始 SQL 行。
Admin `/api/v1/admin/users`、`/api/v1/admin/renames/{rename_id}/revert` 和
`/api/v1/admin/users/{user_id}/blacklist` 已接入 Admin session、CSRF、稳定命令 DTO 和用户
安全投影；创建、改名、论坛账号 ID 修改、key 撤销/重签、黑名单/解封和改名回滚由 Users owner
负责版本 fence、命令账本与审计。`/api/v1/admin/renames` 继续提供 A29 改名历史读取。key
明文只可能出现在创建/重签的成功响应，不进入日志或错误响应；未配置用户登录数据密钥时，
该 Users port 明确报告能力不可用。
Admin A32 `/api/v1/admin/credentials` 已接入 session、origin/CSRF、`command_id`、
`expected_state_version` 和安全 DTO，支持凭据导入、改名、启停、删除及分页读取。导入请求的
secret 只在应用/加密适配器边界短暂存在，响应、命令账本、审计和管理端页面均不回显明文；该
能力要求 PostgreSQL 完整档与独立 `credential_data_key`，SQLite 或未配置密钥时明确返回
`501 feature_not_supported`。
Admin `/api/v1/admin/dashboard` 已接入 Admin session、两套数据库的 service heartbeat 安全投影
和 runtime 指定的 Worker control Unix socket 状态，返回 health/process/worker 安全字段；mail、
realtime、credential aggregation、delivery、fulfillment、recovery 和 alerting section 由对应
owner 提供安全汇总，只有 owner 未装配或读取失败时才明确返回 `unknown`，不构造占位数据。Worker
control 失败只映射为有限 condition。MoogleMail availability 对每个 world 合并其专属 scope 和
`shared` scope，避免 Worker 实际可用共享凭据时 dashboard 错误显示为缺失。

Admin `/api/v1/admin/auctions` 的 GET 已接入 Admin session、`world_id`/`status` 筛选、旧
`page`/`page_size` 分页和 PostgreSQL Serializable 生命周期/lot 数量快照；POST 创建已接入
零版本 fence、命令账本、world/intake target 锁、策略快照、审计和 `auction.created` 事件，响应显式声明已接入的
生命周期与 `manual_intake` 能力。SQLite Lite 对该
列表返回 `501 feature_not_supported`。A06 lot 列表同样只在 PostgreSQL 完整档提供安全快照。
`PATCH /api/v1/admin/auctions/{auction_id}/schedule` 已接入 Admin session/CSRF、严格 JSON、
`command_id` 和状态版本 fence；HTTP 映射计划预览、开拍、结束三项时间。Draft 可修改三项，Preview
必须原样提交已经发生的预览时间且只修改未来阶段；严格顺序、数据库时间、活动租约、幂等账本、
策略/调度同步、审计和 realtime 事件由应用与 PostgreSQL owner 完成，SQLite Lite 返回
`501 feature_not_supported`。
`GET /api/v1/admin/worlds` 读取两后端的数据库权威业务世界状态；
`PATCH /api/v1/admin/worlds/{world_id}` 使用严格 JSON、CSRF、`command_id` 和
`expected_policy_revision`，把 blocker、CAS、幂等账本和追加审计交给应用/存储 owner。SQLite
对写入返回 `501 feature_not_supported`。该开关不等同于 A26/A27 runtime 中只控制
Worker/MoogleMail 入站的 world 配置。
`GET /api/v1/admin/settlement-catalog` 只接受可选 `world_id`/`sequence` 和有界分页，返回
PostgreSQL 已结算拍卖的最小稳定目录；结算/recovery 报告仍按目录 UUID 独立读取，SQLite 返回 501。

## 实时连接与恢复

`PATCH /api/v1/admin/lots/{lot_id}` 已接入 Admin session/CSRF 和稳定命令 DTO；HTTP 层只映射
`withdraw`/`set_start_price` 与版本 fence，事务、审计、预览事件和 withdrawn-return 义务由
应用/存储 owner 完成，SQLite Lite 返回 `501 feature_not_supported`。
Admin `/api/v1/admin/rules` 已接入读取、候选校验和发布：PUT 携带 `command_id` 与
`expected_revision`，由应用层完成 revision CAS、幂等账本和审计；SQLite Lite 仅支持读取，
发布返回 `501 feature_not_supported`。管理端规则编辑页面使用同一版本化契约。

Admin A26/A27 的 runtime 文档包含可选 `operations` 分组。HTTP 仍通过应用层严格解析、校验和
指纹/命令账本端口读写；GET/validate/PUT 响应为浏览器补齐未接管的候选，原物理文件和历史
命令的指纹不因投影扩展而变化。`managed` 决定是否在重启时覆盖对应部署字段，不是热加载开关。

Admin A16-A19 邮件路由已接入：轮询状态、入站列表/详情使用严格邮箱/分页/时间筛选和
安全 DTO，暂停/恢复使用 Admin session、CSRF、`command_id` 与 `expected_state_version`。
PostgreSQL 完整档负责一致读取和 Serializable 状态命令；SQLite Lite 明确返回
`501 feature_not_supported`。A17 列表返回可空 `sender`/`subject` scalar 与 `has_message_display`；A18
详情返回同样的 `sender`/`subject` 以及受限脱敏 `message_display`（正文 64 KiB、JSON 128 KiB、附件 1000 条），
拒绝装备邮件还可返回独立 `equipment_diagnostic` 安全投影；不返回装备 key、认证邮件正文、Cookie、轮询游标或任意未脱敏 metadata。
A18 详情还返回有界 `deliveries` 数组，字段为 delivery id/operation/recipient/status/fencing_version/attempt_count/
failure_code/external_reference；仅详情新增，A17 列表不新增。查询同时匹配来源、world、MID、operation、origin_id，
A20 使用单个 `command_id` 调用 Worker control，全局唤醒已装配的 MoogleMail poller，返回带
`pollers` 的 `202 accepted`；不把唤醒动作伪装成远程读取成功。

Public 与 Admin 是不同 surface，路由只提供各自静态产物。`RustEmbed` 类型由对应装配根独占，
本 crate 通过泛型资源类型提供通用分派；默认分体制品不引用另一端资源，不把链接器裁剪当作
隔离保证。可选 `auction-server` 同时链接两个资源类型，但子进程各自只向对应 surface 提供资源，不能跨路由泄漏。
已登记路径均由对应 owner 负责真实投影或明确的 Lite 能力响应；未装配 owner、数据库档位不支持
的能力返回明确的 `feature_not_supported`/`feature_not_migrated`，不伪造旧系统成功数据。未登记
的 `/api`、`/health` 路径返回带请求 ID 的 JSON 404；未知浏览器导航按 `Accept` 返回 404 页面，
不会获得状态为 200 的 SPA 入口。两类 surface 的 HTTP 响应还会提供标准 `Date` 头，
供 Public 浏览器按旧版契约估算服务端时钟；该头不承载业务状态，也不改变任何服务端时间判断。

两个 surface 共用同一协议边界。缺失的 `X-Request-ID` 会生成 32 位十六进制值，合法入站值只允许
1 至 64 字节 ASCII 字母数字和 `-_.`；重复或非法 header 返回 400。规范化值写回响应，并注入
Public 竞价/代理、Admin 生命周期及支持审计的管理 mutation；请求 ID 不进入幂等哈希。边界提取
W3C `traceparent`/`tracestate`、记录 route/status/duration/client address、恢复 panic、设置浏览器
安全头，并对普通请求应用可配置的默认 15 秒处理超时，WebSocket 路径豁免。连接层另有固定 5 秒
HTTP/1 请求头读取超时。CSP 的脚本来源保持同源；Dioxus 解释器需要 `unsafe-eval`，WASM 编译
需要 `wasm-unsafe-eval`。样式侧允许内联是因为两个 Dioxus index 的首屏防闪样式及组件 style
属性属于当前浏览器契约。

所有 JSON mutation 都保留各路由原有字节上限，并只接受可带合法参数的
`application/json`。Public 普通竞价、代理创建和降低按旧版先执行 rate limit → Origin → authentication，
再校验 query、path、媒体类型和 body；Admin mutation 同样统一先执行 rate limit → Origin → CSRF →
session，再进入 route-specific query/path/body 校验。两种入口都拒绝
非 JSON、超限、非法或尾随第二个 JSON 值，具体 DTO 再以 `deny_unknown_fields` 拒绝未知字段；稳定映射
分别是 415 `unsupported_media_type`、413 `body_too_large` 和 400 `invalid_json`。业务 GET 不继承
Axum 的隐式 HEAD：未显式注册的方法返回
旧版 JSON 405，`Allow` 只列出该业务路径真实注册的方法。健康、metrics 与静态文件的方法边界独立
维护。无安全业务数据的错误仍输出 `data: null`，`request_id` 作为非敏感扩展保留。

`X-Forwarded-For` 默认完全忽略。TCP peer 命中 `trusted_proxy_cidrs` 后才从右向左选择第一个
非可信地址；任一条目无法解析时丢弃整条链。Public/Admin Unix listener 还必须显式启用
`trusted_unix_proxy`。登录、mutation 与 Public realtime 连接尝试限流使用解析后的地址和彼此独立的
账本，固定窗口最多保留 4096 项，容量满时
按最后访问时间和 IP 稳定淘汰最旧项。

## 静态产物内嵌

两个 surface 由 Public/Admin 装配根各自声明 `RustEmbed` 类型，本 crate 仅提供资源响应适配。
根目录 `Justfile` 的 `web-build-public` 与 `web-build-admin` 使用 `dx bundle` 先构建，再复制到
以下仓库根下的稳定目录；嵌入路径相对于对应 app 的 `CARGO_MANIFEST_DIR` 解析：

```text
target/dx/build/auction-web-public/web/public
target/dx/build/auction-web-admin/web/public
```

`allow_missing = true` 是必需的。`rust-embed` 默认在目标目录不存在时**编译期报错**，而前端
产物由独立的 `dx bundle` 生成；没有这个属性，首次克隆仓库、CI 静态检查或只改后端的构建都会
因为缺少产物而无法编译。批准的浏览器路由在产物缺失时以 503 明确报告，不 panic；未知路径仍
按 404 处理。

release 构建把稳定目录中的产物烘进二进制，dev 构建从同一目录读取，因此本地改前端后重新执行
`just web-build` 即可；不应直接把 `target/dx/<package>/<debug|release>/web/public` 当作嵌入目录。

目标请求按以下顺序封闭分派：

1. 已注册业务路由由 Axum handler 处理；未注册的 `/api`、`/health` 保留路径直接返回 JSON 404；
2. 精确静态文件只接受 GET/HEAD，其他方法返回带 `Allow: GET, HEAD` 的 JSON 405；
3. 只有 `assets/routes.rs` 中与 Dioxus `Route` 同步的显式路径才返回 `index.html`，并使用
   `Cache-Control: no-store`；
4. 其他路径不回退 SPA：浏览器 HTML 导航返回最小 404 页面，其他请求返回 JSON 404；
5. 批准的浏览器路由缺少产物时返回 503，并说明需要执行的构建命令。

缓存策略按旧版语义和 Dioxus 产物形状区分：浏览器路由入口 `no-store`；直接读取 `index.html`、
顶层 `wasm/*.js`/`*.wasm` 和其他稳定文件名使用 `no-cache` 并提供 `Last-Modified` 条件读取；只有
`assets/` 下带内容指纹的资源使用一年 `immutable`。Public 非身份化读接口统一 `no-cache`，Public
forum/sync-status、会话/身份化/变更接口和全部 Admin 业务接口统一 `no-store`，包括错误响应。

## 维护与验证

修改本模块时同步对应[契约](../../docs/contracts/README.md)与源码入口；跨模块理由按[决策记录规则](../../docs/documentation.md)维护。

```bash
cargo check --locked -p auction-http
```

该命令只做静态编译；完整门禁见[开发指南](../../docs/development.md)，运行证据边界见[验证状态](../../docs/status.md)。
