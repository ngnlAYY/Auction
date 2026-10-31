# auction-config

[文档导航](../../docs/README.md) · [配置参考](../../ops/config/README.md) · [示例配置](../../ops/config/examples)

TOML 配置的加载、合并与校验：角色文件、公共文件、敏感文件、Admin runtime 文件与维护凭据，以及首次部署的配置生成。所有配置错误都在进程启动前失败，错误只给出路径与字段提示，绝不回显秘密值。

## 公开接口

| 接口 | 用途 |
| --- | --- |
| `AppConfig::load(role)` | 读取 `AUCTION_CONFIG_FILE`；未设置时尝试运行目录下的 `ops/config/<role>.toml`（不是仓库模板）；不存在时使用内置默认值。Admin 的 `auction-config/admin.toml` 发现与首次设置由 Admin 装配根另行处理 |
| `AppConfig::load_from_path(role, path)` | 从显式路径加载（`auction-server`、`auction-cli` 使用） |
| `AppConfig` 字段 | 已合并、已校验的非敏感配置：listener、metrics、tracing、realtime、cache、auction、worker、http_security、session、路径、`admin_runtime`、装备规则与 `bot_players` 投影 |
| `AppConfig` getter | 敏感值只经显式 getter 暴露：`database_url()`、`redis_url()`、`admin_actor()/admin_secret()`、三组数据密钥、Cookie、`tracing_headers()`；`Debug` 输出已脱敏 |
| `TomlRuntimeFileStore` | Admin runtime 文件的读取与原子替换（临时文件 + rename + 目录同步 + 独立锁文件） |
| `load_postgres_maintenance_secrets` | 读取建库/重建维护凭据（仅 `auction-cli` 使用） |
| `load_mooglemail_keyenv` | 读取本地联调用的 `.keyenv`（`hv-ck-key1` 为 Bot 号，`hv-ck-key2` 为测试号） |
| `StandardCronSchedule` | 保留旧版五字段 cron 输入语法，支持描述符、`@every` 与 `TZ=`/`CRON_TZ=` 前缀；日历触发采用 croner |
| `FirstRunConfig` | 首次设置表单到三角色配置目录的原子物化 |
| `managed_runtime_directory` | 合并部署配置目录对应的同级运行目录 `.<目录名>.server` |

## 加载流程

1. 固定角色文件的绝对路径，按角色解析（Public 的 `[session]` 只接受 `max_public_sessions`，旧的时长字段被拒绝）。
2. 以角色文件所在目录解析 `common_file`、`secrets_file`、`runtime_config_file`、`worker_config_file`（仅 Admin）等相对路径，读取公共文件（闭集，拒绝角色字段混入）。
3. 读取敏感文件：`[database]` 五字段或兼容的 `database_url`（二者互斥）、Redis URL、Cookie、OTLP headers、Admin 凭据、三组数据密钥（标识与材料必须成对）。
4. 读取并完整校验 runtime 文件：Admin/Worker 应用 `[operations.*]` 中 `managed = true` 的分组覆盖；Worker 再投影邮件世界、过滤、模板与论坛标色。Public 只保留 `bot_players` 名单与装备规则；Worker 投影完成后也不保留完整 Admin runtime 文档。
5. 解析 listener（Admin runtime 的 listener、安全、会话、metrics、tracing 优先于角色文件），按角色校验全部区块与跨字段约束（预算关系、超时与租约、启用条件等）。

角色、公共与普通敏感文件统一限 4 MiB，在同一文件描述符上检查普通文件类型（FIFO 不会阻塞）。runtime 文件也限 4 MiB，额外要求父目录非符号链接且禁止 group/other 写入、文件精确 `0640`；维护凭据限 64 KiB，要求规范化绝对路径、单硬链接、当前账号所有且仅 owner 可读。

## 主要约束

- **数据库**：`[database]` 五字段要求显式主机、非零端口、账号、口令与库名，账号口令按原文填写，由 loader 负责 URL 编码；兼容的 `database_url` 必须显式指定主机（或 socket）与库名，省略端口时固定为 5432，不继承 `PGPORT` 等环境变量。
- **Redis**：`cache.backend = "redis"` 时必须提供 `redis_url`。
- **tracing**：启用时须有 endpoint 与非零采样率；进程环境中存在 `OTEL_EXPORTER_OTLP_HEADERS` 或 `OTEL_EXPORTER_OTLP_TRACES_HEADERS` 时拒绝启动，导出头只能来自敏感文件。
- **Worker**：启用 MoogleMail 时必须有凭据来源（数据库凭据所需的 `credential_data_key`、`.keyenv` 或 `mooglemail_cookie`）以及 `user_login_data_key`；启用论坛时必须有 `ehentai_cookie` 或 `credential_data_key`；启用运维告警时须配置收件人、告警 world 必须在已启用的 MoogleMail 世界内，且 Worker 运行时要求 MoogleMail 投递可用。
- **HTTP**：origin allowlist 启用时不能为空（TCP 模式可从 listener 推导）；`trusted_unix_proxy` 只能用于 Unix listener；Public/Admin 启用 metrics 时 listener 必须是 loopback TCP 或 Unix socket（`/metrics` 与业务共用 listener，不能直接暴露到公网）。
- `log_filter` 用与 tracing 初始化相同的 `EnvFilter` 语法预先校验。
- **cron 时区**：省略前缀时使用进程本地时区。日历表达式先展开字段，再由 croner 判断：时、分、秒各只有一个取值的固定时刻，在夏令时跳过该时刻时选择缺失区间后的首个真实时刻，回拨时只取较早的一次；其他表达式跳过不存在的时刻、保留回拨中的两次匹配。`@every` 使用 UTC 时长，与夏令时无关。语法兼容不表示与其他 cron 实现的夏令时策略相同。

## 源码结构

| 路径 | 内容 |
| --- | --- |
| `crates/auction-config/src/config/settings/app/` | `AppConfig` 模型、加载、最终构建与 getter |
| `crates/auction-config/src/config/settings/auction/` | 缓存、拍卖策略/排期/早鸟、HTTP listener、可观测性、角色默认值 |
| `crates/auction-config/src/config/settings/worker/` | Worker 主循环、投递、履约、恢复、control、论坛、MoogleMail、运维告警 |
| `crates/auction-config/src/config/settings/worker_validation/`、`crates/auction-config/src/config/settings/validation/` | 各区块的范围与跨字段校验 |
| `crates/auction-config/src/config/settings/secrets/` | 敏感模型、数据库连接字段、维护凭据、`.keyenv`、runtime 投影 |
| `crates/auction-config/src/config/settings/operations/` | runtime `[operations.*]` 接管覆盖 |
| `crates/auction-config/src/runtime/` | runtime 文件存储与校验 |
| `crates/auction-config/src/scheduling/` | 标准 cron |
| `crates/auction-config/src/setup/` | 首次部署配置生成与合并部署运行目录 |

首次设置直接以 `ops/config/examples/` 中的示例为模板（编译期 `include_str!`），因此示例文件同时是生成器的输入：修改示例时须保证首次设置生成的配置仍能通过三角色 loader 校验。示例中每个赋值字段前必须紧邻含“可选/必填”的中文注释（`ci/checks/check.sh config` 检查）。

首次设置发布目录前要求其直接父目录已存在、为非符号链接目录、归当前账号或 root 所有且禁止 group/other 写入；私有临时目录中的口令与 Admin 凭据不能依赖可被其他账号替换的同级名称。目标已存在仍一律拒绝覆盖。
