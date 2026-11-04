# 配置参考

[文档导航](../../docs/README.md) · [运维手册](../README.md) · [示例](examples) · [auction-config](../../crates/auction-config/README.md)

所有运行配置都是 TOML。字段的权威说明是 [`ops/config/examples/`](examples) 中每个字段上方的中文注释（标明“可选/必填”、默认值与范围）；本页说明文件结构、加载规则与各区块的用途。

## 文件结构

```text
/etc/auction/
├── public.toml          Public 角色文件
├── admin.toml           Admin 角色文件
├── worker.toml          Worker 角色文件
├── common.toml          三角色共用的非敏感配置
├── secrets.toml         敏感配置（0600）
└── runtime/
    └── admin.toml       runtime 配置（Admin 后台编辑，0640）
```

| 文件 | 拥有 |
| --- | --- |
| 角色文件 | `common_file`、`secrets_file`（必填，相对路径以角色文件所在目录解析）；可选 `runtime_config_file`；Admin 另有 `worker_config_file`；listener、tracing、HTTP 安全、会话（Public/Admin）、realtime（Public）、`[worker.*]`（Worker） |
| `common.toml` | `log_filter`、`[metrics]`、`[readiness]`、`[cache]`、`[auction.*]`（排期、早鸟、经济、结算、恢复策略）、CLI 专用 `[backup]` |
| `secrets.toml` | 数据库、Redis URL、外部 Cookie、Admin 登录凭据、三组数据密钥、OTLP 导出头、R2 备份令牌与可选打卡地址 |
| `runtime/admin.toml` | 可由 Admin 后台修改的运行参数（见下文） |

### 加载规则

- 角色文件路径：Public/Worker 的 `AppConfig::load` 先读取 `AUCTION_CONFIG_FILE`，未设置时尝试当前目录下的 `ops/config/<角色>.toml`，不存在则构造内置默认配置；正式装配仍要求数据库等依赖齐备。Admin 入口另会尝试 `auction-config/admin.toml`，仍不存在才进入首次设置。这些是运行时发现路径，仓库模板位于 `ops/config/examples/`。`auction-server` 与 `auction-cli` 使用 `--config-dir` 指定目录，不用该环境变量选择配置；`auction-server` 检测到它会拒绝启动。
- 除 `AUCTION_CONFIG_FILE` 外不存在任何环境变量覆盖；部署字段写在文件中。W7.7 的两个闭集行为开关可由数据库事实覆盖 TOML 基线，规则见下文。
- 所有角色运行配置的解析与校验错误都在启动前失败，错误不回显秘密值。CLI 专用 `[backup]` 的范围和打卡 URL 由 CLI 校验，常驻角色仅解析保留。角色、公共、普通敏感与 runtime 文件大小上限为 4 MiB；独立维护凭据另限 64 KiB。
- 三个角色必须引用同一个 `common_file`，并且同时引用同一个 `runtime_config_file`（或全部省略）；三角色数据库目标必须相同。`auction-cli config check` 会联合检查这些一致性。

## 敏感文件（`secrets.toml`）

| 字段 | 说明 |
| --- | --- |
| `[database]` `host`、`port`、`username`、`password`、`name` | PostgreSQL 连接（必填）。账号与口令按原文填写，loader 负责编码；端口必须非零。也可用兼容的 `database_url`（`postgres://…`），二者不能同时出现 |
| `redis_url` | `cache.backend = "redis"` 时必填 |
| `admin_actor`、`admin_secret` | Admin 登录凭据，必须成对；既有 secret 16–4096 字节可加载，首次设置要求至少 32 字节；无控制字符 |
| `equipment_data_key_id` / `equipment_data_key` | 装备 key 加密（手工录入、投稿）；三角色必须一致 |
| `user_login_data_key_id` / `user_login_data_key` | 用户登录 key 加密（注册回信、Admin 用户管理） |
| `credential_data_key_id` / `credential_data_key` | 外部 Cookie 加密（凭据管理、Worker 读取数据库凭据、诊断归档） |
| `mooglemail_cookie`、`ehentai_cookie` | 直接配置的 Cookie（不使用数据库凭据时） |
| `public_otlp_headers`、`admin_otlp_headers`、`worker_otlp_headers` | 各角色 OTLP 导出头 |
| `backup_heartbeat_success_url`、`backup_heartbeat_failure_url` | Worker 所引用敏感文件中的可选完整打卡地址；各自默认空，不要求成对。仅接受无 userinfo、无 fragment 的 HTTPS URL，最多 4096 字节且不含控制字符或首尾空白；`config check` 校验且不回显地址 |
| `backup_r2_access_key_id`、`backup_r2_secret_access_key` | Worker 所引用敏感文件中的 R2 令牌，成对且非空、无控制字符；异机启用时必填，只限目标 bucket 对象读写；见[异机副本](#异机副本) |

三组数据密钥都是 32 字节随机数的标准 Base64（`openssl rand -base64 32`），标识与材料必须成对，三组不能复用。密钥缺失时对应能力不可用（接口返回能力不可用，Worker 相关功能拒绝启用）。

文件必须只有 owner 可访问（建议 0600）。建库/重建用的维护凭据是另一份独立文件（模板 `ops/config/examples/secrets/postgres-rebuild.toml`），使用 `[database]` 五字段或与之互斥的兼容 `database_url`，目标库固定为 `postgres`；只供 `auction-cli db create/rebuild` 使用，常驻进程不引用。`admin_actor`、密钥、Cookie 与 OTLP headers 等根字段须写在 `[database]` 表之前，避免被 TOML 解释为数据库字段。

## common.toml

| 区块 | 用途 |
| --- | --- |
| `log_filter` | tracing 过滤器（`EnvFilter` 语法），默认 `info,<服务名>=debug`；追加 `auction_observability::log_dedup=off` 可在重启后关闭已接入调用点的重复错误去重，窗口与汇总语义见 [observability README](../../crates/auction-observability/README.md#重复错误去重) |
| `[metrics]` | 进程指标开关、转发拒绝及可选附加出口，见下文 |
| `[readiness]` | 审计失败模式、迁移兼容模式与 HTTP 就绪缓存，见[就绪策略](#就绪策略) |
| `[cache]` | 见[缓存](#缓存) |
| `[backup]` | CLI 专用定时备份策略，见[定时备份策略](#定时备份策略) |
| `[auction.schedule]` | 周排期：时区、预览/开拍/计划结束的星期与时间、参与世界 |
| `[auction.schedule.early_bird]` | 赛季早鸟期：赛季开始日期与时间、窗口天数、开拍日、开拍钟点、预览提前量、竞价时长、赛季前场次、窗口内品质、赛季前暂停收件 |
| `[auction.economics]` | 新场次的经济规则：默认起拍价、加价比例、软截止窗口与延长、代理开关/默认模式/慢速间隔/设置次数上限、Final 提前量 |
| `[auction.settlement]` | 新场次的结算策略：版本与模式（`hold_curve_v2` 或 `legacy_zero_v0`）、补偿池、isekai 载体 Credits、领先时长曲线锚点 |
| `[auction.settlement.recovery]` | 恢复策略：v1 固定 7 天、v2 固定 24 小时，`payment_timeout_seconds` 必须与版本对应；提醒次数固定 1，COD 退回上限可配。自动排期的赛季前场次覆盖为 v2 |

`metrics.enabled` 默认 false。Public/Admin 开启后，业务 listener 必须是
loopback TCP 或 Unix socket；Worker control 按原 peer 授权提供指标。
以下部署层可选字段允许 Prometheus 绕过 Unix socket，直接抓取本机出口：

| `[metrics]` 字段 | 默认与作用 |
| --- | --- |
| `public_loopback_listen_address` | 省略关闭；Public 附加指标地址 |
| `admin_loopback_listen_address` | 省略关闭；Admin 附加指标地址 |
| `worker_loopback_listen_address` | 省略关闭；Worker 附加指标地址 |
| `reject_forwarded_requests` | 默认 true；业务指标路由及三角色新出口拒绝转发的 TOML 基线；在线开关只允许收紧 |
| `sli_snapshot_enabled` | 默认 false；Worker 每六十秒只读汇总的 TOML 基线 |

有效 `enabled = true` 且填写本角色地址时才监听；地址只接受数值
`127.0.0.1` 和非零端口，拒绝主机名、通配地址、其他 IPv4 和 IPv6。
已填写但未启用的地址也必须合法，三个地址不能重复。
任一地址都不能与本角色实际业务 TCP listener 冲突，包括通配监听；
Admin 按 runtime 生效后的 listener 核对。示例端口为 19100/19101/19102。

Admin runtime 的 `[metrics]` **只覆盖 Admin 的 `enabled`**；拒绝策略基线和三个地址
始终来自 common，不进入 runtime 文档，不改变指纹或命令摘要。
Public/Worker 的 `enabled` 仍来自 common。
`metrics_reject_forwarded_requests` 可在线 `set true` 收紧、`clear` 回落此基线；
`set false` 被命令层拒绝。放宽拒绝或修改地址/`enabled` 仍须重启对应角色。

附加出口只提供 `/metrics`，共享业务指标路由的判定与响应实现；
Worker 的新出口独立于 control socket，不开放 control/健康/业务路由。
绑定或服务失败只记录固定类别 ERROR、降级采集，不影响业务 readiness。
删除对应地址并按部署窗口重启，即恢复原来的指标路径。
Worker control 的 peer UID/GID 授权不变，也不使用转发拒绝开关。

转发拒绝按头存在性覆盖所有方法，优先于 request-id 400。
精确头集合、状态码及路径见
[路由契约](../../docs/contracts/routes.md#健康与指标publicadmin)；
安装、直接采集与 SSH 隧道见[运维手册](../README.md#指标)。

经济、结算与恢复策略在**创建场次时冻结**，修改模板只影响之后创建的场次；排期控制后续计划与早鸟收件窗口，不改写已创建场次的时间与策略快照。业务含义见[业务规则](../../docs/business-rules.md)与[结算与履约](../../docs/guide/settlement.md)。

### SLI 快照

SLI 基线不由 Admin runtime 覆盖；仅 Worker 消费，且须同时启用有效 metrics。
启动时把 `sli_snapshot_enabled` 注入 W7.7，数据库 bool 开关
`metrics_sli_snapshot_enabled` 可在线覆盖；clear/到期回到当前进程的 TOML 基线。
单改 TOML 基线须重启，在线切换开关不需重启；旧程序回退须移除未知 TOML 字段。
汇总最多借一个空闲连接，忙时跳过，总预算两秒，失败保留旧指标及原年龄。
未实现历史流程标为待索引，不以零耗时代替缺测。启用、查看与回退见
[SLI 运维](../README.md#sli-基线与容量观察)，完整定义见
[SLI 口径](../../crates/auction-observability/README.md#sli-与容量基线)。

### 就绪策略

`common.toml [readiness]` 是三角色共同引用的启动快照，全部字段均可省略，不由 Admin runtime 覆盖；修改文件基线仍须重启，遵守适用的停机门禁；数据库开关覆盖后续行为，无需重启。枚举值非法时拒绝加载，不回显 TOML 原文。迁移兼容模式仍只在启动时读取。

| 字段 | 默认值 | 语义 |
| --- | --- | --- |
| `audit_failure_mode` | `"shadow"` | `legacy`：任一审计失败即粘滞未就绪，不计算影子；`shadow`：沿用旧就绪与全部失败心跳计数，同时比较分类门禁并记录差异；`classified`：只由 permanent + unknown 失败粘滞阻断，心跳 `audit_failure_count` 也只投影这两类累计 |
| `migration_compatibility_mode` | `"shadow"` | `strict`：完整迁移历史必须恰好匹配，不读兼容登记；`shadow`：实际仍严格，在同一只读快照计算兼容判定并记录差异；`compatible`：显式允许全部未来后缀已登记兼容的数据库，精确定义见[数据库契约](../../docs/contracts/database.md#迁移兼容登记与就绪) |
| `cache_enabled` | `true` | Public/Admin 最终就绪布尔值在进程内正负均缓存 1 秒，并发共享一次限时 750ms 的检查；`false` 逐次执行原检查，不合并、不应用该 750ms 预算。Worker control 不使用该缓存 |

瞬时白名单保留决策 32 的 SQLSTATE `40001`、`40P01`、`57014`、`55P03`、整个 `08` 类、sqlx `PoolTimedOut` 及连接通信失败 `Io`，依决策 71 另外加入 `53300`、`57P01`、`57P02`、`57P03`；`57P05`、`25P03/25P04` 仍按未知粘滞阻断。未知错误是未知风险处置，不代表已证明不可重试；与不可重试失败一样保持粘滞，不因后续成功清零。分类只影响健康信号，不扩大事务或外部写入重试范围。完整分类由 [auction-storage](../../crates/auction-storage/README.md#审计失败与就绪) 拥有。

切换到 `classified` 前，影子观察中的差异应只出现在 `transient`（预期差异），`permanent` / `unknown` 无差异；没有样本不能解释为通过。观察结果由用户审阅并决定切换，不自动推进。步骤与指标见[运维手册](../README.md#审计就绪影子观察)。可经运行时开关覆盖为 `legacy`、`false` 回退；clear 只回到当前进程的 TOML 基线。改文件基线后仍须受控重启；单独关闭指标或日志不改变就绪计数。

迁移模式平时保持 `shadow`，差异只允许为 `compatible_ahead`；同版数据库没有超前样本，不能据零差异宣布超前分支已验证。换回已含 W7.8a 的旧程序前，运维必须显式设置 `migration_compatibility_mode = "compatible"`，按[程序回退](../deploy/RECOVERY.md#程序回退)完成区间核对并受控重启。设置 `strict` 并重启恢复原迁移判定；不回退数据库，也不放宽 CLI 的迁移、备份、恢复或盘点边界。数据库检查自身总预算为 750ms，独立于 HTTP 缓存开关，对 heartbeat/supervisor 也生效。

### 定时备份策略

`common.toml [backup]` 全部字段可省略；只有 `backup scheduled` 和 CLI `config check` 消费、校验运维语义，Public/Admin/Worker 不据此启动任务，也不应用这些范围校验。不放进 Admin runtime。启用与回退分别是用户启用与停用 timer，没有额外 enabled 开关；每次 CLI 启动重新读取配置。

| 字段 | 默认值 | 范围 |
| --- | --- | --- |
| `local_retention_count` | `7` | `1–365`，本地已完成定时备份份数 |
| `min_free_bytes` | `1073741824`（1 GiB） | `67108864–1099511627776`（64 MiB–1 TiB） |
| `min_free_percent` | `10` | `1–50`，与字节水位同时满足 |
| `timeout_seconds` | `7200` | `60–7200`，自本次启动起的任务预算 |

目录和数据库名仍是 CLI 参数。scheduled 只加载 Worker 及其引用文件，再调用 `validate_backup()`；不依赖 Public/Admin 的联合预检。`config check` 继续联合检查并报告普通 `backup` 参数及 Worker 两个地址的配置标志，`config show` 同样只投影这些安全信息。打卡客户端一次 HTTPS GET，连接三秒、请求五秒，不重试、不使用环境代理、重定向或 Cookie；任何打卡失败不改变备份结果，发送结果只写 stderr/systemd journal，终态回执只保存 `heartbeat_configured`。外部服务按决策暂缓启用，敏感示例保持注释/空值，地址不得进入日志或回执。启用、回执和故障处置见[备份与恢复手册](../deploy/RECOVERY.md#定时备份)。

#### 异机副本

`common.toml [backup.offsite]` 同样只由 CLI 消费，默认关闭；`enabled = false` 在下次 scheduled 生效，暂停加密、上传和旧待上传项的重试，不影响本地备份或角色进程。重新启用后继续原待上传项。启用前必须完成[离线密钥托管与 R2 设置](../deploy/RECOVERY.md#异机副本前置准备)，特别是 daily/monthly 的 bucket lock；CLI 不代替用户检查或修改控制台策略。

| 字段 | 默认值 | 校验 |
| --- | --- | --- |
| `enabled` | `false` | 为 true 时 `age_recipient`、`endpoint`、`bucket` 及成对 R2 凭据必填；`prefix` 可空，空表示 bucket 根下直接使用 `daily/`、`monthly/` 等固定布局 |
| `age_recipient` | `""` | age 原生 X25519 公钥，62 字节、小写 `age1` 与 bech32 字符集；加密由主机 age 校验，不提供私钥或口令字段 |
| `endpoint` | `""` | HTTPS 根地址，无 userinfo、query、fragment 或额外路径 |
| `bucket` | `""` | 3–63 字节的小写字母、数字或连字符，首尾为字母/数字；拒绝保留名称 |
| `prefix` | `""` | 可选；最多 512 字节的字母、数字、`-_.` 与 `/`，无首尾斜杠、空段、`.` 段或 `..`；空串表示不添加前缀 |

Worker 引用的敏感 TOML 根字段 `backup_r2_access_key_id` 与 `backup_r2_secret_access_key` 必须成对、非空、不超过 4096 字节，无首尾空白或控制字符；禁用时可以全部省略。它们由 `SecretString` 保存，仅经显式 getter 交给 R2 适配器，不能放在 common、角色或 runtime 文件。公钥、endpoint、bucket 和 prefix 是普通配置。`config check` 校验启用依赖及字段格式，但不执行 age、不连接 R2；`config check` 与 `config show` 对异机字段只输出 enabled 及是否配置，绝不输出字段值、令牌或签名 URL。错误只有字段名与固定原因。轮换令牌不改变待上传身份；更换公钥或目的地后，哈希不匹配的旧项保留并记 pending_destination_changed 告警，新目标正常继续备份。旧任务不能自动改投新目标，需由用户在原配置下处理或将原状态与密文整体隔离保留。

## 角色文件

### Public / Admin

| 区块 | 用途 |
| --- | --- |
| `[listener]` | `mode = "tcp"`（`address`，默认 Public `127.0.0.1:8090`、Admin `127.0.0.1:8091`）或 `mode = "unix"`（`runtime_directory`、`socket_path`、`group`）。Unix 运行目录及其祖先必须由 root 或运行账号拥有、禁止 group/other 写入、无符号链接 |
| `[http_security]` | origin allowlist（`allowed_origins_enabled`、`allowed_origins` 精确 origin，最多 16 个）、登录/写请求限流与窗口、`cookie_secure`（生产必须 true）、请求超时、`trusted_proxy_cidrs`（最多 64 个规范 CIDR）、`trusted_unix_proxy`（仅 Unix listener） |
| `[session]` | Public：`max_public_sessions`（默认 5）。Admin：`idle_ttl_seconds`（默认 900）、`absolute_ttl_seconds`（默认 28800） |
| `[tracing]` | OTLP/HTTP 导出：开关、endpoint、批量与超时、队列容量、采样比例（百万分率） |
| `[realtime]`（Public） | `consumer_key`（每个实例唯一）、轮询间隔、每页流数与事件数、每连接缓冲 |

Admin 配置了 runtime 文件时，listener、HTTP 安全、会话、`metrics.enabled`
与 tracing 以 runtime 对应区块为准；metrics 的拒绝策略与附加地址始终来自 common。

### Worker

| 区块 | 用途 |
| --- | --- |
| `[worker]` | 拍卖主循环：并发、轮询间隔、批次上限、抖动、单项超时、恢复重试间隔 |
| `[worker.delivery]` | 投递 outbox：批次、并发、轮询、租约与续租、发送宽限、请求与收尾超时 |
| `[worker.fulfillment]` | 结算履约：批次、并发、轮询、租约、超时、重试间隔、证据轮询间隔 |
| `[worker.recovery]` | 恢复：批次、并发、轮询、租约、超时、低余额阈值、健康刷新与证据过期 |
| `[worker.control]` | Worker control：开关、socket 组、`allowed_peer_uids`/`allowed_peer_gids`（启用时至少一项）、运行目录与 socket 路径 |
| `[worker.forum]` | 论坛同步：开关、凭据 scope、公开站点 origin、板块 ID、轮询、页数上限、请求间隔与超时、租约 |
| `[worker.mooglemail]` | 邮件：开关、邮箱标识与世界、邮件默认起拍价、请求边界、收件与维护 cron、观察间隔与隔离开关、抖动、租约、重试、发件人白名单、`keyenv_file`（本地联调） |
| `[worker.mooglemail.send_intervals]` | 各类出站邮件发送起点的最小间隔 |
| `[worker.mooglemail.diagnostics]` | 诊断归档：开关、目录（生产须为可写目录）、保留天数、总字节上限 |
| `[worker.mooglemail.equipment_filter]` | 投稿过滤：品质门槛（始终生效）、前后缀/分类/类型/等级过滤 |
| `[worker.mooglemail.mail_templates]` | 出站邮件正文模板（只能使用规定占位符） |
| `[[worker.mooglemail.additional_worlds]]` | 额外世界的邮箱（独立游标、租约与白名单） |
| `[worker.operations_alert]` | 运维告警：开关、收件人与世界、扫描间隔、批次窗口、冷却、各类积压阈值 |

各循环的预算关系按其工作方式分别校验，例如 fulfillment/recovery 的单项超时必须小于租约，delivery 依靠续租允许发送请求跨越初始租约，MoogleMail 则另校验读取、处理与整批预算。不能把所有循环都按“请求超时 < 租约”配置。启用外部功能时的依赖条件：

- MoogleMail：需要凭据来源（数据库凭据 + `credential_data_key`，或 `keyenv_file`，或 `mooglemail_cookie`）以及 `user_login_data_key`。
- 论坛：需要 `ehentai_cookie` 或 `credential_data_key`。
- 运维告警：需要收件人，告警世界必须在已启用的 MoogleMail 世界内。

#### MoogleMail 收件与维护

以下字段只属于 `worker.toml` 的 `[worker.mooglemail]`，不由 Admin 参数界面接管。
路径隔离可在运行时切换；维护日程和观察间隔仍按启动配置生效：

| 字段 | 默认值与范围 | 行为 |
| --- | --- | --- |
| `poll_isolation_enabled` | `true` | 在线开关基线；true 独立收件与维护，false 恢复每轮三分支 join |
| `maintenance_schedule` | 省略；接受与 `schedule` 相同的标准 cron、描述符、时区前缀和 `@every` | 省略时跟随实际生效的收件 `schedule`，包括 Admin 托管邮件参数对该字段的覆盖；显式配置时使用独立维护日程 |
| `read_mail_observation_interval_seconds` | `3600`，范围 `60–86400` | 完整已读列表纯计数观察的最小间隔；首次观察延后该间隔，只在每次 `sweep` 结束后检查，实际间隔向上取整到维护节奏；失败也消耗本次间隔 |

两个世界同时启用时，启动时从实际收件日程的首两个未来触发时刻计算一次相位：
`phase = min(150 秒, 两次触发间隔 / 2)`。main 使用零相位，isekai 使用该相位；
单世界部署使用零相位。无论基线是否隔离开启，装配都预构建无相位与带相位两套
收件日程并冻结相位，供在线切换直接选择。同一世界的收件和维护都按
`schedule.next_after(now - phase) + phase` 平移整条日程，再叠加原
`jitter_seconds`，不会因直接给下一触发点加延迟而漏掉尚未到来的偏移时刻。
`@every` 是相对节奏，在该公式中自然抵消相位，不另加错峰分支。
默认五分钟 cron 下，两个世界的名义触发点相差 150 秒。

收件唤醒与持久邮箱下一次 due 使用同一平移日程；租约、游标、命令资格与业务完成仍依赖数据库事实。手动触发立即唤醒收件，不受相位影响，也不唤醒独立维护。维护和纯观察分别使用原 `fetch_timeout_seconds` 的总截止点，各阶段内的数据库等待、凭据排队与远端操作不能重置期限。`maximum_pages`、完整列表检查、恢复及发送证明保持原语义；低频纯观察只记录完整列表数量，不创建历史入站命令。

在线切换使用 `auction-cli switch set mooglemail_poll_isolation_enabled false`，
clear 回落此 TOML 基线；无需重启。切换在下一轮收件调度或下一次维护门复读后生效，
在途轮次按开始时模式完整结束。旧路径恢复每轮 Read Mail 观察、维护与收件的
三分支 `tokio::join!`、原观察频率和无相位日程；维护任务保持 idle 而不退出，
两个循环共用每世界互斥门。进入隔离时观察计时重置为当前时刻，
睡眠按不超过五秒切片复读开关，不重复叠加 jitter 或重置同一模式的触发点。
启动时始终解析并校验 `maintenance_schedule`，即使基线为 false；
在线切换不重写 `mail_poll_state.next_poll_at`，首个新模式收件可继续等待旧 due。
显式维护日程与观察间隔的修改仍需重启 Worker。

#### MoogleMail 发送前基线

`[worker.mooglemail] sent_baseline_mode` 可选，默认 `"shadow"`；只接受 `full`、`shadow`、`watermark`，未知值在配置解析时拒绝。该字段只属于 Worker TOML，不进入 Admin runtime 参数或 W7.7 在线开关；修改需要重启 Worker，并受部署时段门禁（W7.5）约束。

| 值 | 行为 |
| --- | --- |
| `full` | 完整 Sent 排除集及其十分钟有效缓存，保留旧路径 |
| `shadow` | 旧路径唯一裁决；复用同一读取轮次的页面，只读比较水位的新增 MID 集合与列表候选集合；不增加 POST |
| `watermark` | 每次发送前 fresh 读完最新分钟组，保存最大数值 MID 与该分钟；发送后核对原锚点并读完边界分钟，按 `MID > W` 取得候选，再执行原完整详情和 compose 证明 |

边界随 prepared 句柄一次消费，不新增持久水位表，不推进 Worker 游标。Empty 需要空列表及末页证明。仅水位路径要求 MID 为无前导零、可由 `u128` 受检解析的十进制身份，通用 opaque ID 解析不变；还要求同一账号与世界代次内 MID 分配单调且不复用，Sent 时间排序与之相容，同分钟行允许任意排列。数字空洞不算不连续。凭据、world、endpoint 或 owner 代次变化立即使上下文与旧完整缓存失效。

写前边界无效时在原预算内回退 fresh 完整扫描；写后锚点消失时完整枚举并使用原边界，不以发送后最大 MID 重建基线。写后 MID/时间前提失效且缺少发送前完整集合时保持 `write_unknown`。完整回退仍受 `maximum_pages`（1–100）和原 `fetch_timeout_seconds` 截止点、限速及资源限制；失败关闭，未知结果绝不重发。常规轮 Sent/Read Mail 水位及每日校验不在本切片内。

默认 shadow 沿用旧路径，不能解除 D3；须在旧完整扫描触顶前由用户完成观察并切换 watermark。核实 MID 语义、观察周期、分支样本及差异口径见[运维手册](../README.md#sent-发送基线影子观察)。回退为 `full` 并受控重启；换回不认识该字段的旧二进制前须移除新字段。

## runtime 配置（`runtime/admin.toml`）

runtime 文件让运营人员在 Admin “参数配置”页修改部分参数，而无需登录服务器编辑 TOML。Admin 是唯一写者：提交的候选先经服务端完整校验，再以命令账本 + 原子替换写入（临时文件、rename、目录同步、独立锁文件）。文件必须是 0640、父目录禁止他人写入、归 Admin 运行账号所有。**修改在重启相关角色后生效**，不改写已有场次与任务。

| 区块 | 用途 | 生效 |
| --- | --- | --- |
| `allow_manual_early_open`、`allow_manual_early_settlement` | 是否允许 Admin 提前开拍 / 提前结算 | 重启 Admin |
| `bot_players` | Bot 角色名列表（登录页注册提示、内部履约） | 重启 Public、Worker |
| `[worlds.main]`、`[worlds.isekai]` | 各世界收件开关与发件人白名单 | 重启 Worker |
| `[equipment_filter]` | 投稿装备过滤 | 重启 Worker |
| `[equipment_colors]` | 论坛装备名标色规则；同时决定公开站点的“稀有/融合”分组（Peerless 或首条命中规则实际上色的为稀有） | 重启三角色 |
| `[mail_templates]` | 出站邮件正文 | 重启 Worker |
| `[[dashboard_mailboxes]]` | 仪表盘需要核对的世界与邮箱 | 重启 Admin |
| `[equipment_source]` | Admin 手工录入时读取装备详情：允许的主机与路径、请求边界、是否要求可交易 | 重启 Admin |
| `[listener]`、`[metrics]`、`[security]`、`[session]`、`[tracing]`、`[worker_control]` | Admin 自身的监听、指标 enabled、HTTP 安全、会话、tracing 与 control socket 路径；指标转发拒绝开关只在 common 文件中配置 | 重启 Admin |
| `[operations.*]` | 接管部署参数（见下） | 见下 |

### operations 接管

`[operations.<组>]` 中 `managed = true` 时，该组参数覆盖部署配置中的对应值；`managed = false`（默认）时沿用 `common.toml` 与角色文件。

| 组 | 覆盖 | 生效 |
| --- | --- | --- |
| `economics` | 新场次经济模板（`[auction.economics]`） | 重启 Admin 与 Worker |
| `schedule` | 周排期与早鸟期（`[auction.schedule]`） | 重启 Admin 与 Worker |
| `forum` | 论坛同步参数 | 重启 Worker |
| `alerts` | 运维告警参数 | 重启 Worker |
| `worker`（含 `delivery`/`fulfillment`/`recovery`） | Worker 各循环的调度边界；不覆盖 `worker.fulfillment.evidence_poll_seconds` | 重启 Worker |
| `mail`（含 `send_intervals`） | 邮件轮询计划与请求边界、发送间隔；不包含全局开关、endpoint、凭据、邮箱身份与诊断归档路径 | 重启 Worker |

启用接管时，`config check` 还会核对 Admin 与 Worker 得到的有效模板与排期一致。

## 缓存

`[cache]`（`common.toml`）：

| 字段 | 说明 |
| --- | --- |
| `backend` | `disabled`（默认）或 `redis`；`redis` 需要在敏感文件提供 `redis_url`，且程序以 `cache-redis` feature 构建 |
| `public_rules_ttl_seconds` | Public 规则读模型的缓存 TTL，1–3600 秒，默认 30 |

缓存只用于可丢弃的读模型（目前是 Public 规则文档），永远不参与金额、资格、幂等、会话或审计判断；缓存不可用时直接回源数据库。规则发布后 Admin 与 `auction-cli rules import` 会主动失效缓存。

## 权限要求

| 对象 | 要求 |
| --- | --- |
| 配置目录、角色文件、`common.toml` | 不能被 group/other 写入 |
| `secrets.toml` | 只有 owner 可访问 |
| runtime 文件 | 恰好 0640；父目录不能被 group/other 写入、不能是符号链接 |
| 维护凭据 | 规范化绝对路径、非符号链接、单硬链接、归当前账号且仅 owner 可读 |

`auction-cli config check` 在非严格模式下把普通文件和敏感文件的权限问题列为警告，`--strict` 视为错误；runtime 文件及父目录的权限由 loader 强制检查，维护凭据也有独立的强制检查，不受此开关放宽。生产部署必须通过严格检查。

## 示例与首次设置

`ops/config/examples/` 中的文件同时是首次设置向导的生成模板（编译期内嵌）。因此：

- 每个赋值字段上方必须紧邻含“可选”或“必填”的中文注释（`ci/checks/check.sh config` 检查）。
- 修改示例后，向导生成的三角色配置必须仍能通过 loader 校验。
- 示例中的值仅用于演示；`allowed_peer_*`、origin、密钥等必须按实际部署填写。

首次设置的目标配置目录必须尚不存在；其直接父目录须预先建立，为真实目录（非符号链接），归当前运行账号或 root 所有，且禁止 group/other 写入。这是生成器的强制门禁，不受 `config check --strict` 控制。不能直接把目标放在 `/tmp` 下；如需临时生成，先在其中建立归运行账号所有、权限 0700 的私有父目录，再把目标放在该私有目录内。

## 排空与停止预算

三个角色各自在非敏感角色 TOML 中拥有 `[shutdown]`；不属于 `common.toml`，不被 Admin runtime 覆盖，没有环境变量覆盖。省略区块或字段时使用角色默认值。

| 字段 | Public/Admin | Worker | 生效 |
| --- | --- | --- | --- |
| `drain_timeout_seconds` | 默认 10，范围 1–300 | 默认 10，范围 1–300 | 重启 |
| `critical_write_timeout_seconds` | 只接受 0，默认 0 | 默认 60，范围 0–300；0 完整选择旧排空路径 | 重启 |

Worker 新路径在首次停止时关闭入口，立即取消尚未登记交接的准备期工作并保留 claim 由租约恢复。delivery Send（含装备出库/解锁、附件和 COD）、Take/Return、discard 和 PM 的围栏调用开始即受保护：调用前登记，已关闭则不发起围栏；登记成功后交接与关键段同样计入活动、阻止排空结束，仍须确认围栏提交才允许外写。论坛在已有持久 claim 下的首个实际 POST 前登记。全部已登记活动共享一次关键段截止，结果按原端口提交后才 complete，其余 drop 只记录未收口。运维告警发送复用 delivery，不启用任何暂缓的告警通道。之后执行普通资源收口。默认最多 60 + 10 秒，既有请求、续租、凭据所有权和证据截止不延长。第二次信号立即强制收口、非零退出并跳过可选 tracing 刷新；预算耗尽同样非零退出，不在关闭路径补写持久结果。

`critical_write_timeout_seconds = 0` 配合 `drain_timeout_seconds = 10` 完整恢复旧排空路径，包括 PM 随主 runner 收口的旧策略；只把 60 改成 10 仍是新路径。切换需要重启，并应遵守维护窗口。换回不识别 `[shutdown]` 的旧二进制时同时恢复旧角色配置，避免未知字段校验失败。

外层停止上限须大于“普通 + 关键段 + tracing 上限 + 余量”。示例 Public/Admin unit 为 45 秒，Worker 为 120 秒；tracing 合法上限为 30 秒。`auction-server` 从各 RolePlan 派生 `普通 + 关键段 + 30 + 5` 秒并求和，默认 45 + 45 + 105 = 195 秒；server unit 为 225 秒，另覆盖三次各 5 秒强制回收与余量。CLI 每次停止读取有限的 `TimeoutStopUSec`，客户端等待该值再加 10 秒；stack unit 为 330 秒，覆盖三个角色、属性查询与回执余量。提高应用预算须同步提高对应 unit、server 与 stack 上限；新程序不会修改已安装的主机 unit。

服务 owner 回收后仍监听终止信号；tracing 刷新期间再收到信号时，装配根直接非零退出，避免不可取消的刷新线程延迟强退。

## 数据库运行时开关

这是部署字段来自 TOML 的明确行为覆盖例外，不引入环境变量覆盖，
也不扩展 AdminRuntimeDocument。已知名称及基线如下：

| 名称 | 值 | 消费角色 / TOML 基线 |
| --- | --- | --- |
| readiness_audit_failure_mode | legacy/shadow/classified | 三角色 / readiness.audit_failure_mode |
| readiness_cache_enabled | bool | Public/Admin / readiness.cache_enabled |
| metrics_sli_snapshot_enabled | bool | Worker / metrics.sli_snapshot_enabled |
| mooglemail_poll_isolation_enabled | bool | Worker / worker.mooglemail.poll_isolation_enabled |
| metrics_reject_forwarded_requests | bool | 三角色 / metrics.reject_forwarded_requests |

优先级为最新且未过期的数据库 set → 启动时 TOML 基线。
clear、expire、到期或未设置均回落基线；修改 TOML 不清除无期限覆盖。

三角色五秒刷新，单次一秒预算；首次失败保留基线，后续失败保留最后成功快照，
但仍按原数据库期限回落。默认基线仍为 shadow/true；切 classified 仍需用户
审阅影子样本。切换不清零审计累计；策略版本变化立即使旧 HTTP 就绪缓存失效。
Worker control 不使用 HTTP 就绪缓存。邮件隔离开关在 Worker 下一轮调度或维护门
复读后生效；指标转发拒绝在每次 `/metrics` 请求边界读取，且只允许在线收紧，
`set false` 被命令层拒绝。`migration_compatibility_mode`、排空预算与 W7.10 的
runtime 文件预算热读取仍不在本批范围。操作与 stale 诊断见
[运维手册](../README.md#运行时开关)。

## 数据库连接分档

公共文件的 `[database_profiles] mode` 为 `profiled`（默认）或 `legacy`。
未知模式在 TOML 解析时拒绝；连接参数只在建连时应用，修改须重启。
重启受 W7.5 停机门禁约束，本批不接入 W7.7。
长期凭据与 realtime 会话已借出，修改池选项不能在线改写它们；
强制轮换会释放所有权锁，因此不支持靠替换池实现无重启切换。

`profiled` 按代码中的用途枚举指定 `application_name`，不开放任意名称。
三服务名称为 `auction-public`、`auction-admin`、`auction-worker`。
CLI 名称按入口区分，见 [CLI 连接用途](../../apps/auction-cli/README.md#数据库连接用途)。
关闭服务超时时仅设置服务名称，沿用 30 秒 acquire 与服务器既有会话设置。
`legacy` 固定池上限 16、acquire 30 秒，不显式设置名称或任何会话参数。
两种模式均保留原池指标及新增事务观测，指标关闭不改变业务结果。

三个角色文件各有 `[database_pool]`：

- `max_connections` 默认 16，表示物理池总上限，包含长期 owner。
  Public 预留 1，Admin 预留 0，Worker 按最坏情况预留 3；
  扣除后至少剩 1 个普通连接。Worker 的 3 个分别为邮件、论坛与 LISTEN。
- `timeouts_enabled` 默认 false；与 `mode = "legacy"` 同时启用会失败。
- `acquire_timeout_millis`、`statement_timeout_millis`、
  `lock_timeout_millis` 均无隐式默认值，启用时必须全部显式填写。
  候选值分别为 2000、5000、1000，不能将候选值理解为默认启用值。
  三者范围为 1–120000ms，且 lock 必须短于 statement。
- Public/Admin 的 acquire 必须严格短于有效 HTTP `request_timeout`；
  Admin 在 runtime 覆盖完成后校验。Worker 无 HTTP 请求期限。
  这不保证多条语句和多次事务尝试的总耗时小于 HTTP 期限。
- `idle_in_transaction_session_timeout_millis` 默认 0，范围 0–3600000ms。
  服务超时开关关闭时不写入；开启后 0 表示禁用该单项。
  非零可能产生 `25P03`；按决策 71 仍归 unknown，可能粘滞阻断 readiness，
  必须由用户另行决定，不能因 W0.14 已部署就自动启用。

服务超时仅在以下条件全部满足后由用户启用：W0.14 已部署；
按决策 72 观察完整 Final 与结算周期，差异仅在预期瞬时类别；
三角色有效 readiness 策略均为 `classified`；指标可采集并查看。
配置加载不会查询数据库来证明这些线上前置条件。
回退顺序为先关闭服务超时并重启，再考虑切回 readiness 旧模式。

维护策略位于公共文件的 `[database_profiles.maintenance]`：
`max_connections` 默认 4，`acquire_timeout_millis` 默认 30000。
profiled 维护档的基础 `statement_timeout` 显式为 0，不套用服务档短超时。
已有 owner 限界保留：建库/重建 30 秒语句与 5 秒锁等待；
恢复预检/保留盘点 5 秒语句与 1 秒锁等待，以及证据事务的局部限界。
池的 `after_connect` 与直连复用固定 GUC 名、绑定值的单次 `set_config` 初始化；
设置失败的连接不交给 owner。驱动 UTC、TLS 与目标解析规则不变。
`SET LOCAL` 在事务内覆盖会话值，事务结束后恢复，并非自动取最小值。

部署预算位于公共文件的 `[database_profiles.budget]`，均为规划声明：

- `max_connections = 100`。
- `superuser_reserved_connections = 3`，`reserved_connections = 0`；
  reserve 初值待用户核实线上，工具不宣称已经验证服务器值。
- `max_concurrent_cli = 2`，只声明预算并行数，不强制调度 CLI 进程。
- `external_reserved_connections = 8`，覆盖独立 guard、外部备份程序、
  exporter 与人工运维会话；不是从服务池再次扣减长期 owner。

`auction-cli config check` 汇总一组 Public/Admin/Worker 的实际池上限，
加上并行 CLI 池与外部预留，和扣除服务器 reserve 后的普通连接上限比较。
profiled 超预算失败，legacy 只展示预算，保留旧路径。
每增加实例都要另计服务池，不能把一个配置目录的核对当作全主机发现。
`config show` 仅输出非敏感配置和实际池上限，不输出 URL 或凭据。
