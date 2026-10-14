# 运行配置与敏感文件

[文档导航](../../docs/README.md) · [loader 与校验](../../crates/auction-config/README.md) · [部署](../deploy/README.md)

运行配置统一使用 TOML。字段、默认值和启用条件以受版本控制的示例及 loader 为准；本页说明文件关系、
生效时机和安全边界。配置保存不改变数据库权威事实。

## 本页导航

- [文件职责](#文件职责)
- [首次部署配置生成](#首次部署配置生成)
- [准备私有配置](#准备私有配置)
- [PostgreSQL 连接参数](#postgresql-连接参数)
- [PostgreSQL 维护凭据](#postgresql-维护凭据)
- [Runtime 共享与重启](#runtime-共享与重启)
- [敏感字段](#敏感字段)
- [后端和拓扑](#后端和拓扑)
- [缓存与拍卖模板](#缓存与拍卖模板)
- [HTTP listener 与安全](#http-listener-与安全)
- [Public 实时 relay](#public-实时-relay)
- [论坛与邮件](#论坛与邮件)
- [Worker 循环预算](#worker-循环预算)

## 文件职责

| 文件/模板 | 所有内容 | 引用方式 |
| --- | --- | --- |
| [common.toml](examples/common.toml) | 日志、指标、缓存、拍卖策略 | 三角色必填 `common_file`，必须指向同一普通文件 |
| [public.toml](examples/public.toml)、[admin.toml](examples/admin.toml)、[worker.toml](examples/worker.toml) | 角色 listener、tracing、realtime、session 或任务调度 | 默认 `ops/config/{role}.toml`；`AUCTION_CONFIG_FILE` 仅改选文件 |
| [PostgreSQL 敏感模板](examples/secrets/postgres.toml)、[Lite 敏感模板](examples/secrets.toml) | 数据库、Cookie、密钥等敏感字段 | 每角色必填 `secrets_file` |
| [runtime/admin.toml](examples/runtime/admin.toml) | 可审计的 Admin 参数、邮件业务策略、可接管运行参数 | Admin/Worker 可共享 `runtime_config_file`，Public 不读取 |
| [维护凭据](examples/secrets/postgres-rebuild.toml) | 短期建库/重建权限 | 仅 CLI 的 `--maintenance-secrets-file` |

loader 先固定角色文件的绝对位置，再以其目录解析相对引用，包括默认发现和相对的配置选择器。
不支持逐项环境变量覆盖。示例占位值不能用于生产；每个示例赋值字段前必须紧邻中文“可选/必填”注释，
由 `just config-example-check` 检查。

## 首次部署配置生成

内嵌 PostgreSQL Admin 的 `setup --output-dir <绝对路径>` 提供 loopback 一次性设置；
无默认配置且未设置 `AUCTION_CONFIG_FILE` 时也可自动进入。目标目录必须不存在，具体步骤见
[首次部署](../deploy/README.md#首次部署配置)。

生成器复用通用模板，在私有临时目录设置敏感文件 0600、非敏感文件 0640，逐角色 loader 校验后
原子发布完整目录；不依赖 umask 放宽权限。生成配置信任本机代理、启用 Worker control 并绑定服务 UID，
不包含联调 `.keyenv`、部署码、首次确认值或临时代理策略。正式 Admin 不开放初始化 API。
分体设置保存后退出，运行目录、组、unit、代理与迁移另按部署手册准备。

[合并服务](../../apps/server/README.md)使用相同 TOML 格式，将 control 放在配置目录同级私有运行目录，
以当前账号组/UID 管理；仅网页明确确认后初始化专用空库。其 `--config-dir` 与 `AUCTION_CONFIG_FILE` 互斥。

## 准备私有配置

```bash
cp ops/config/examples/secrets/postgres.toml ops/config/auction.secret.toml
chmod 600 ops/config/auction.secret.toml
```

填写真实参数后，仓库示例角色文件可把 `secrets_file` 改为 `../auction.secret.toml`。
`*.secret.toml`、`*.local.toml` 被 Git 忽略；备份、owner 和权限仍由部署者保证。
生产宜按角色分别挂载最少敏感字段；首次设置生成的共享文件也可按此拆分。

## PostgreSQL 连接参数

敏感 TOML `[database]` 的 `host`、`port`、`username`、`password`、`name` 五项必填，见
[完整模板](examples/secrets/postgres.toml)。host 只填主机名/IP，不含协议、端口或路径；port 为 1–65535。
账号、密码和库名按原文填写，包含 `@`、`:`、`%` 也不要预转义，由配置层编码。
`admin_actor` 等根字段须放在 `[database]` 前面，避免被 TOML 归入该表。

兼容 `database_url` 用于 SQLite，或需要 Unix socket/显式 TLS 参数的 PostgreSQL；与 `[database]` 二选一，
混用即失败。五字段形式沿用驱动默认 TLS，需要显式策略时使用带参数 URL。
数据库凭据只在敏感文件中，外部备份工具同样要求显式 host/port，不从环境补齐目标。

## PostgreSQL 维护凭据

`postgres-bootstrap` 加载 Public 应用配置，`postgres-rebuild` 加载 Worker 应用配置。
其独立维护 TOML 只接受 `[database]` 或 `database_url`，必须指向与该应用配置相同 endpoint/TLS profile 的
`postgres` 维护库；可以使用不同高权限账号，不能被常驻角色的 `secrets_file` 引用。

路径须规范化绝对路径、非 symlink，父目录不得 group/other 可写；Unix 文件归命令有效用户所有、
只有一个 hard link 且 owner-only。未提供时从应用 URL 仅替换库名派生维护连接，应用角色须具备所需权限。
具体 owner/CREATEDB、查询参数和破坏性门禁见[恢复手册](../deploy/RECOVERY.md#2-准备空目标)。

## Runtime 共享与重启

Admin/Worker 示例共用 [runtime 模板](examples/runtime/admin.toml)。生产文件放在受信目录，父目录不得
被 group/other 写入，文件须由加载/更新进程的有效 UID 拥有并按 0640 管理；Admin/Worker 共享时使用同一运行账号。
A26/A27 的更新、fingerprint fence、命令账本和原子替换仅支持
PostgreSQL；SQLite 可在启动时读取文件，HTTP 更新返回 `501 feature_not_supported`。
页面提交完整候选，保存成功是 TOML 落盘，不是热加载。Admin 启动恢复任何失败（含结果未知/需人工核对）均停止启动，
不能沿用旧安全配置；恢复若改变已加载指纹，必须重新部署重启。

| 配置组 | 责任与生效范围 | 重启范围 |
| --- | --- | --- |
| Admin listener/security/session、人工提前操作、equipment_source | Admin 启动快照 | Admin |
| `worlds`、发件人 allowlist、`equipment_filter`、14 个 `mail_templates` | Admin 管理、Worker 邮件业务快照 | 受影响的 Admin/Worker |
| `operations.economics` | 后续新拍卖的冻结经济模板 | Admin、Worker |
| `operations.schedule` | 后续周排期的时区/星期/时间/world | Admin、Worker |
| `operations.forum` | 论坛开关、批量、分页和请求/任务预算 | Worker |
| `operations.alerts` | 告警开关、阈值、冷却、重试和收件人 | Worker |
| `operations.worker` | 生命周期、delivery、fulfillment、recovery 预算 | Worker |
| `operations.mail` | 邮件 cron、批量、分页、HTTP/处理预算、续租和退避 | Worker |

六组 `operations` 默认 `managed=false`，勾选接管后才覆盖公共/角色参数；表单初始候选不代表运行配置。
接管任一组时部署预检要求同一 runtime 文件，始终核对 Admin/Worker 有效拍卖、结算模板和排期一致。
不回写已有拍卖快照、due、游标、账本或租约。论坛 endpoint/scope/公开链接/测试板块及 MoogleMail
transport/凭据/mailbox identity/全局启用仍归部署配置。

旧文档省略 `operations` 或 `operations.mail` 保持指纹，显式 null 不被接受。
邮件 `batch_timeout_seconds` 默认 600 秒且默认值省略序列化，升级新增字段不会单独改变旧指纹。
cron 候选校验与 Worker 共用解析器，保存不立即轮询。
可选 `dashboard_mailboxes` 仅为非敏感范围投影，须与 Worker 实际 mailbox/world 一致。

装备等级 min/max 各为 0–1000，0 关闭该边界；非零 max 不小于 min，启用边界时缺等级仍拒绝。
`allow_manual_early_open`、`allow_manual_early_settlement` 默认关闭；事务内按同一策略限制提前操作。
提前结算仅 PostgreSQL，要求理由并收紧 lot 截止时间，实际结算仍由 Worker 执行。
`equipment_source.require_tradeable` 默认关闭，仅影响 Admin 手工录入，邮件过滤独立拒绝不可交易来源。
装备请求固定 HTTPS origin、禁重定向、精确 world 路径和有界响应；示例预算为 2 MiB、250 ms 间隔、10 秒总超时。

## 敏感字段

| 字段 | 用途与限制 |
| --- | --- |
| `[database]` / `database_url` | 数据库连接，二选一 |
| `redis_url` | 可选 Redis |
| `session_signing_key` | 会话签名 |
| `ehentai_cookie`、`mooglemail_cookie` | 外部服务单 Cookie 入口 |
| `public_otlp_headers`、`admin_otlp_headers`、`worker_otlp_headers` | 角色专属 OTLP 头 |
| `admin_actor`、`admin_secret` | 成对配置；actor 1–256 字节，禁冒号、首尾空白及控制字符；secret 16–4096 字节且禁 ASCII 控制字符；首次表单另限 actor 128、secret 512 字节 |
| `equipment_data_key_id` / `equipment_data_key` | 装备信封密钥对 |
| `user_login_data_key_id` / `user_login_data_key` | 登录 key 信封密钥对 |
| `credential_data_key_id` / `credential_data_key` | 外部 Cookie 信封密钥对 |

三组数据密钥分别为独立轮换标识与 32 字节 Base64 key，成对配置且不能混用。
旧单文件敏感格式不兼容；放入角色主配置会因未知字段失败。日志、错误和 DTO 不得包含敏感原文。

## 后端和拓扑

`[database]` 或 `postgres://`/`postgresql://` 选择 PostgreSQL 完整档，部署基线 major 18。
`sqlite:` 为本地/演示/单主机低并发 Lite：每角色至多一个实例、Worker 并发 1、数据库位于本地文件系统。
能力见[架构矩阵](../../docs/architecture.md#部署档位与能力)；Lite 已支持的写入同样保留事务、幂等和审计。
部署预检要求三角色同一数据库目标；具体 endpoint/path 比较见[auctionctl](../../apps/auctionctl/README.md#部署预检)。

## 缓存与拍卖模板

公共 `[cache]` 默认 disabled；redis 需制品 `cache-redis` 和敏感 `redis_url`。
规则 TTL 默认 30 秒、范围 1–3600；按语言缓存，发布后失效。损坏值回源，失效失败靠 TTL 收敛，
不回滚权威事务。缓存不参与竞价、会话、锁、幂等或审计正确性。

`[auction]` 的 schedule/economics/settlement/recovery 默认值见[公共模板](examples/common.toml)。
新拍卖创建时冻结 `policy_snapshot`，后续出价/取消使用快照；模板改动不回写旧场次。
自动后继也是新聚合，冻结 Admin/Worker 启动时有效模板，而非复制前驱经济规则。
周排期默认关闭，内置 UTC 周四 22:00 预览、周五 00:00 开拍、周六 12:00 结束，可显式覆盖。
邮件默认起拍价独立，见[Worker 定价说明](../../apps/worker/README.md#拍卖邮件起拍价)。

## HTTP listener 与安全

Public/Admin 主配置 `[listener]` 的 tcp 使用 address，Unix 字段须为空；Linux unix 使用规范绝对
runtime_directory、直接位于其中的 socket_path 与 group。Admin runtime listener 覆盖主配置 fallback；
其 tcp 模式允许保留未使用的 runtime_directory，但 group/socket_path 仍须为空。
Unix 绑定核验目录 owner/group、目录级锁、stale socket 身份并固定 0660；目录不得 group/other 可写，
祖先须 root/运行账号所有且不可被 group/other 写入，仅 root-owned sticky 祖先例外。目录链禁止 symlink。
代理组只需遍历/连接权限，两端锁名不同，不能复用 Worker control socket。
启用 metrics 时有效 listener 只能 loopback TCP 或 Unix；禁止通过代理暴露公网。

`[http_security]` 保存精确 origin、限速、Secure Cookie、请求超时与可信代理；`[session]` 保存会话预算。
`trusted_proxy_cidrs` 最多 64 个规范不重复 CIDR，默认空时忽略 X-Forwarded-For；`trusted_unix_proxy`
仅允许有效 Unix listener，不能放宽 TCP peer。同源 allowlist 默认启用。
Public session 默认空闲 30 分钟/绝对 7 天，Admin runtime 默认 15 分钟/8 小时。

## Public 实时 relay

`[realtime].consumer_key` 是稳定非敏感标识，多 Public 实例必须不同。PostgreSQL session advisory lock
拒绝重复 owner；连接失效须停止消费并重启实例。SQLite 使用数据库同目录 0600 sidecar 锁，平台/文件身份
无法证明互斥时失败关闭，详见[Public 故障边界](../../apps/public-api/README.md#持锁-relay-与故障边界)。
其余字段限制批次、轮询和单连接缓冲；满缓冲淘汰慢连接，客户端须 catch-up。

## 论坛与邮件

完整字段见[Worker 模板](examples/worker.toml)。Forum 使用独立“发布 → 回帖导入”循环，阶段失败互不跳过；
默认 300 秒周期加最多 5 秒抖动，分页默认 10、硬上限 20，超限失败而不截断出价。
item timeout 须覆盖分页/写入/间隔且小于 lease。`public_base_url` 只允许无凭据/路径/查询的 HTTPS origin，
回环 HTTP 仅供本地联调。

MoogleMail 每个 world 有独立调度/唤醒，cron 默认 `*/5 * * * *`，兼容描述符、`@every`、
`TZ=`/`CRON_TZ=`；未指定时区使用 Worker 本地时区，部署宜显式指定。
共享客户端默认 1 秒间隔、15 秒请求超时；fetch/item 预算须覆盖请求，确定性错误不 inline retry。
一次拉取在绝对 deadline 内返回已解析的最旧详情前缀，batch_size 只是上限；无法完整定位 cursor/末尾则失败关闭。

batch_timeout 默认 600 秒、范围 1–86400，至少覆盖一次 fetch_timeout + item_timeout；可先于全部重试到期。
到期只提交已完成前缀及末封 ID，无完成项则持久化失败退避。批次续租，token/fence 失效即取消陈旧处理。
runtime 的 worlds/equipment_filter/mail_templates 必须完整同时存在或全部缺失；全缺失才沿用主配置兼容值。

## Worker 循环预算

顶层生命周期、慢速代理和结算默认 60 秒周期、5 秒抖动、各阶段批次 100、单项 30 秒。
省略并发时 PostgreSQL 为 8、Lite 为 1；示例显式选择 Lite 并发 1。阶段失败不跳过后续阶段，Lite 不执行结算。
fulfillment/recovery 独立循环默认立即执行后等待 60 秒加抖动，批次 100、并发 8、lease 60 秒、单项 30 秒。

PostgreSQL delivery 独立控制领取、续租、发送与终态确认；续租间隔小于 lease，sending grace 大于完整请求超时。
默认 timeout/grace 为 180/300 秒，启用 MM 时 timeout 大于两倍 fetch budget。
实际并发取配置与 transport 上限较小者，MM compose 账号状态固定上限 1。
论坛和邮件每种 Cookie 全局最多一条 active，由 session advisory lock 保证 Worker owner；更换或失效后重启交接。
`.keyenv` 仅由严格 loader 读取，Worker 只用 `hv-ck-key1`，`hv-ck-key2` 仅供显式隔离验证且不自动切换。

operations_alert 的 scan/dispatch/reconcile 独立运行，默认 60 秒加最多 5 秒抖动；抖动不得大于间隔。
各循环失败只影响自身重试，scan/reconcile 受 claim lease 约束，dispatch 另预留来源读取预算。
其他范围和默认值以模板注释为准，修改后仍需重启。
