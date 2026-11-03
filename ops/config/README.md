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
| `common.toml` | `log_filter`、`[metrics]`、`[cache]`、`[auction.*]`（排期、早鸟、经济、结算、恢复策略）、CLI 专用 `[backup]` |
| `secrets.toml` | 数据库、Redis URL、外部 Cookie、Admin 登录凭据、三组数据密钥、OTLP 导出头、R2 备份令牌与可选打卡地址 |
| `runtime/admin.toml` | 可由 Admin 后台修改的运行参数（见下文） |

### 加载规则

- 角色文件路径：Public/Worker 的 `AppConfig::load` 先读取 `AUCTION_CONFIG_FILE`，未设置时尝试当前目录下的 `ops/config/<角色>.toml`，不存在则构造内置默认配置；正式装配仍要求数据库等依赖齐备。Admin 入口另会尝试 `auction-config/admin.toml`，仍不存在才进入首次设置。这些是运行时发现路径，仓库模板位于 `ops/config/examples/`。`auction-server` 与 `auction-cli` 使用 `--config-dir` 指定目录，不用该环境变量选择配置；`auction-server` 检测到它会拒绝启动。
- 除 `AUCTION_CONFIG_FILE` 外不存在任何环境变量覆盖；任何字段都只能写在文件中。
- 所有角色运行配置的解析与校验错误都在启动前失败，错误不回显秘密值。CLI 专用 `[backup]` 的范围和打卡 URL 由 CLI 校验，常驻角色仅解析保留。角色、公共、普通敏感与 runtime 文件大小上限为 4 MiB；独立维护凭据另限 64 KiB。
- 三个角色必须引用同一个 `common_file`，并且同时引用同一个 `runtime_config_file`（或全部省略）；三角色数据库目标必须相同。`auction-cli config check` 会联合检查这些一致性。

## 敏感文件（`secrets.toml`）

| 字段 | 说明 |
| --- | --- |
| `[database]` `host`、`port`、`username`、`password`、`name` | PostgreSQL 连接（必填）。账号与口令按原文填写，loader 负责编码；端口必须非零。也可用兼容的 `database_url`（`postgres://…`），二者不能同时出现 |
| `redis_url` | `cache.backend = "redis"` 时必填 |
| `admin_actor`、`admin_secret` | Admin 登录凭据，必须成对；secret 16–4096 字节、无控制字符 |
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
| `log_filter` | tracing 过滤器（`EnvFilter` 语法），默认 `info,<服务名>=debug` |
| `[metrics]` | `enabled`：是否提供 `/metrics`；Public/Admin 开启时 listener 必须是 loopback 或 Unix socket |
| `[cache]` | 见[缓存](#缓存) |
| `[backup]` | CLI 专用定时备份策略，见[定时备份策略](#定时备份策略) |
| `[auction.schedule]` | 周排期：时区、预览/开拍/计划结束的星期与时间、参与世界 |
| `[auction.schedule.early_bird]` | 赛季早鸟期：赛季开始日期与时间、窗口天数、开拍日、开拍钟点、预览提前量、竞价时长、赛季前场次、窗口内品质、赛季前暂停收件 |
| `[auction.economics]` | 新场次的经济规则：默认起拍价、加价比例、软截止窗口与延长、代理开关/默认模式/慢速间隔/设置次数上限、Final 提前量 |
| `[auction.settlement]` | 新场次的结算策略：版本与模式（`hold_curve_v2` 或 `legacy_zero_v0`）、补偿池、isekai 载体 Credits、领先时长曲线锚点 |
| `[auction.settlement.recovery]` | 恢复策略：v1 固定 7 天、v2 固定 24 小时，`payment_timeout_seconds` 必须与版本对应；提醒次数固定 1，COD 退回上限可配。自动排期的赛季前场次覆盖为 v2 |

经济、结算与恢复策略在**创建场次时冻结**，修改模板只影响之后创建的场次；排期控制后续计划与早鸟收件窗口，不改写已创建场次的时间与策略快照。业务含义见[业务规则](../../docs/business-rules.md)与[结算与履约](../../docs/guide/settlement.md)。

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
| `enabled` | `false` | 为 true 时下面四项及成对 R2 凭据必填 |
| `age_recipient` | `""` | age 原生 X25519 公钥，62 字节、小写 `age1` 与 bech32 字符集；加密由主机 age 校验，不提供私钥或口令字段 |
| `endpoint` | `""` | HTTPS 根地址，无 userinfo、query、fragment 或额外路径 |
| `bucket` | `""` | 3–63 字节的小写字母、数字或连字符，首尾为字母/数字；拒绝保留名称 |
| `prefix` | `""` | 最多 512 字节的字母、数字、`-_.` 与 `/`，无首尾斜杠、空段、`.` 段或 `..` |

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

Admin 配置了 runtime 文件时，listener、HTTP 安全、会话、metrics 与 tracing 以 runtime 文件中的对应区块为准。

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

以下字段只属于 `worker.toml` 的 `[worker.mooglemail]`，不由 Admin 参数界面接管，修改后需要重启 Worker：

| 字段 | 默认值与范围 | 行为 |
| --- | --- | --- |
| `poll_isolation_enabled` | `true` | 每个世界独立运行收件与维护；维护先执行 Sent/Take 恢复和 compose 清理，再检查 Read Mail 纯观察是否到期 |
| `maintenance_schedule` | 省略；接受与 `schedule` 相同的标准 cron、描述符、时区前缀和 `@every` | 省略时跟随实际生效的收件 `schedule`，包括 Admin 托管邮件参数对该字段的覆盖；显式配置时使用独立维护日程 |
| `read_mail_observation_interval_seconds` | `3600`，范围 `60–86400` | 完整已读列表纯计数观察的最小间隔；首次观察延后该间隔，只在每次 `sweep` 结束后检查，实际间隔向上取整到维护节奏；失败也消耗本次间隔 |

两个世界同时启用且隔离开启时，启动时从实际收件日程的首两个未来触发时刻计算一次相位：`phase = min(150 秒, 两次触发间隔 / 2)`。main 使用零相位，isekai 使用该相位；单世界部署使用零相位。同一世界的收件和维护都按 `schedule.next_after(now - phase) + phase` 平移整条日程，再叠加原 `jitter_seconds`，不会因直接给下一触发点加延迟而漏掉尚未到来的偏移时刻。`@every` 是相对节奏，在该公式中自然抵消相位，不另加错峰分支。默认五分钟 cron 下，两个世界的名义触发点相差 150 秒。

收件唤醒与持久邮箱下一次 due 使用同一平移日程；租约、游标、命令资格与业务完成仍依赖数据库事实。手动触发立即唤醒收件，不受相位影响，也不唤醒独立维护。维护和纯观察分别使用原 `fetch_timeout_seconds` 的总截止点，各阶段内的数据库等待、凭据排队与远端操作不能重置期限。`maximum_pages`、完整列表检查、恢复及发送证明保持原语义；低频纯观察只记录完整列表数量，不创建历史入站命令。

回退时设置 `poll_isolation_enabled = false` 并重启 Worker：恢复每轮 Read Mail 观察、维护与收件的三分支 `tokio::join!`、原观察频率和无相位日程，不启动独立维护任务，两个新节奏参数不生效。重启须遵守部署时段门禁。

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
| `[listener]`、`[metrics]`、`[security]`、`[session]`、`[tracing]`、`[worker_control]` | Admin 自身的监听、指标、HTTP 安全、会话、tracing 与 control socket 路径 | 重启 Admin |
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
