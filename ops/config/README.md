# TOML 配置

运行时配置统一使用两个必需边界和一个可选 Admin 文件：主配置保存普通设置，敏感配置保存
凭据，Admin runtime TOML 保存可审计的非敏感运行参数。进程会在
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

Admin 示例的 `runtime_config_file` 指向 [`examples/runtime/admin.toml`](examples/runtime/admin.toml)。
该文件不包含数据库连接串、Cookie、密码或签名密钥；生产部署仍应把模板复制到受信目录，
确保父目录不可被 group/world 写入，并将 runtime 文件设为 `0640`。A26/A27 只在 PostgreSQL
完整档提供更新命令、fingerprint fence、durable 命令账本和原子替换；SQLite Lite 只可在
启动时读取已存在的文档，HTTP 更新返回 `501 feature_not_supported`。
其中 `dashboard_mailboxes` 是可选的非敏感 world/mailbox 投影；启用后 Admin 会按该范围核对
MoogleMail poller 和凭据 availability，必须与 Worker 的对应配置保持一致。
当前 `allow_manual_early_open` 与 `[equipment_source]` 虽可解析、校验和持久化，但尚未接入
生命周期授权与装备来源 HTTP 适配器；修改这两组字段目前不会改变对应运行行为，必须在该生效链
完成后才能作为可操作配置交付。

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

主配置中的 `[cache]` 只保存 `backend`。`backend = "disabled"` 不需要 Redis；
`backend = "redis"` 还要求对应二进制以 `cache-redis` feature 构建，并在敏感文件提供
`redis_url`。具体缓存写入由调用方显式提供有限 TTL；缓存关闭或不可用不得影响权威业务
结果。
当前三个装配根只会按配置建立并检查 cache adapter，尚无权威查询调用 `get`/`set`/`invalidate`；
因此 Redis 是已装配但尚未产生性能收益的可选能力，不能把“连接成功”描述为业务缓存已接入。

主配置还可以包含 `[auction]` 拍卖策略模板，分为 `[auction.economics]`、
`[auction.settlement]` 和 `[auction.settlement.recovery]`。这些字段都是非敏感的可选字段，
完整字段、中文注释和默认值见各角色示例。Admin 创建拍卖时，模板会在 PostgreSQL 事务中冻结
为 `policy_snapshot`，并同步写入遗留规则列；竞价和取消事务都会从这份快照恢复 PPM、代理
限制、final phase 及时间规则。MoogleMail 入站的默认起拍价继承同一 `[auction.economics]`
模板，只有显式覆盖值才会改变它。

Public/Admin 主配置还可以包含 `[http_security]` 与 `[session]`：前者只保存精确 origin、
速率限制和 Cookie `secure` 属性，后者保存会话时限与公开会话并发上限。两者都不是凭据，
但仍由配置校验限制范围；原始登录 key、会话 token 和数据库 URL 不得出现在主配置。

Public 主配置还可以包含 `[realtime]`：`consumer_key` 是持久化 relay 的稳定非敏感标识，
多 Public 实例必须各不相同；其余字段限制 relay 批次、轮询间隔和单连接缓冲。缓冲区满时
慢 WebSocket 会被淘汰，不能用增大缓冲替代客户端 catch-up。

Worker 的 `[worker.forum]` 支持可选 `public_base_url`；它只能是无凭据、无路径和无查询的
HTTPS origin（本地联调可用回环 HTTP），用于把论坛主题中的拍卖相对地址转换为公开端绝对链接；
`item_timeout_seconds` 必须覆盖三次请求与请求间隔预算，并小于独立 `lease_duration_seconds`。
Forum 使用自己的顺序循环，主题只在即将执行时逐条领取，发布失败后仍继续回帖导入。

Worker 的 `[worker.mooglemail]` 为每个 world 建立独立轮询任务和手动唤醒器；`schedule` 使用
旧版同等的标准五字段 cron，默认 `* * * * *`，并支持描述符、`@every` 与显式 IANA 时区前缀。
共享客户端使用旧版默认的一秒请求间隔与 15 秒请求超时，`fetch_timeout_seconds` 和
`item_timeout_seconds` 必须覆盖该请求预算；读取与单封邮件处理分别配置尝试次数、单次总超时和
指数退避。批次期间按 `renew_interval_seconds` 延长 claim，续租失败后取消陈旧处理；这些字段都是非敏感可选策略，
默认值和中文注释见 `examples/worker.toml`。

Worker 顶层 `[worker]` 控制 Auction 生命周期/结算主循环。省略并发时 PostgreSQL 使用 8、SQLite
Lite 使用 1；默认轮询 60 秒、抖动上限 5 秒、每类批次 100、单项总时限 30 秒。生命周期与结算
分别尝试完整批次，一类 claim 失败不会跳过另一类，也不会终止其它 Worker owner。

Worker 的 `[worker.delivery]` 独立控制 delivery outbox 的轮询及随机抖动、批次、并发、claim lease、sending
续租、过期宽限、完整外部请求和终态写回超时。续租间隔必须小于 lease，sending grace 必须大于
请求超时；这些字段都是可选非敏感策略，完整中文注释和默认值见 `examples/worker.toml`。该区块
只影响 PostgreSQL 完整档的外部交付，SQLite Lite 不装配 delivery Worker。

Worker 的 `[worker.fulfillment]` 和 `[worker.recovery]` 只在 PostgreSQL 完整档装配。两者默认立即
执行一轮，再等待 60 秒加最多 5 秒随机抖动；默认批次 100、并发 8、claim lease 60 秒、单项总超时
30 秒。临时失败保留持久化重试边界并只影响自身下一轮，不能阻塞其它 Worker owner。

Worker 的 `[worker.operations_alert]` 分别调度 scan、dispatch 和 reconcile 三类循环，默认每 60 秒
运行一次；scan/reconcile 受 claim lease 限时，dispatch 还会为来源读取预留超时。单类循环失败只在
下一轮重试，不阻塞另外两类 owner。批次窗口、冷却、重试、阈值和正文边界默认值与旧版一致。
