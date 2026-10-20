# 运行配置与敏感文件

[文档导航](docs/README.md) · [loader 与校验](crates/auction-config/README.md) · [部署](ops/README.md)

运行配置统一使用 TOML。字段、默认值和启用条件以受版本控制的示例及 loader 为准；本页说明文件关系、生效时机和安全边界。
配置保存不改变数据库权威事实。

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
- [Cloudflare 与 Unix 上游](#cloudflare-与-unix-上游)
- [Public 实时 relay](#public-实时-relay)
- [论坛与邮件](#论坛与邮件)
- [Worker 循环预算](#worker-循环预算)

## 文件职责

| 文件/模板 | 所有内容 | 引用方式 |
| --- | --- | --- |
| [common.toml](ops/config/examples/common.toml) | 日志、指标、缓存、拍卖策略 | 三角色必填 `common_file`，必须指向同一普通文件 |
| [public.toml](ops/config/examples/public.toml)、[admin.toml](ops/config/examples/admin.toml)、[worker.toml](ops/config/examples/worker.toml) | 角色 listener、tracing、realtime、session 或任务调度 | 默认 `ops/config/{role}.toml`；`AUCTION_CONFIG_FILE` 仅改选文件 |
| [敏感模板](ops/config/examples/secrets.toml) | PostgreSQL、Cookie、密钥等敏感字段 | 每角色必填 `secrets_file` |
| [runtime/admin.toml](ops/config/examples/runtime/admin.toml) | 可审计的 Admin 参数、邮件业务策略、可接管运行参数 | Admin/Worker 可共享 `runtime_config_file`，Public 不读取 |
| [维护凭据](ops/config/examples/secrets/postgres-rebuild.toml) | 短期建库/重建权限 | 仅 CLI 的 `--maintenance-secrets-file` |

loader 先固定角色文件的绝对位置，再以其目录解析相对引用，包括默认发现和相对的配置选择器；不支持逐项环境变量覆盖。
示例占位值不能用于生产，每个示例赋值字段前必须紧邻中文“可选/必填”注释，由 `bash ci/checks/check.sh config` 检查。
启用 tracing 时，设置 `OTEL_EXPORTER_OTLP_HEADERS` 或 `OTEL_EXPORTER_OTLP_TRACES_HEADERS`（含空值）会被拒绝，防止 SDK 覆盖
敏感 TOML 中的导出头；须在部署端移除，错误不回显变量值。

## 首次部署配置生成

内嵌 PostgreSQL Admin 的 `setup --output-dir <绝对路径>` 提供 loopback 一次性设置；无默认配置且未设置 AUCTION_CONFIG_FILE
时可自动进入，目标目录必须不存在。分体保存后退出，正式 Admin 不开放初始化 API；安装与迁移见
[首次部署](ops/README.md#首次部署配置)。

生成器在私有临时目录复用模板，固定敏感文件 0600、非敏感文件 0640（不依赖 umask），经三角色 loader 校验后原子发布；
生成配置信任本机代理，启用 Worker control 并绑定服务 UID，不含 `.keyenv`、部署码、首次确认值或临时代理策略。
[合并服务](apps/auction-server/README.md#首次启动)复用此格式，以当前账号组/UID 管理配置目录同级的私有 control 目录；仅网页明确
确认后初始化专用空库，`--config-dir` 与 `AUCTION_CONFIG_FILE` 互斥。

## 准备私有配置

```bash
cp ops/config/examples/secrets.toml ops/config/auction.secret.toml
chmod 600 ops/config/auction.secret.toml
```

填写真实参数后，仓库示例角色文件可把 `secrets_file` 改为 `../auction.secret.toml`。`*.secret.toml`、`*.local.toml` 被 Git
忽略；备份、owner 和权限仍由部署者保证。生产宜按角色分别挂载最少敏感字段，首次设置生成的共享文件也可按此拆分。

## PostgreSQL 连接参数

敏感 TOML `[database]` 的 `host`、`port`、`username`、`password`、`name` 五项必填，见
[完整模板](ops/config/examples/secrets.toml)。host 只填主机名/IP，不含协议、端口或路径；port 为 1–65535。账号、密码和库名
按原文填写，包含 `@`、`:`、`%` 也不要预转义，由配置层编码。`admin_actor` 等根字段须放在 `[database]` 前面，避免被 TOML
归入该表。

兼容 `database_url` 仅接受 `postgres://`、`postgresql://`，与 `[database]` 二选一，混用即失败。五字段形式沿用驱动默认 TLS；
Unix socket 或显式 TLS 策略使用带参数 URL，其他协议不受支持。数据库凭据只在敏感文件中，外部备份工具同样要求显式
host/port，不从环境补齐目标。

旧 URI 必须在 authority/路径或查询覆盖中明确指定最终 host（含 Unix socket）和库名；拒绝 `postgres:` 缩略形式、
空目标、fragment、控制字符及无效百分号或 UTF-8 转义。URI/query 显式端口须非零；省略端口固定为 5432，配置层只追加
该默认端口，不重写原文凭据编码。普通角色与直接存储连接都不使用 `PGHOST`、`PGPORT`、`PGDATABASE` 补齐目标；用户名、
密码、pgpass、IPv6、socket 和 TLS 参数保留原驱动语义。

## PostgreSQL 维护凭据

`postgres-bootstrap` 加载 Public 应用配置，`postgres-rebuild` 加载 Worker 应用配置。其独立维护 TOML 只接受 `[database]` 或
`database_url`，必须指向与该应用配置相同 endpoint/TLS profile 的 `postgres` 维护库；可以使用不同高权限账号，不能被常驻
角色的 `secrets_file` 引用。

路径须规范化绝对路径、非 symlink，父目录不得 group/other 可写；Unix 文件归命令有效用户所有、只有一个 hard link 且
owner-only。未提供时从应用 URL 仅替换库名派生维护连接，应用角色须具备所需权限。具体 owner/CREATEDB、查询参数和破坏性
门禁见[恢复手册](ops/deploy/RECOVERY.md#2-准备空目标)。

## Runtime 共享与重启

`equipment_filter.qualities` 是硬性接收品质门槛，默认 `["Legendary", "Peerless"]`；低于该列表的拍卖装备邮件直接退回，`equipment_filter.enabled` 只控制前后缀、分类、类型和等级等附加过滤。空列表回退默认 L/P。
`equipment_colors.rules` 控制论坛拍卖帖中的装备名称标色：可按装备类型（如 `One-Handed`）和装备种类（如 `Rapier`）过滤，按顺序首个命中；前缀/后缀分别标色，二者同时命中时整件装备使用 combined 颜色；Peerless 品质词固定彩虹色。该配置只影响论坛帖子，不改变 Public/Admin 装备展示。
类型、种类、品质分别用 `categories`、`equipment_types`、`qualities` 数组多选，同维 OR、维度间 AND；
每组最多 256 项，每项为去首尾空白后的 1–64 字节名称、无控制字符且不重复（忽略 ASCII 大小写）。空数组沿用旧
`category`、`equipment_type`、`quality` 单值，两者均空表示该维不过滤；同维新旧字段均非空拒绝。
空数组编码时省略，以保持旧配置指纹及命令重放；新非空数组须使用支持该契约的 Admin、Worker 和 `auctionctl`，
保存后按下表重启，Worker 以启动时的单条多选规则渲染，不拆成多条规则。
新 `equipment_types` 数组的材质族闭集为 Cloth 的 Cotton/Gossamer/Ironsilk/Phase、Light 的
Leather/Drakehide/Kevlar/Shade、Heavy 的 Plate/Chain/Reactive/Power。材质族匹配要求实际装备大类正确、
完整种类首词精确相等并以空格连接非空部位，不作子串匹配；其他数组选项精确匹配完整种类。
例如 `["Phase", "Shade Helmet"]` 匹配所有 Phase 部位或精确的 Shade Helmet；旧 `equipment_type` 单值仍只精确匹配完整名称。
`bot_players` 是 runtime 顶层的 bot 显示名列表（最多 16 个，匹配大小写不敏感），Worker 启动时读取独立名单投影。
命中名单的买家或流拍/撤拍返还收件人保留在内部履约路径；仅在真实远端身份、装备及资金证明全部通过后完成内部义务，
流拍/撤拍装备留存。当前凭据身份与收件人不匹配时持久延期，不转普通外发，也不凭名单直接满足义务。
修改名单须重启 Worker，只影响尚未生成 delivery 的新结算；已有 `manual_reconcile` 投递仍需人工收口。

Admin/Worker 共用 [runtime 模板](ops/config/examples/runtime/admin.toml)；共享文件须使用同一运行账号，文件归加载/更新进程
有效 UID、权限 0640，父目录受信且禁止 group/other 写入。A26/A27 提交完整候选，以 fingerprint fence、命令账本和原子替换
落盘，不热加载。Admin 启动恢复失败（含结果未知/需人工核对）即阻断启动，不沿用旧安全配置；恢复改变已加载指纹须重新部署
重启。

| 配置组 | 责任与生效范围 | 重启范围 |
| --- | --- | --- |
| Admin listener/security/session、人工提前操作、equipment_source | Admin 启动快照 | Admin |
| `worlds`、发件人 allowlist、`equipment_filter`、`equipment_colors`、14 个 `mail_templates` | Admin 管理、Worker 邮件业务/论坛标色快照 | 受影响的 Admin/Worker |
| `bot_players` | Admin 管理、Worker 内部履约收件人名单快照 | Worker |
| `operations.economics` | 后续新拍卖的冻结经济模板 | Admin、Worker |
| `operations.schedule` | 后续周排期的时区/星期/时间/world | Admin、Worker |
| `operations.forum` | 论坛开关、批量、分页和请求/任务预算 | Worker |
| `operations.alerts` | 告警开关、阈值、冷却、重试和收件人 | Worker |
| `operations.worker` | 生命周期、delivery、fulfillment、recovery 预算 | Worker |
| `operations.mail` | 邮件 cron、批量、分页、HTTP/处理预算、续租和退避 | Worker |

六组 `operations` 默认 `managed=false`，接管后才覆盖公共/角色参数，表单候选不代表运行配置。预检始终要求 Admin/Worker 有效
拍卖、结算模板和排期一致，接管任一组还须共用 runtime。配置不回写已有拍卖快照、due、游标、账本或租约；论坛 endpoint/
scope/公开链接/测试板块/世界板块 ID（`main_board_id` 默认 77、`isekai_board_id` 默认 90）及 MoogleMail transport/凭据/mailbox identity/全局启用仍归部署配置。省略 `operations` 或
`operations.mail` 保持旧指纹，显式 null 拒绝；邮件 `batch_timeout_seconds` 默认 600 秒，默认值不序列化，新增字段不单独改变
旧指纹。cron 与 Worker 共用解析器，保存不立即轮询；可选 `dashboard_mailboxes` 仅投影非敏感范围，须匹配 Worker 实际
mailbox/world。

装备等级 min/max 各为 0–1000，0 关闭该边界；非零 max 不小于 min，启用边界时缺等级仍拒绝。`allow_manual_early_open`、
`allow_manual_early_settlement` 默认关闭，事务内按同一策略限制提前操作；提前结算要求理由并收紧 lot 截止时间，实际结算仍由
Worker 执行。`equipment_source.require_tradeable` 默认关闭，仅影响 Admin 手工录入，邮件过滤独立拒绝不可交易来源。装备请求
固定 HTTPS origin、禁重定向、精确 world 路径和有界响应；示例预算为 2 MiB、250 ms 间隔、10 秒总超时。

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

三组数据密钥分别为独立轮换标识与 32 字节 Base64 key，成对配置且不能混用。旧单文件敏感格式不兼容；放入角色主配置会因
未知字段失败。日志、错误和 DTO 不得包含敏感原文。

## 后端和拓扑

数据库仅支持 PostgreSQL，部署基线 major 18；保留分体和单主机合并部署。部署预检要求三角色同一数据库目标，连接目标比较见
[auctionctl](apps/auctionctl/README.md#部署预检)。

## 缓存与拍卖模板

公共 `[cache]` 默认 disabled；redis 需制品 `cache-redis` 和敏感 `redis_url`。规则 TTL 默认 30 秒、范围 1–3600；按语言缓存，
发布后失效。损坏值回源，失效失败靠 TTL 收敛，不回滚权威事务；缓存不参与竞价、会话、锁、幂等或审计正确性。

`[auction]` 的 schedule/economics/settlement/recovery 默认值见[公共模板](ops/config/examples/common.toml)。新拍卖创建时冻结
`policy_snapshot`，后续出价/取消使用快照；模板改动不回写旧场次。自动后继也是新聚合，冻结 Admin/Worker 启动时有效模板，
而非复制前驱经济规则。

新场次软截止窗口 `soft_close_window_seconds` 与延长 `soft_close_extension_seconds` 默认均为 900 秒（固定 15 分钟）。有效出价的
候选截止是接受时刻加 900 秒，不是旧截止加 900 秒，且不得回退。升级程序不会覆盖部署文件中的显式 60，也不会修改既有
草稿；操作者须先核对 effective economics：`operations.economics.managed = true` 时更新共享 runtime 的
`[operations.economics]`，否则更新公共文件 `[auction.economics]`，把有效来源的两个值都手动改为 900，核对 Admin/Worker 使用
相同模板后受控重启相关程序。首次设置生成的新文件已使用 900。现有从未发布草稿另行使用
[显式修正命令](apps/auctionctl/README.md#草稿软截止显式修正)，逐个指定目标并确认；进行中、已发布或已结算场次不追溯、
不自动重开，不批量改库。周排期默认关闭，内置 UTC 周四 22:00 预览、周五 00:00 开拍、周六 12:00 结束，可显式覆盖。
邮件默认起拍价独立，见[Worker 定价说明](apps/auction-worker/README.md#拍卖邮件起拍价)。

## HTTP listener 与安全

Public/Admin 主配置 `[listener]`：tcp 使用 address，Unix 字段为空；unix 使用规范绝对 runtime_directory、直属 socket_path 和
group。Admin runtime 覆盖主配置 fallback，其 tcp 可保留未使用的 runtime_directory，group/socket_path 仍须为空。Unix 绑定核验
目录 owner/group、目录锁和 stale socket 身份，socket 固定 0660；目录禁 group/other 写入，祖先须 root/运行账号所有且禁
group/other 写入（仅 root-owned sticky 祖先例外），整条目录链禁 symlink。代理组仅需遍历/连接权限；两端锁名不同，不得复用
Worker control socket。metrics 仅允许有效 loopback TCP 或 Unix listener，禁止经代理公开。

`[http_security]` 管精确 origin、限速、Secure Cookie、请求超时和可信代理；`[session]` 管会话预算。`trusted_proxy_cidrs` 最多
64 个规范、不重复 CIDR，默认空时忽略 X-Forwarded-For；`trusted_unix_proxy` 仅用于有效 Unix listener，不放宽 TCP peer；
同源 allowlist 默认启用。会话默认空闲/绝对期限：Public 30 分钟/7 天，Admin runtime 15 分钟/8 小时。

## Cloudflare 与 Unix 上游

[Cloudflare 部署](ops/README.md#cloudflare-整站代理)不新增配置字段；Caddy 终止源站 TLS 并清洗地址头，Rust 只信任有权限连接
Unix socket 的上游，不直接解释 `CF-Connecting-IP`、`Forwarded` 或 `X-Forwarded-Proto`。现有示例默认仍用于本地 TCP 开发，
不可把关闭 Secure Cookie 或回环 origin 原样带入橙云 HTTPS 部署。以下部署示例统一使用 `ngnl:ngnl` 运行应用和拥有 socket；
账号名称不是程序限制。

Public `public.toml` 合并以下区块，其他公共/敏感引用保持不变：

```toml
# 可选区块：Cloudflare 经本机 Caddy 访问时选用 Unix listener。
[listener]
# 可选：启用 Unix 上游；默认 tcp。
mode = "unix"
# 可选：Unix 模式不使用 TCP address，保持空值。
address = ""
# 必填（Unix）：部署前建立为 ngnl:ngnl、0750 的受信目录。
runtime_directory = "/run/hv-auction"
# 必填（Unix）：直属 runtime 目录，不能与 Admin 或 Worker control 复用。
socket_path = "/run/hv-auction/public.sock"
# 必填（Unix）：复用 ngnl 组；Caddy 需具备该组权限，socket 固定 0660。
group = "ngnl"

# 可选区块：覆盖生产 HTTPS 安全参数，其余限流和超时沿用默认值。
[http_security]
# 可选：默认启用；橙云部署不得关闭精确 Origin 校验。
allowed_origins_enabled = true
# 必填（启用校验）：仅填写 Public 的精确 HTTPS origin，无路径。
allowed_origins = ["https://auction.example.com"]
# 可选：默认 false；生产 HTTPS 必须开启，不依赖转发头推导。
cookie_secure = true
# 可选：Unix 拓扑没有可信 TCP peer，保持默认空列表。
trusted_proxy_cidrs = []
# 可选：默认 false；只有受保护 Unix listener 才开启。
trusted_unix_proxy = true
```

Admin 配置了 `runtime_config_file` 时，实际配置 owner 是该 runtime 的 `[listener]` 和 **`[security]`**，不是角色文件中的
`[http_security]`。在 runtime 合并同样的 listener（socket_path 改为 `/run/hv-auction/admin.sock`），安全参数用：

```toml
# 可选区块：Admin runtime 的实际安全配置，不与 Public 共用 origin。
[security]
# 可选：默认启用；不得为代理部署关闭。
allowed_origins_enabled = true
# 必填（启用校验）：仅填写 Admin 的精确 HTTPS origin。
allowed_origins = ["https://admin.auction.example.com"]
# 可选：默认 false；生产 HTTPS 必须开启。
cookie_secure = true
# 可选：Unix 拓扑保持默认空列表。
trusted_proxy_cidrs = []
# 可选：默认 false；受保护 Unix listener 启用。
trusted_unix_proxy = true
```

无 runtime 的 Admin 则改角色文件 `[listener]`/`[http_security]`，不要创建无效的 `[security]`。公共/敏感文件职责、Admin 会话与
CSRF 不变；Unix 目录/祖先权限和 systemd 限制仍须满足上一节，Public/Admin unit 使用 `User=ngnl`、`Group=ngnl`，不再需要
额外 socket 组。目录由以下 tmpfiles 定义建立：

```text
d /run/hv-auction 0750 ngnl ngnl -
```

Caddy 保持原运行用户，在它的 systemd drop-in 设置 `SupplementaryGroups=ngnl`，不应改成以应用用户运行。共享组可以遍历目录、
连接 socket，但不能写 runtime 目录，也能访问其他 `ngnl` 组可读文件；因此敏感 TOML 保持 0600、敏感目录 0700，并严格控制该组
成员。`trusted_unix_proxy` 信任所有能连接 socket 的进程，并非按 Caddy 进程名认证。更改后须安装目录定义、重载 unit 并受控
重启才生效，文档修改不会操作主机。

## Public 实时 relay

`[realtime].consumer_key` 是稳定非敏感标识，多 Public 实例必须不同。PostgreSQL session advisory lock 拒绝重复 owner；连接失效
须停止消费并重启实例，详见[Public 故障边界](apps/auction-public-api/README.md#持锁-relay-与故障边界)。其余字段限制批次、轮询和单连接
缓冲；满缓冲淘汰慢连接，客户端须 catch-up。

## 论坛与邮件

完整字段见[Worker 模板](ops/config/examples/worker.toml)。Forum 独立循环“发布 → 回帖导入”，阶段失败不跳过后续阶段；默认
周期 300 秒、抖动最多 5 秒，分页默认 10、硬上限 20，超限失败不截断出价。`worker.forum.verify_post_body` 默认 `false`
（省略也关闭），同步发布及回帖刷新都不额外回读/逐字校验原始 BBCode；设为 `true` 才启用写前、写后及未知结果恢复的严格
正文确认，比较规则见[外部写入契约](docs/contracts/invariants.md#i-08-外部写入发生在本地提交之后)。关闭时保留真实固定帖
身份、POST 回执、幂等及人工写入门禁，但本地目标镜像不保证远端正文一致；历史未知结果不能仅凭现存帖子 ID 自动完成，
关闭时同步转人工授权、回帖刷新保留未知门禁。该开关不受 `operations.forum` 接管，修改须重启 Worker，已有人工授权门禁不
自动清空；显式填写新字段前须更新会读取 Worker 配置的程序（包括 auctionctl），旧程序严格拒绝未知字段。item timeout 须覆盖
分页/写入/间隔且小于 lease；`worker.forum.public_base_url` 仅允许无凭据/路径/查询的 HTTPS origin，回环 HTTP 仅供本地联调。
该字段默认省略，不显示 Web 地址；配置后固定 #1 的 `Auction address` 以 `[url=地址][b]地址[/b][/url]` 显示完整当期拍卖 URL，
内层粗体阻止论坛截短链接文字并改写回读 BBCode，保留 `?auction=<UUID>` 和异世界参数；`About system` 固定链接到论坛说明
主题 `301946`。修改配置需重启 Worker，该字段归部署配置所有、不受 `operations.forum` 接管；已有人工门禁任务仍须按
[论坛恢复命令](apps/auctionctl/README.md#论坛与结算核验)核对后授权，重启不清空失败状态。

MoogleMail 按 world 独立调度/唤醒。cron 默认 `*/5 * * * *`，兼容描述符、`@every`、`TZ=`/`CRON_TZ=`；时区省略时取 Worker
本地值，部署宜显式指定。共享客户端默认间隔 1 秒、请求超时 15 秒；fetch/item 预算须覆盖请求，确定性错误不 inline retry。
拉取在绝对 deadline 内返回已解析的最旧详情前缀，batch_size 仅为上限；无法完整定位 cursor/末尾即失败关闭。batch_timeout
默认 600 秒、范围 1–86400，至少覆盖一次 fetch_timeout + item_timeout，可先于全部重试到期；到期仅提交完成前缀及末封 ID，
无完成项则持久化失败退避。批次续租，token/fence 失效即取消陈旧处理。runtime 的 worlds/equipment_filter/mail_templates 须
完整同时存在或全部缺失，仅全缺失时沿用主配置兼容值。

装备 PM 不新增 TOML 字段或独立凭据 owner。forum.enabled 控制是否装配，与 mooglemail.enabled 无关；独立 PM runtime 复用
delivery.poll_interval_seconds/jitter_seconds、forum.item_timeout_seconds/lease_duration_seconds，共享 forum HTTP 限速。每轮至多
1 新发送＋1 只读核对、串行，单项总预算小于租约；代码默认轮询 60 秒不代表实际配置或到信承诺。forum.verify_post_body 不控制
PM：PM 成功始终要求完整冻结内容、recipient/sender 与 Sent MSID 精确证明。受控升级/未知结果运维见
[装备标记升级](ops/README.md#装备标记与超价提醒升级)。

## Worker 循环预算

顶层生命周期、慢速代理和结算默认 60 秒周期、5 秒抖动、各阶段批次 100、单项 30 秒；主循环默认并发 8，示例使用同值，阶段
失败不跳过后续阶段。fulfillment/recovery 独立循环默认立即执行后等待 60 秒加抖动，批次 100、并发 8、lease 60 秒、单项 30 秒；
其中等待买家支付 COD、卖家领取货款的 MM 证据读取共享一个默认 300 秒的绝对时间窗口，由
`worker.fulfillment.evidence_poll_seconds` 配置；窗口内新出现的待追踪邮件不会单独计时，而是在下一个窗口与其他邮件一起读取。

PostgreSQL delivery 独立控制领取、续租、发送与终态确认；续租间隔小于 lease，sending grace 大于完整请求超时，默认
timeout/grace 为 180/300 秒，启用 MM 时 timeout 大于两倍 fetch budget。实际并发取配置与 transport 上限较小者，MM compose
账号状态固定上限 1。论坛和邮件每种 Cookie 全局最多一条 active，由 session advisory lock 保证 Worker owner；更换或失效后重启
交接。`.keyenv` 仅由严格 loader 读取，Worker 只用 `hv-ck-key1`，`hv-ck-key2` 仅供显式隔离验证且不自动切换。

operations_alert 的 scan/dispatch/reconcile 独立运行，默认 60 秒加最多 5 秒抖动，抖动不得大于间隔；各循环失败只影响自身
重试，scan/reconcile 受 claim lease 约束，dispatch 另预留来源读取预算。其他范围和默认值以模板注释为准，修改后仍需重启。
