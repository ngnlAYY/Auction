# 配置参考

[文档导航](docs/README.md) · [运维手册](ops/README.md) · [示例](ops/config/examples) · [auction-config](crates/auction-config/README.md)

所有运行配置都是 TOML。字段的权威说明是 [`ops/config/examples/`](ops/config/examples) 中每个字段上方的中文注释（标明“可选/必填”、默认值与范围）；本页说明文件结构、加载规则与各区块的用途。

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
| `common.toml` | `log_filter`、`[metrics]`、`[cache]`、`[auction.*]`（排期、早鸟、经济、结算、恢复策略） |
| `secrets.toml` | 数据库、Redis URL、外部 Cookie、Admin 登录凭据、三组数据密钥、OTLP 导出头 |
| `runtime/admin.toml` | 可由 Admin 后台修改的运行参数（见下文） |

### 加载规则

- 角色文件路径：环境变量 `AUCTION_CONFIG_FILE`；未设置时依次尝试当前目录下的 `ops/config/<角色>.toml`（Admin 另会尝试 `auction-config/admin.toml`）。`auction-server` 与 `auction-cli` 使用 `--config-dir` 指定目录，不读取该环境变量。
- 除 `AUCTION_CONFIG_FILE` 外不存在任何环境变量覆盖；任何字段都只能写在文件中。
- 所有错误都在启动前失败，错误信息只包含路径与字段，绝不回显秘密值。文件大小上限 4 MiB。
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

三组数据密钥都是 32 字节随机数的标准 Base64（`openssl rand -base64 32`），标识与材料必须成对，三组不能复用。密钥缺失时对应能力不可用（接口返回能力不可用，Worker 相关功能拒绝启用）。

文件必须只有 owner 可访问（0600）。建库/重建用的维护凭据是另一个只含 `[database]` 的独立文件（模板 `ops/config/examples/secrets/postgres-rebuild.toml`，`name` 固定为 `postgres`），只供 `auction-cli db create/rebuild` 使用，常驻进程不引用。

## common.toml

| 区块 | 用途 |
| --- | --- |
| `log_filter` | tracing 过滤器（`EnvFilter` 语法），默认 `info,<服务名>=debug` |
| `[metrics]` | `enabled`：是否提供 `/metrics`；Public/Admin 开启时 listener 必须是 loopback 或 Unix socket |
| `[cache]` | 见[缓存](#缓存) |
| `[auction.schedule]` | 周排期：时区、预览/开拍/计划结束的星期与时间、参与世界 |
| `[auction.schedule.early_bird]` | 赛季早鸟期：赛季开始日期与时间、窗口天数、开拍日、开拍钟点、预览提前量、竞价时长、窗口内品质、赛季前暂停收件 |
| `[auction.economics]` | 新场次的经济规则：默认起拍价、加价比例、软截止窗口与延长、代理开关/默认模式/慢速间隔/设置次数上限、Final 提前量 |
| `[auction.settlement]` | 新场次的结算策略：版本与模式（`hold_curve_v2` 或 `legacy_zero_v0`）、补偿池、isekai 载体 Credits、领先时长曲线锚点 |
| `[auction.settlement.recovery]` | 恢复策略：付款期限、提醒次数、COD 退回上限 |

`[auction.*]` 的取值在**创建场次时冻结**，修改只影响之后创建的场次。业务含义见[业务规则](docs/business-rules.md)与[结算与履约](docs/guide/settlement.md)。

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
| `[worker.mooglemail]` | 邮件：开关、邮箱标识与世界、邮件默认起拍价、请求边界、cron 计划与抖动、租约、重试、发件人白名单、`keyenv_file`（本地联调） |
| `[worker.mooglemail.send_intervals]` | 各类出站邮件发送起点的最小间隔 |
| `[worker.mooglemail.diagnostics]` | 诊断归档：开关、目录（生产须为可写目录）、保留天数、总字节上限 |
| `[worker.mooglemail.equipment_filter]` | 投稿过滤：品质门槛（始终生效）、前后缀/分类/类型/等级过滤 |
| `[worker.mooglemail.mail_templates]` | 出站邮件正文模板（只能使用规定占位符） |
| `[[worker.mooglemail.additional_worlds]]` | 额外世界的邮箱（独立游标、租约与白名单） |
| `[worker.operations_alert]` | 运维告警：开关、收件人与世界、扫描间隔、批次窗口、冷却、各类积压阈值 |

所有循环必须满足“单项超时 < 租约”等预算关系，loader 会校验。启用外部功能时的依赖条件：

- MoogleMail：需要凭据来源（数据库凭据 + `credential_data_key`，或 `keyenv_file`，或 `mooglemail_cookie`）以及 `user_login_data_key`。
- 论坛：需要 `ehentai_cookie` 或 `credential_data_key`。
- 运维告警：需要收件人，告警世界必须在已启用的 MoogleMail 世界内。

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
| `worker`（含 `delivery`/`fulfillment`/`recovery`） | Worker 各循环的调度边界 | 重启 Worker |
| `mail`（含 `send_intervals`） | 邮件轮询计划与请求边界、发送间隔（不包含全局开关、凭据与邮箱身份） | 重启 Worker |

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

`auction-cli config check` 在非严格模式下把权限问题列为警告，`--strict` 视为错误；生产部署必须通过严格检查。

## 示例与首次设置

`ops/config/examples/` 中的文件同时是首次设置向导的生成模板（编译期内嵌）。因此：

- 每个赋值字段上方必须紧邻含“可选”或“必填”的中文注释（`ci/checks/check.sh config` 检查）。
- 修改示例后，向导生成的三角色配置必须仍能通过 loader 校验。
- 示例中的值仅用于演示；`allowed_peer_*`、origin、密钥等必须按实际部署填写。
