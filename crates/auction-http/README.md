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

P09 接收 `equipment_group=rare|fusion`、可选 `max_level` 与默认 true 的 `include_unleveled`，严格解析后交给应用读取端口，
在分页前与类型组合筛选；P09/P11 响应 DTO 保持不变，边界检查返回行与请求分组及等级一致。具体字段及统计约定见
[装备分组契约](docs/contracts/routes.md#p09p11-装备分组)。

[HTTP 契约](docs/contracts/routes.md)是路由、状态码、DTO、分页和能力清单；Public 指定列表的 `page_size` 不设 200 条上限，仍严格检查正整数、
offset 溢出及 cursor 互斥；[assets/routes.rs](crates/auction-http/src/assets/routes.rs)
与 Dioxus Route 共同限定 15 条 Public、21 条正式 Admin 浏览器路径，修改任一方必须同步其余两处。
P12 成交价编号页的 `sort` 仅接受 `closed_desc`（默认）、`closed_asc`、`price_desc`、`price_asc`；
名称同时接受 Unicode 空白及英文逗号分词，空词忽略，原有总长 1024 字节、最多八词及每词 128 字节边界保留；
旧 cursor 仅配默认排序，其他组合返回 400。可选 `stats.median_exact` 保留精确半个 Credit，原整数 `median` 继续兼容。
`http/mod.rs` 与各 `mod.rs` 只做装配/导出，`shared/state/{public,admin}.rs` 分别持有两个表面的窄端口、不能互用。

两个 state 的端口与安全策略保持各自所有权。已校验的代理网段、Admin dashboard world/邮箱配置以只读 `Arc` 切片
共享；请求提取 state 不再深复制这些列表。请求 span 在交出 Request 前借用路径和 trace 头，响应关联 ID 也不另建副本。

[public/countdown](crates/auction-http/src/http/public/countdown) 拥有论坛动态 SVG 的 HTTP、固定英文视图及有界 CSS 动画，
复用公开时间窄端口，无 JS、外链资源或会话依赖（见[图片契约](docs/contracts/routes.md#p22-自托管论坛倒计时)）。
计时数字默认可见；减少动态效果仍逐秒离散换字，不发生滑动或闪烁。动画不可用时保留加载时的数字快照和 UTC 截止。
[routes/public/watch.rs](crates/auction-http/src/http/routes/public/watch.rs) 拥有 P23/P24 装备标记列表与 CAS 写入，
[routes/public/watch_preference.rs](crates/auction-http/src/http/routes/public/watch_preference.rs) 拥有 P25/P26 账号渠道
GET/PUT：两者复用会话、来源/CSRF、限流、strict JSON、JSON 405 与 `no-store`，不接收客户端 user_id；
词法错误 400、业务错误 422、命令/版本冲突 409、依赖缺失或集合超限 503，旧 channel 只接受 mm 占位。
[routes/public/session_preference.rs](crates/auction-http/src/http/routes/public/session_preference.rs) 拥有 P27/P28 登录时长 GET/PUT，
复用账号身份、Origin、限流、严格 JSON 与 `no-store`，不在 HTTP 层计算固定期限。P06 的截止字段支持成对 `null`；
普通 GET session 不写入或清除 Cookie；P06 新增空 body、无 query 的 PUT，仅按权威 `inspect` 同步浏览器持久期。
登录、退出、时长保存/重放及显式同步由浏览器同源原生锁串行化；重放保存仍另读当前期限，不延长数据库授权期。
完整方法、错误、400 天上限及同版页面要求见[会话同步契约](docs/contracts/routes.md#p06-会话读取与-cookie-同步)。
[public/registration.rs](crates/auction-http/src/http/public/registration.rs) 拥有 P29 匿名注册说明，仅读取装配根注入的 bot 名单快照，
不读取配置文件、数据库或游戏凭据。响应直接序列化共享安全 DTO，不逐请求复制名单；方法、缓存与空名单边界见
[注册提示契约](docs/contracts/routes.md#p29-注册账号提示)。
[routes/reads/auctions.rs](crates/auction-http/src/http/routes/reads/auctions.rs) 共同拥有 P07 v1 与 P30 v2；筛选、分页、
游标和错误处理复用同一路径，v2 按请求选择论坛地址投影并映射独立条目 DTO，v1 保留原字段闭集。
查询次数和页面升级顺序见[v2 拍卖历史契约](docs/contracts/routes.md#p30-v2-拍卖历史与论坛地址)。
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

[admin/mail/diagnostics.rs](crates/auction-http/src/http/admin/mail/diagnostics.rs) 仅在 A16 的
`include_diagnostics=true` 查询下组合安全已完成 MID 与可选 Worker 观测。旧查询及暂停/恢复响应保持原字段集合，
原始/非数字游标不出服务端；Worker 不可用不抹掉数据库侧安全游标，不增加业务写入。

[admin/credentials](crates/auction-http/src/http/admin/credentials) 将 A32 的互斥 Cookie/论坛账号输入映射为应用来源，
复用会话、Origin/CSRF、严格 JSON 与 `no-store`。远端账号拒绝/挑战使用 422，不触发 Admin 401 退出；
获取结果不回传明文，原导入事务、单 active 约束和 Worker 重启边界不变，详见[外部凭据](docs/contracts/routes.md#外部凭据)。

## 请求边界与能力边界

请求边界统一处理合法 `X-Request-ID`、trace context、安全头、访问日志、panic 与超时；普通请求和 WebSocket
握手共用 15 秒处理预算，升级后的长连接不受该计时器限制。连接层 HTTP/1 请求头预算固定 5 秒；默认忽略 `X-Forwarded-For`，仅显式信任的 TCP/Unix 代理
可提供客户端地址；Unix listener 不伪造 TCP peer，应用不根据转发协议头推导 Origin 或 Secure Cookie
（见[橙云部署](ops/README.md#cloudflare-整站代理)）。Public 竞价守卫按限流 → Origin → 认证执行，
Admin mutation 守卫按限流 → Origin → CSRF → 会话执行。竞价、标记、警告渠道、生命周期和世界启停通过
`decode_after_guard` 在守卫及 query/path 校验后读取正文；其余已挂载 `strict_json::enforce` 的旧路由仍先经过
正文中间件，再进入 handler 守卫。不能混写成全站统一的正文顺序，也不能为减少解析次数改变原有错误优先级。
两条严格 JSON 路径均保留路由大小上限、单值与 DTO 字段闭集。
守卫后的正文读取直接复用未超限的单个非空 `Bytes` 帧，多帧才合并；不再为小请求预分配整条路由上限，
也不复制仅用于确认超限的额外字节。等于上限时仍继续观察 EOF 或下一非空帧，流错误及既有 400/413 优先级不变。
TCP peer 直接注入每次请求的扩展，不为每条连接重新给整张路由表装配 middleware；
Unix 连接仍不注入伪造地址，原可信代理解析、5 秒请求头预算及连接排空边界不变。
可信 XFF 倒序借用头和地址段选择客户端，不建立整链 Vec；找到候选后仍校验完整左侧链，
任一非法段整体回退，全部受信时仍取最左地址。未消费的 Axum `OriginalUri` 扩展不再启用。
限流窗口与最近访问时间在取得互斥锁后读取单调时钟，按实际账本更新顺序维护淘汰次序。
Public 规范 UUID 在栈缓冲中格式化后比较；实时目标复用同一非零、小写和规范形式校验。
Public 竞价、代理、标记、警告渠道和注销在 Cookie 认证后核对可选 `x-auction-expected-user-id`；规范非零 UUID
与当前身份不符返回 409 `session_changed`，无效或重复头返回 400 `invalid_expected_user_id`。省略该头保留旧 v1，读取路由行为不变。
P25/P26 共用路由的方法拒绝响应始终声明 `Allow: GET, PUT`；HEAD 仍返回 405，不能由单一 GET builder
提前固定为只有 GET 的方法清单。
Admin 代理历史的旧二进制游标固定为 26 字节：除 `start` 外，先检查 35 字符无填充长度，再解码到
26 字节栈缓冲；过长查询不再先分配解码 Vec，版本、种类、时间及 UUID 校验和 `invalid_cursor` 外壳不变。
错误优先级、405/Allow 和缓存要求见[通用响应与错误](docs/contracts/routes.md#通用响应与错误)。
Admin 结算报告按原 `adjustment.policy` 区分冻结的 v0/v1/v2，金额矩阵交领域校验，不在 HTTP 重算政策。
v2 的 `compensation_amount/compensation_out` 仅为按奖池预算缩放后的基础优惠，逐项和汇总均不超过基础优惠需求；路径和字段不变。
Admin 结算报告读取或不变量校验失败时记录 `admin_settlement_report_failed`，字段限定为 `auction_id` 与
闭集 `failure_code`，通过请求 span 关联请求 ID；底层错误文本、SQL 和敏感值不进入该诊断事件。

Unix listener 的组名解析使用 `getgrnam_r` 和调用内独占缓冲，避免并发启动覆盖 libc 静态组记录；
数字 GID 不分配缓冲。命名查询从 1 KiB 起步，仅在 `ERANGE` 时倍增，最多 1 MiB，未知组或超限失败关闭。
该上限约束解析缓冲，不声称限制 NSS 后端的同步查询耗时；socket 目录链、owner/group、锁和清理身份约束不变。
单实例锁先核验目标为当前账号拥有的单链接普通文件，再设置权限；拒绝硬链接时不修改其目标文件。

- 写命令使用稳定 `command_id`、摘要与版本门禁；成功竞价/代理响应直接使用事务内冻结结果，不在提交后重读权限。
- 按已登记能力返回安全结果或明确不支持，不以空列表和默认值伪造成功。
- P09/P11/P14 的完整装备 URL 由受限 DTO 暴露，响应固定 `no-store`；P15 论坛镜像正文也可能包含完整装备 URL，成功和错误响应同样固定 `no-store`。缓存分组不能放宽 handler 已声明的 `no-store`。
- Admin 凭据、用户 key、邮件诊断和 dashboard 严格投影安全字段，登录 key 只出现在创建/重签成功响应。
- Admin dashboard 凭据快照先做完整值对象校验；损坏时该 section 使用 `unknown/source_unavailable`，其余 section 不受影响。
  健康状态直接判断是否存在 active/invalid，避免跨 scope 计数求和溢出；世界投影直接借用配置或默认数组。
- Runtime 保存与 Worker 唤醒接受结果不表示配置已热加载或远端任务完成。
- 响应映射接管已持有的数据：dashboard recovery 不复制完整快照，代理摘要只为 wire 要求的嵌套 bid 副本复制文本，
  结算报告的批次去重借用分类文本。旧 realtime replay 的派生 UUID 直接在原命名空间前缀后序列化 JSON，
  不暂存第二份序列化缓冲区；命名空间、字段顺序和幂等输入不变。

Public 装备目录、详情和补偿流水先校验本次读取投影，再把旧链接转移到解析目标，避免逐行复制后丢弃。
只有解析成功才映射响应；列表先核对链接数量，再逐项配对，不生成缺失链接的默认空字符串。
P11 正式详情在完整投影校验后直接接管装备属性 JSON，再映射到原有 `attributes` 字段；P09 列表仍不附带该字段。
分页请求将解码后的 cursor 直接交给读取端口，不保留多余副本。
不修改共享缓存、解析端口、URL 保密边界或 `no-store`；敏感分组直接写入禁止缓存策略，不再先扫描将被覆盖的响应头。

## 实时连接与恢复

[public/realtime](crates/auction-http/src/http/public/realtime) 拥有 Catch-up、握手、事件校验、连接生命期和只读会话复核；
WebSocket 支持 HTTP/1.1 Upgrade 与 HTTP/2 扩展 CONNECT，只有服务端通知，不承载竞价命令。握手先检查会话与公开范围，
单拍卖范围拒绝 settled，配额许可随连接 RAII 释放。Public 进程通过持久化 relay 向有界本地 Hub 分发；
跳号、重连或 `refetch_required` 按[实时契约](docs/contracts/events.md)执行 Catch-up/权威快照恢复，
慢连接可被淘汰，不把本地 Hub 或事件 payload 当成权威状态。

Hub 不创建后台任务。进程总量 1024、每用户 8、发现流额外 128 槽以及每连接 1–1024 条缓冲的边界不变。
广播先非阻塞预留队列槽位，再复制事件，满/闭队列原位淘汰，不暂存待删除 ID；配额仍在订阅真正结束时释放。
每条连接继续独立检查事件范围与注册表，保持协议错误、慢消费者、会话失效及进程关闭的既有关闭码。
事件校验、DTO 映射和序列化不另建共享缓存，持久化事件与会话端口仍是唯一权威。
会话、停机和存活期限优先于数据；同时就绪的客户端帧与订阅事件由内层公平选择，避免单侧持续就绪饿死另一侧。
连接保持握手身份且只读复核不续期；账号修改权威期限后，下次复核会同步下一次截止检查时间。
Public drain 先拒绝新订阅并通知既有 1001 关闭，再等待升级任务持有的 RAII 许可归零；
普通流、发现流及 pending upgrade 共用此计数，Hyper 排空不等于 WebSocket 排空。
等待服从装配根原有总预算，最后许可释放时通知，不增加后台轮询或独立超时。

Public 四类固定二进制游标按精确编码长度直接解码到栈，保留严格字母表、无填充和规范尾位校验。
价格历史名称最多保留九个非空词以判定八词上限，不继续为已确定非法的后缀分配字符串。

## 静态产物内嵌

Public/Admin 装配根分别持有 `RustEmbed` 类型，仅显式 `web-embed` 制品选择对应资源，API-only 使用固定空集合。
`bash ci/release/web-bundle.sh public|admin` 通过 `dx bundle` 生成并复制到 `target/dx/build/auction-web-public/web/public`
与 `target/dx/build/auction-web-admin/web/public`。保留 `allow_missing = true`：无前端产物仍可编译，已登记浏览器路径
返回 503，未知路径返回 404；默认分体制品不包含另一端资源，合并二进制的子进程也只提供所属表面。

分派顺序为业务路由、精确静态文件、显式浏览器路由、404，未登记 `/api`/`/health` 不回退 SPA。静态文件只接受
GET/HEAD，浏览器入口与直接 HTML `no-store`，稳定文件名（含 favicon 别名）`no-cache`，仅非 HTML 内容指纹资源使用
一年 `immutable`；正式边界默认给全部响应双 CDN `no-store` 头，只有静态 handler 私有标记可启用资源 CDN 策略。
条件读取与各类 API 缓存见[静态契约](docs/contracts/routes.md#静态入口与缓存)。
精确文件查找只接受单一前导斜线及非空普通路径段，拒绝 `.`、`..`、反斜线和重复斜线；不做百分号解码。
这样不会把开发态 RustEmbed 的路径规范化当作 HTTP 授权，也不会按原始 `/assets/` 前缀给目录外文件签发缓存许可。
校验只封闭请求路径穿越，不阻断可信资源目录内显式声明的符号链接；正式发布 payload 的禁 symlink 门禁仍须保留，
开发态资源目录也必须由可信构建流程提供。批准的 SPA 路由、精确正常文件和 favicon 别名不变。

## 维护与验证

```bash
cargo check --locked -p auction-http
```

仅静态编译；默认检查与测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
字段、路由或部署行为变化时同步对应契约和配置模板。
