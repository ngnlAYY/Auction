# TOML 配置

运行时配置统一使用两个必需边界和一份可选、由 Admin 管理的共享 runtime 文件：主配置保存普通
设置，敏感配置保存凭据，runtime TOML 保存可审计的非敏感 Admin 参数与旧版 MoogleMail 业务
策略。Admin 和 Worker 可以指向同一文件，Public 不读取它。进程会在
`ops/config/{role}.toml` 存在时加载对应主配置；`AUCTION_CONFIG_FILE` 只负责选择另一个
主配置，单个配置项不会从环境变量读取。

已经纳入版本控制的主配置和敏感配置示例位于 [`examples/`](examples/)，runtime 配置示例
集中在 [`examples/runtime/`](examples/runtime/)，破坏性运维专用敏感模板集中在
[`examples/secrets/`](examples/secrets/)。主配置必须通过 `secrets_file`
引用敏感文件；相对路径以主配置所在目录解析。示例主配置引用的 `secrets.toml` 只包含
安全的本地占位值，不能直接用于生产环境。
每个示例赋值字段前都必须紧邻中文“可选”或“必填”注释；`just config-example-check` 会阻止
遗漏字段状态说明的示例进入默认静态验证。

三进程部署目录、启动顺序、健康探针、权限和 Caddy 上游模板见
[`../deploy/README.md`](../deploy/README.md)。该资料不替代具体主机的服务管理器配置。

生产或本地真实配置可按以下方式准备：

```bash
cp ops/config/examples/secrets.toml ops/config/auction.secret.toml
chmod 600 ops/config/auction.secret.toml
```

然后把主配置的 `secrets_file` 指向 `../auction.secret.toml`。`*.secret.toml` 和
`*.local.toml` 已被 Git 忽略，但部署者仍需保证备份、文件所有者和读取权限正确。
生产环境宜为 Public、Admin、Worker 分别挂载敏感文件，并只提供该角色实际需要的字段；
仓库中的单一 `secrets.toml` 只是共享结构示例。

`auctionctl postgres-rebuild` 可通过 `--maintenance-secrets-file` 短暂加载独立 TOML，结构见
[`examples/secrets/postgres-rebuild.toml`](examples/secrets/postgres-rebuild.toml)。该文件只有一个
必填 `database_url`，必须指向与 Worker 数据库相同 endpoint/TLS profile 的 `postgres` 维护库；
用户名和密码可以属于独立高权限维护角色。文件路径必须是规范化绝对路径，文件不得是 symlink，
父目录不得允许 group/other 写入；Unix 文件必须归命令当前有效用户所有、只有一个 hard link 且
权限为 owner-only。它不是三进程 runtime 配置，不得由 Public/Admin/Worker 的
`secrets_file` 引用；未提供时重建命令才会从普通 `database_url` 派生维护连接并要求该应用角色拥有
对应 owner/`CREATEDB` 权限。

Admin 与 Worker 示例的 `runtime_config_file` 都指向
[`examples/runtime/admin.toml`](examples/runtime/admin.toml)。
该文件不包含数据库连接串、Cookie、密码或签名密钥；生产部署仍应把模板复制到受信目录，
确保父目录不可被 group/world 写入，并将 runtime 文件设为 `0640`。A26/A27 只在 PostgreSQL
完整档提供更新命令、fingerprint fence、durable 命令账本和原子替换；SQLite Lite 只可在
启动时读取已存在的文档，HTTP 更新返回 `501 feature_not_supported`。
其中 `dashboard_mailboxes` 是可选的非敏感 world/mailbox 投影；启用后 Admin 会按该范围核对
MoogleMail poller 和凭据 availability，必须与 Worker 的对应配置保持一致。旧版可编辑的
`worlds`、各 world 发件人 allowlist、`equipment_filter` 和 14 个 `mail_templates` 也位于该文件；
Worker 重启时把它们投影为 MoogleMail 业务快照。transport endpoint、凭据来源、调度、mailbox
标识和全局 `worker.mooglemail.enabled` 仍由 Worker 主配置拥有。
`allow_manual_early_open`、`allow_manual_early_settlement` 与 `[equipment_source]` 已接入实际运行 owner，但仍遵守 runtime 更新的
`restart_required` 边界，不会热改当前进程；业务区块变更要求重启受影响的 Admin 与 Worker。重启后，前者决定 HTTP capability，并在生命周期事务
锁内约束计划时间之前的人工开放；计划时间已到的开放不受影响。人工提前结算开关默认关闭，并且只在
PostgreSQL 完整档声明 capability；启用后要求操作理由，把 live lot 截止时间收紧到事务时间，再由
既有 settlement Worker 写入结算与履约事实。`[equipment_source]` 配置 host/path allowlist、
64 KiB 至 8 MiB 响应上限、1 至 60 秒共享请求间隔、1 至 30 秒总超时，以及仅用于 Admin 手工
录入的 `require_tradeable` 开关。该开关默认关闭，关闭时允许 Soulbound 装备，开启时只接受可交易
装备；MoogleMail 邮件装备过滤仍独立拒绝不可交易来源。适配器始终保留固定 HTTPS origin、无重定向、
精确 world 路径和流式有界读取。示例默认 2 MiB、250 ms、10 秒和关闭可交易限制。

以下字段只能出现在敏感 TOML：

- `database_url`：数据库连接串；
- `redis_url`：启用 Redis 缓存时的连接串；
- `session_signing_key`：会话签名密钥；
- `ehentai_cookie`、`mooglemail_cookie`：外部服务 Cookie；
- `public_otlp_headers`、`worker_otlp_headers`、`admin_otlp_headers`：角色专属 OTLP 认证或路由头；
- `admin_actor`、`admin_secret`：Admin 登录交换身份与共享密钥，必须成对配置；
- `equipment_data_key_id`、`equipment_data_key`：Admin 装备 key 信封的轮换标识与 32 字节
  Base64 数据密钥；两者必须成对配置，且只在 PostgreSQL 完整档启用装备录入时使用。
- `user_login_data_key_id`、`user_login_data_key`：MoogleMail Users 注册/换绑登录 key 信封的
  独立轮换标识与 32 字节 Base64 数据密钥；两者必须成对配置，不能与装备 key 混用；
- `credential_data_key_id`、`credential_data_key`：Admin 外部 Cookie 凭据的独立轮换标识与
  32 字节 Base64 数据密钥，不能与装备或用户登录 key 混用。

旧的单文件格式不兼容；把这些字段放入主配置会因未知字段而启动失败。解析错误和日志不得
输出敏感 TOML 原文或上述字段值。

`database_url` 同时决定数据库和部署档位：

- `postgres://` 或 `postgresql://` 选择完整生产档，支持独立进程和完整持久化能力。
  部署基线固定 PostgreSQL major 18，并及时安装 18.x 当前修复版本。
- `sqlite:` 选择 Lite 档，只用于本地、演示或明确的单主机低并发部署。每个角色至多一个
  实例，Worker 并发为一，数据库文件必须位于本地文件系统；不承诺 PostgreSQL 的全部功能、
  多副本、高可用或并发性能。

Lite 已实现的写入仍必须满足事务原子性、命令幂等和追加式审计。不要通过额外环境变量
伪装或覆盖部署档位。

主配置中的 `[cache]` 保存非敏感的 `backend` 和可重建 Public 规则读模型的
`public_rules_ttl_seconds`。`backend = "disabled"` 不需要 Redis；`backend = "redis"` 还要求
对应二进制以 `cache-redis` feature 构建，并在敏感文件提供 `redis_url`。TTL 范围为 1-3600 秒，
默认 30 秒；缓存关闭或不可用不得影响权威业务结果。
Public 规则读取会按语言使用该 TTL 执行 `get`/`set`，规则发布成功后由 Admin 失效同一语言的 key。
命中内容必须通过完整文档校验；损坏快照会尽力删除并回源数据库。Redis 失效失败只依赖 TTL
收敛，不会回滚已经提交的规则事务。

主配置还可以包含 `[auction]` 拍卖策略模板，分为 `[auction.schedule]`、`[auction.economics]`、
`[auction.settlement]` 和 `[auction.settlement.recovery]`。这些字段都是非敏感的可选字段，
完整字段、中文注释和默认值见各角色示例。Admin 创建拍卖时，模板会在 PostgreSQL 事务中冻结
为 `policy_snapshot`，并同步写入遗留规则列；竞价和取消事务都会从这份快照恢复 PPM、代理
限制、final phase 及时间规则。MoogleMail 入站的默认起拍价继承同一 `[auction.economics]`
模板，只有显式覆盖值才会改变它。
`[auction.schedule]` 默认关闭；启用后的内置时间为 UTC 周四 22:00 开放预览、周五 00:00
正式开拍、周六 12:00 计划结束。所有星期、时间、时区和 world 均可在 TOML 中覆盖；默认值
变化只影响后续生成的新轮次，不会改写数据库中已经存在的拍卖。

Public/Admin 主配置还可以包含 `[listener]`、`[http_security]` 与 `[session]`。`[listener]` 的
`mode = "tcp"` 使用 `address`；`mode = "unix"`（仅 Linux）要求规范化的绝对
`runtime_directory`、直接位于该目录下的 `socket_path` 和 Unix `group`，TCP 模式下后三者必须
为空。Public 和 Admin 都支持这两种模式；Admin 若加载 runtime 文件，则由 runtime 的 listener
覆盖主配置 fallback。Unix listener 绑定前会检查目录 owner/group/权限、目录级单实例锁、stale
socket 身份和 `0660` 权限，Public/Admin 使用不同锁文件名，不能复用 Worker control socket。启用
metrics 时，有效 listener 必须是 loopback TCP 或 Unix；Admin runtime 覆盖后的有效组合也执行
同一校验，非本地 TCP 会在绑定前失败关闭。

`[http_security]` 前者只保存精确 origin、
速率限制、Cookie `secure`、普通请求处理超时和可信代理网段，后者保存会话时限与公开会话并发
上限。`trusted_proxy_cidrs` 只接受最多 64 个规范且不重复的 CIDR；默认空集合会忽略所有
`X-Forwarded-For`。`trusted_unix_proxy` 只允许 Public/Admin 的有效 Unix listener 启用，不能用它
放宽 TCP peer。两者都不是凭据，
但仍由配置校验限制范围；省略 origin 开关时默认启用同源 allowlist。Public 会话默认空闲 30 分钟、
绝对 7 天，Admin runtime 默认空闲 15 分钟、绝对 8 小时。原始登录 key、会话 token 和数据库 URL
不得出现在主配置。

Public 主配置还可以包含 `[realtime]`：`consumer_key` 是持久化 relay 的稳定非敏感标识，
多 Public 实例必须各不相同；PostgreSQL 会以会话级 advisory owner lock 拒绝重复 key 的第二个
实例启动，SQLite Lite 在同主机使用 0600 非阻塞 sidecar 文件锁强制同 key 单实例，非 Unix 平台
无法取得该互斥证明时拒绝启动。其余字段限制 relay 批次、轮询间隔和单连接缓冲。缓冲区满时
慢 WebSocket 会被淘汰，不能用增大缓冲替代客户端 catch-up。

Worker 的 `[worker.forum]` 支持可选 `public_base_url`；它只能是无凭据、无路径和无查询的
HTTPS origin（本地联调可用回环 HTTP），用于把论坛主题中的拍卖相对地址转换为公开端绝对链接；
`poll_interval_seconds`/`jitter_seconds` 默认使用独立的 300/5 秒周期，不改变 Auction 顶层 60 秒循环；
成功扫描会把数据库中的下一次 due 推进一个完整论坛周期。`maximum_topic_pages` 默认 10、硬上限 20，
超过上限会明确失败；`item_timeout_seconds` 必须覆盖分页读取、固定楼层写入、共享请求间隔与收口预算，
并小于独立 `lease_duration_seconds`。Forum 使用自己的顺序循环，主题只在即将执行时逐条领取，发布
失败后仍继续回帖导入。

Worker 的 `[worker.mooglemail]` 为每个 world 建立独立轮询任务和手动唤醒器；`schedule` 使用
标准五字段 cron，默认 `*/5 * * * *`（每 5 分钟），并支持旧版描述符、`@every` 与显式 IANA
时区前缀。
共享客户端使用旧版默认的一秒请求间隔与 15 秒请求超时，`fetch_timeout_seconds` 和
`item_timeout_seconds` 必须覆盖该请求预算；读取与单封邮件处理分别配置尝试次数、单次总超时和
指数退避。批次期间按 `renew_interval_seconds` 延长 claim，续租失败后取消陈旧处理；这些字段都是非敏感可选策略，
默认值和中文注释见 `examples/worker.toml`。若共享 runtime 同时包含完整的 `worlds`、
`equipment_filter` 和 `mail_templates`，这些业务字段覆盖 Worker 主配置中的对应直接配置；三个区块
全部缺失时保留直接配置和旧 runtime fingerprint，部分存在则配置失败。确定性业务/输入/配置错误不
执行 inline retry，只有可重试依赖错误和超时按配置尝试次数重试，失败时 cursor 不提前推进。

Worker 顶层 `[worker]` 控制 Auction 生命周期、慢速代理和结算主循环。省略并发时 PostgreSQL 使用
8、SQLite Lite 使用 1；默认轮询 60 秒、抖动上限 5 秒、每阶段批次 100、单项总时限 30 秒。
三个阶段分别尝试完整批次，一阶段读取/命令失败不会跳过后续阶段，也不会终止其它 Worker owner；
SQLite Lite 不启用结算阶段。

Worker 的 `[worker.delivery]` 独立控制 delivery outbox 的轮询及随机抖动、批次、并发、claim lease、sending
续租、过期宽限、完整外部请求和终态写回超时。续租间隔必须小于 lease，sending grace 必须大于
请求超时；这些字段都是可选非敏感策略，完整中文注释和默认值见 `examples/worker.toml`。该区块
只影响 PostgreSQL 完整档的外部交付，SQLite Lite 不装配 delivery Worker。`concurrency` 是配置
上限，实际 runner 还会取 transport 声明上限的较小值；当前 MoogleMail 因账号级 compose 状态固定为 1。
论坛和游戏邮件分别只允许一个 active Cookie；Worker 按凭据类型取得 PostgreSQL session
advisory ownership，替换或失效后需要重启 Worker 完成所有权交接。`.keyenv` 兼容入口只把
`hv-ck-key1` 用作运行 Cookie，`hv-ck-key2` 仅供显式隔离验证，不会自动切换。

Worker 的 `[worker.fulfillment]` 和 `[worker.recovery]` 只在 PostgreSQL 完整档装配。两者默认立即
执行一轮，再等待 60 秒加最多 5 秒随机抖动；默认批次 100、并发 8、claim lease 60 秒、单项总超时
30 秒。临时失败保留持久化重试边界并只影响自身下一轮，不能阻塞其它 Worker owner。

Worker 的 `[worker.operations_alert]` 分别调度 scan、dispatch 和 reconcile 三类循环，默认每 60 秒
加最多 5 秒随机抖动运行一次；抖动上限不能超过轮询间隔。scan/reconcile 受 claim lease 限时，dispatch
还会为来源读取预留超时。单类循环失败只在下一轮重试，不阻塞另外两类 owner。批次窗口、冷却、重试、
阈值和正文边界默认值与旧版一致。
