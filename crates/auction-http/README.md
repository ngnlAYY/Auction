# auction-http

Public 与 Admin 的 HTTP 表面适配器。

- `src/http/mod.rs`：`public_router()` / `admin_router()`，健康检查和冻结契约路径注册。
- `src/assets/mod.rs`：前端静态产物内嵌与 SPA fallback。

## 职责边界

只做 HTTP 输入到应用命令、应用输出到稳定契约的映射。处理器**不得**包含竞价、结算或状态机
规则——那些属于领域与应用层。当前 `/api/v1/worlds`、`/api/v1/auctions`、
`/api/v1/auctions/current`、`/api/v1/auctions/{auction_id}/lots`、
`/api/v1/auctions/{auction_id}/bid-log` 和 `/api/v1/lots/{lot_id}` 已通过只读应用端口读取
权威数据库目录/投影；`/api/v1/price-history` 也已接入筛选、成交投影、编号页统计和 keyset
cursor。Public session 的登录、当前会话和注销，以及 `/api/v1/me/bids`、
`/api/v1/lots/{lot_id}/proxy-authorizations/mine` 和 `/api/v1/lots/{lot_id}/bids` 的账户读取/普通竞价
命令也已接入应用端口；普通竞价使用持久化 `command_id` 幂等账本、稳定请求摘要和数据库
事务返回结果。账户读取会在 PostgreSQL/SQLite 快照内返回普通竞价、代理 quota、追加转换历史、
最近代理 bid 和 owner capability；两个代理 mutation 路径仍接入同一事务并返回命令确认，
其完整旧版命令结果投影尚待后续补齐。Public 启动时未提供数据库状态则返回明确的
`storage_unavailable`。

Admin `/api/v1/admin/session` 已接入 Basic Auth 登录交换、独立 session/CSRF Cookie、
`X-CSRF-Token` 校验、滑动刷新、actor 会话轮换和注销；原始 token 不进入数据库，PostgreSQL/SQLite
只保存摘要。Admin 的拍卖 `preview`、`open`、`cancel` 和 `final-phase` 命令也已接入
生命周期应用端口：命令账本、拍卖状态版本、追加审计记录和已注册 lifecycle realtime 事件
在同一 PostgreSQL/SQLite 事务内提交。Public `/api/v1/events` 已接入两套数据库的持久化
Catch-up，`/api/v1/ws` 提供带 Public session 和并发上限的只读服务端推送；Public `/status` 页面已在已认证会话下自动订阅该流，收到
跳号或 `refetch_required` 后停止并回到权威快照边界。Admin 结算报告已接入 PostgreSQL 完整档的一致只读
快照；Admin recovery 报告也已接入同一 PostgreSQL 完整档读取边界；SQLite Lite 对这两项能力
返回 `501 feature_not_supported`。A14 recovery 确认已接入 PostgreSQL 完整档的版本 fence、
证据重校验、命令账本、退款激活、guarded relist、审计和预览目录 realtime 事件；SQLite 对
该能力返回 `501 feature_not_supported`。Admin `/api/v1/admin/auction-forum/preview` 已
接入有界的本地 HTML/文本解析，输入上限为 2 MiB，解析结果只返回 catalog、posts 和 warnings，
不会抓取远程页面或写数据库。外部论坛发布、邮件和履约 outbox 仍属于后续 Worker/应用阶段，
其余 Admin 业务路径仍在应用端口接入前返回 501。
Admin `/api/v1/admin/audit` 已接入 Admin session、精确筛选、时间范围、opaque keyset cursor 和
数字分页；响应只包含追加式审计的安全字段与 JSON object metadata，不暴露原始 SQL 行。
Admin `/api/v1/admin/renames` 已接入 Admin session、`user_id`/`name` 筛选、有限数字分页和
两套数据库的改名历史安全投影；A30 回滚命令仍保持 `feature_not_migrated`，不在 HTTP 层伪造
用户状态写入。
Admin `/api/v1/admin/dashboard` 已接入 Admin session 和两套数据库的 service heartbeat
安全投影，返回 health/process section；尚未迁移的 mail aggregation、realtime、credentials、delivery、
fulfillment、recovery 和 alerting section 明确返回 `unknown`，不构造占位数据。

Admin `/api/v1/admin/auctions` 的 GET 已接入 Admin session、`world_id`/`status` 筛选、旧
`page`/`page_size` 分页和 PostgreSQL Serializable 生命周期/lot 数量快照；POST 创建已接入
零版本 fence、命令账本、world/intake target 锁、策略快照、审计和 `auction.created` 事件，响应显式声明已接入的
生命周期能力，intake 仍保持未迁移。SQLite Lite 对该
列表返回 `501 feature_not_supported`。A06 lot 列表同样只在 PostgreSQL 完整档提供安全快照。
`PATCH /api/v1/admin/lots/{lot_id}` 已接入 Admin session/CSRF 和稳定命令 DTO；HTTP 层只映射
`withdraw`/`set_start_price` 与版本 fence，事务、审计、预览事件和 withdrawn-return 义务由
应用/存储 owner 完成，SQLite Lite 返回 `501 feature_not_supported`。
Admin `/api/v1/admin/rules` 已接入读取、候选校验和发布：PUT 携带 `command_id` 与
`expected_revision`，由应用层完成 revision CAS、幂等账本和审计；SQLite Lite 仅支持读取，
发布返回 `501 feature_not_supported`。管理端规则编辑页面使用同一版本化契约。

Admin A16-A19 邮件路由已接入：轮询状态、入站列表/详情使用严格邮箱/分页/时间筛选和
安全 DTO，暂停/恢复使用 Admin session、CSRF、`command_id` 与 `expected_state_version`。
PostgreSQL 完整档负责一致读取和 Serializable 状态命令；SQLite Lite 明确返回
`501 feature_not_supported`。HTTP 层不返回原始邮件正文、Cookie、轮询游标或任意 metadata，
A20 只把带版本 fence 的触发命令排入 Worker，返回 `202`，不把排队动作伪装成远程读取成功。

Public 与 Admin 是不同 surface，路由和静态产物分别隔离；公开端二进制不包含管理端资源。
除已接入的世界、拍卖/lot/竞价日志/价格历史只读目录、Public session、Admin session、普通竞价和代理 mutation 命令外，当前已登记的业务路径在应用用例
接入前返回明确的 501 和 `feature_not_migrated`，不伪造旧系统成功数据，也不把未登记路径
误当成迁移完成。

## 静态产物内嵌

两个 surface 各自内嵌对应的 `dx build` 产物，路径由 `rust-embed` 按本 crate 的
`CARGO_MANIFEST_DIR` 解析：

```text
target/dx/build/auction-web-public/web/public
target/dx/build/auction-web-admin/web/public
```

`allow_missing = true` 是必需的。`rust-embed` 默认在目标目录不存在时**编译期报错**，而前端
产物由独立的 `dx build` 生成；没有这个属性，首次克隆仓库、CI 静态检查或只改后端的构建都会
因为缺少产物而无法编译。缺失产物在运行时以 503 明确报告，不 panic。

release 构建把产物烘进二进制，dev 构建从磁盘读取，因此本地改前端不必重编后端。

请求命中顺序：

1. 精确匹配静态文件；
2. 无扩展名路径回退 `index.html`（对应旧 `manifests/*.routes.json` 的浏览器路由）；
3. 带扩展名但未命中按 404 处理，**不**回退 HTML——否则缺失的 `.wasm`/`.js` 会拿到一份 200
   的 HTML，浏览器端只会报难以定位的解析错误；
4. 产物整体缺失返回 503 并说明需要执行的构建命令。

入口文档使用 `no-cache`：它引用带指纹的 WASM/JS 名称，缓存住会让浏览器加载到已被替换的旧
产物。指纹化产物本身按 `immutable` 长期缓存。
