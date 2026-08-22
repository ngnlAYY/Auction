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

旧 HTTP 层使用统一响应外壳：

    {"code": "...", "message": "...", "data": ...}

成功响应的业务 code、JSON 字段和错误 code 以旧 handler 为准。下表的错误状态码是该
handler 当前明确产生的稳定集合；依赖错误中的 504 表示 context deadline，503 表示
其他依赖不可用。路由器对未注册方法还可能返回 404/405。

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
| P12 | GET | /api/v1/price-history | 公开 | 200 | 400, 503/504 | features/auction/publichttp/adapter.go:63 |
| P13 | GET | /api/v1/compensation-pool | 公开 | 200 | 400, 501, 503/504 | features/auction/publichttp/adapter.go:64 |
| P14 | GET | /api/v1/compensation-pool/{auction_id}/movements | 公开 | 200 | 400, 404, 501, 503/504 | features/auction/publichttp/adapter.go:65 |
| P15 | GET | /api/v1/auctions/{auction_id}/forum | 公开 | 200 | 400, 404, 501, 503/504 | features/auction/publichttp/forum.go:37 |
| P16 | GET | /api/v1/me/bids | Public session | 200 | 400, 401, 503/504 | features/auction/publichttp/account.go:90 |
| P17 | GET | /api/v1/lots/{lot_id}/proxy-authorizations/mine | Public session | 200 | 400, 401, 404, 503/504 | features/auction/publichttp/account.go:97 |
| P18 | POST | /api/v1/lots/{lot_id}/proxy-authorizations | Public session、origin、限流 | 200 | 400, 401, 403, 409, 413/415, 429, 503/504 | features/auction/publichttp/account.go:104 |
| P19 | POST | /api/v1/lots/{lot_id}/proxy-authorizations/{authorization_id}/lower | Public session、origin、限流 | 200 | 400, 401, 403, 409, 413/415, 429, 503/504 | features/auction/publichttp/account.go:111 |
| P20 | POST | /api/v1/lots/{lot_id}/bids | Public session、origin、限流 | 200 | 400, 401, 403, 409, 413/415, 429, 503/504 | features/auction/publichttp/account.go:118 |

Public 分页必须保留旧的 page/page_size 与 cursor 语义。P10 竞价日志额外保留
`world` 与 `lot` 筛选；其条目按 `created_at`、`lot_id`、`accepted_state_version`、
`fact_index`、`id` 降序排列，cursor 是绑定这五个字段的版本化 opaque 值。带 cursor 的返回使用
next_cursor/has_more，不得把未知总数伪装成精确 total。P12 价格历史保留 `name`、`world`、
日期、价格、buyer/seller 筛选，编号页附带完整过滤集 `stats`，cursor 页不返回统计。实时事件的 after 也是
非负 stream sequence，不得改成时间戳游标。P03 当前接受 `auction_id`、`after`、`page_size`，
页面大小上限为 200；响应返回 `events`、`next_after`、`has_more` 和 `refetch_required`。

## Admin

除 Admin session 本身外，下表路径都经过 Admin session；mutation 还经过 origin、CSRF、
限流和 actor/request_id 注入。具有多个 HTTP 方法的路径必须按方法分别保留成功状态。

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
| A25 | GET/POST/PUT | /api/v1/admin/rules | GET/POST/PUT 200；创建时 201 | 400, 401, 403, 404, 409, 413/415, 501, 503/504 | features/rules/adminhttp/adapter.go:91 |
| A26 | GET/PUT | /api/v1/admin/config | GET/PUT 200；Lite 501 | 400, 401, 403, 409, 413/415, 501, 503/504 | features/adminconfig/adminhttp/adapter.go:74 |
| A27 | POST | /api/v1/admin/config:validate | 200；Lite 501 | 400, 401, 403, 413/415, 501, 503/504 | features/adminconfig/adminhttp/adapter.go:87 |
| A28 | GET/POST/PATCH | /api/v1/admin/users | GET 200；POST 201；PATCH 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/users/adminhttp/adapter.go:105 |
| A29 | GET | /api/v1/admin/renames | 200 | 400, 401, 503/504 | features/users/adminhttp/adapter.go:112 |
| A30 | POST | /api/v1/admin/renames/{rename_id}/revert | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/users/adminhttp/adapter.go:117 |
| A31 | PUT/DELETE | /api/v1/admin/users/{user_id}/blacklist | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/users/adminhttp/adapter.go:122 |
| A32 | GET/POST/PATCH/DELETE | /api/v1/admin/credentials | GET 200；POST 201；PATCH/DELETE 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/keymanagement/adminhttp/adapter.go:95 |
| A33 | GET | /api/v1/admin/audit | 200 | 400, 401, 503/504 | features/audit/adminhttp/adapter.go:62 |
| A34 | GET | /api/v1/admin/deliveries | 200 | 400, 401, 503/504 | features/delivery/adminhttp/adapter.go:78 |
| A35 | POST | /api/v1/admin/deliveries/{delivery_id}/release | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/delivery/adminhttp/adapter.go:83 |
| A36 | POST | /api/v1/admin/deliveries/{delivery_id}/retry | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/delivery/adminhttp/adapter.go:89 |
| A37 | POST | /api/v1/admin/deliveries/{delivery_id}/reconcile | 200 | 400, 401, 403, 404, 409, 413/415, 503/504 | features/delivery/adminhttp/adapter.go:95 |

## 迁移冻结规则

1. Rust 路由必须先在本表登记，再实现 handler；迁移期间不得为了方便重命名旧路径。
2. DTO 必须在 auction-contracts 中重新定义；不得把旧 SQL 行或 Rust storage model 直接暴露给浏览器。
3. 所有改变状态的请求必须保留 command_id；同一 ID 搭配不同 request hash 必须返回 409。
4. 公开实时更新不以 HTTP 分页替代 WebSocket；无法保持长连接时只允许按 events catch-up
   语义退化为轮询或 SSE。
5. 本表的状态码、JSON 字段和错误 code 变化必须同时更新 migration plan、契约变更记录和
   对应 owner 的边界说明。

截至当前迁移切片，P01、P02、P05、P06、P07、P08、P09、P10、P11、P12、P13、P14、P15、P16、P17、P18、P19、P20、A03 GET/POST、A04、A06、A15、A25 GET/POST/PUT 已接入应用端口；P02 保留 `lang` 语言选择和缺失语言回退英文，P05 保留公开安全投影和页码分页；P07 保留
`page`/`page_size`/`total` 分页和 `cursor`/`next_cursor`/`has_more` keyset 分页，P09
保留旧的 preview/live 互斥 lot 数组、分类统计和页码字段，P11 保留当前公开 projection
下的 lot 详情边界，P10 保留 world/lot 筛选、编号页与 keyset cursor 两种分页形状以及
公开 bid fact 字段，P12 保留成交条目、筛选条件和编号页统计；P13/P14 保留补偿池批次、余额守恒摘要、opaque keyset cursor 和单场账本分页，PostgreSQL 完整档从同一 Serializable 快照读取，SQLite Lite 明确返回 `501 feature_not_supported`；P15 读取本地论坛同步镜像，未把远程网关状态伪造成成功，SQLite Lite 同样明确返回 `501 feature_not_supported`。P06 的 Public session 已接入
配置、应用端口和 PostgreSQL/SQLite 存储；P16/P17 已接入 Public session、账户读取端口以及
PostgreSQL/SQLite 的普通竞价、代理 quota、转换历史、最近 proxy bid 和 owner capability 投影；P20 的普通竞价已接入 Public session、稳定
`command_id`/`request_hash`、应用竞价事务和旧版竞价结果投影。
Admin 的 `/api/v1/admin/session` 已接入 Basic Auth、CSRF 和持久化会话端口。A03 GET
`/api/v1/admin/auctions` 已接入 Admin 会话保护、`world_id`/`status` 筛选、有限页码和
PostgreSQL Serializable 生命周期/lot 数量安全投影；创建拍卖 POST 已接入零版本 fence、命令账本、world/intake target 锁、策略快照、审计和 `auction.created` 事件；SQLite Lite 的创建与列表 GET 均明确返回 `501 feature_not_supported`。A06
`/api/v1/admin/auctions/{auction_id}/lots` 已接入 Admin 会话保护、有限页码、PostgreSQL
Serializable 同快照的拍卖/lot/装备/投稿安全投影；SQLite Lite 因缺少完整 Admin 生命周期字段
明确返回 `501 feature_not_supported`，不填充伪造的默认生命周期数据。A04
`/api/v1/admin/worlds/{world_id}/auction-intake/items` 已接入 URL/canonical JSON 录入、
固定 HTTPS 来源边界、key 密文信封和 PostgreSQL 同一 Serializable 写事务；成功响应只返回
拍卖、投稿、lot 与装备的安全事实，不返回原始 URL、canonical 正文或 key，SQLite Lite 明确
返回 `501 feature_not_supported`。A15
`/api/v1/admin/lots/{lot_id}/proxy-events` 已接入 Admin 会话保护、页码/opaque cursor 两种分页、
PostgreSQL Serializable 命令账本快照和代理安全事件投影；SQLite Lite 明确返回
`501 feature_not_supported`，管理端 lot 页面已提供历史读取入口。A07
`/api/v1/admin/auctions/{auction_id}/settlement-report` 已接入 PostgreSQL 完整档的一致
只读报告；A08 recovery-report 也已接入 PostgreSQL 完整档的一致安全投影；SQLite Lite 对两项
能力明确返回 `501 feature_not_supported`，不伪造空报告。P18/P19 已接入同一竞价事务、稳定
命令账本、代理状态校验和旧版 `{ authorization: ... }` 完整命令投影；quota、转换历史和
终止原因从提交后的安全账户快照补齐，并在授权状态版本不一致时失败关闭，不暴露原始账本
payload。A09-A12 的
Admin 拍卖生命周期命令已接入应用端口，并在同一数据库事务内追加命令账本、状态版本、审计记录和
注册表内的 lifecycle realtime 事件。A34-A37 delivery Admin 列表、release、retry 和
reconcile 已接入 PostgreSQL 的安全投影、fencing version 和独立命令账本；SQLite 明确返回
`501 feature_not_supported`。这些动作只推进本地 outbox/履约状态，不会伪造外部 MoogleMail
回执；外部履约传输与其余 Admin 查询/命令仍待后续阶段接入。
A14 recovery 确认已接入 PostgreSQL 完整档的版本 fence、证据重校验、命令账本、退款激活、guarded relist、
审计和预览目录 realtime 事件；SQLite Lite 明确返回 `501 feature_not_supported`。A14 仍只覆盖已有
recovery 记录的确认，recovery 生成、Worker 履约和其余路径同样不代表业务能力已经完成。
A21-A24 realtime 运维面已接入 PostgreSQL/SQLite 的安全 lag/failure 投影和 replay 事务；A24
使用 `command_id`、CSRF、命令摘要和审计，重试同一命令返回同一 replay 事件。没有数据库端口的
骨架制品明确返回 `501 feature_not_supported`；这组接口仍只处理本地 realtime 失败边界，外部
广播 sink 未冻结时不会伪造远程投递成功。A25 Admin 规则 GET、候选 POST 校验和 PUT 发布已接入
规则 owner；PUT 保留 `command_id`、`expected_revision`、命令摘要、Serializable revision CAS、
幂等账本和审计，SQLite Lite 只提供 GET，发布明确返回 `501 feature_not_supported`。

A05 `/api/v1/admin/auction-forum/preview` 已接入 Admin session、origin/CSRF 边界和 2 MiB
请求体上限；请求必须在 `html` 与 `text` 中恰好选择一个非空输入，由本地论坛解析器返回
catalog、posts 和 warnings。该接口只做内存预览，不抓取远程页面、不发布回帖、不写数据库；
输入无效或解析失败返回 400，未声明的远程发布仍属于后续 Worker/外部适配阶段。
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
`service_heartbeats` 安全读取，health/process section 返回真实心跳聚合；Worker control、
mail aggregation、realtime、credentials、delivery、fulfillment、recovery、alerting section 在对应 owner
尚未迁移前统一返回 `unknown/source_unavailable`，不伪造可用状态。三个进程装配根已接入
启动/就绪/周期/停止心跳发布；当前接口仍不代表完整 dashboard 行为已完成。

A16-A19 Admin 邮件首片已接入 PostgreSQL 完整档：轮询状态和入站审查读取使用邮箱边界、
严格查询校验与安全 DTO，列表读取保持同一读取快照；暂停/恢复使用 Admin session/CSRF、
`command_id` 和 state-version fence，在 Serializable 事务内写入 `mail_poll_state`、
`mail_admin_commands` 与追加审计。SQLite Lite 对这组能力明确返回 `501 feature_not_supported`。
A20 远程轮询触发已返回 `202`：请求经过 Admin session/CSRF、`command_id` 和
`expected_state_version` 校验后，PostgreSQL 事务只幂等地提前 `next_poll_at`，由 Worker 后续
执行实际 MoogleMail GET；该响应表示已排队，不表示外部读取已经完成。SQLite Lite 明确返回
`501 feature_not_supported`。

A26/A27 Admin runtime 配置已接入 PostgreSQL 完整档：GET 返回非敏感 TOML 文档的 JSON 投影
和稳定 `sha256:` fingerprint，POST `:validate` 只执行严格候选校验，PUT 使用
`command_id` 与 fingerprint fence 写入 durable pending/applied/unknown 命令账本，并通过
同目录临时文件、权限校验、fsync、rename 和目录同步完成原子替换。文件替换与账本提交
之间出现不确定结果时会优先恢复旧文件，无法证明恢复成功则锁存 `unknown` 并返回
`reconciliation_required`；成功更新明确返回 `restart_required`，不伪造热加载。Admin
运行时文件路径由主 TOML 的 `runtime_config_file` 选择，敏感 TOML、Cookie、密钥和数据库
连接串不进入该文档。SQLite Lite 不拥有这项完整命令账本/原子更新能力，相关接口返回
 `501 feature_not_supported`。

A29 `GET /api/v1/admin/renames` 已接入 Admin session、`user_id`/`name` 筛选、有限数字分页和
PostgreSQL/SQLite 同一读取事务内的追加式改名历史安全投影；响应只包含记录 ID、用户 ID、前后
用户名、actor 和创建/回滚时间，不返回命令摘要。A30 改名回滚仍保持 `feature_not_migrated`，
待用户状态版本 fence、命令账本和名称占用冲突规则一并迁移。
