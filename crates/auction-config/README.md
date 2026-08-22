# auction-config

TOML 配置解析与敏感配置加载。

源码布局：`src/lib.rs` 只保留模块声明和公共导出；主/敏感配置解析集中在 `src/config/`，
Admin runtime TOML 文件锁与原子替换集中在 `src/runtime/`。

## 两层文件结构

主配置只放非敏感设置，并通过必填的 `secrets_file` 引用独立敏感 TOML：

```toml
# 可选：监听地址；省略时使用角色默认地址。
listen_address = "127.0.0.1:8090"
# 必填：敏感 TOML 路径。
secrets_file = "secrets.toml"
# 可选：仅 Admin 主配置使用；runtime 示例位于 examples/runtime/admin.toml。
runtime_config_file = "runtime/admin.toml"
# 可选：tracing 过滤器。
log_filter = "info,auction=debug"

# 可选区块：省略时关闭缓存。
[cache]
# 可选：disabled 或 redis，默认 disabled。
backend = "disabled"

# 可选区块：Worker 角色使用；省略时使用并发 1、轮询 5 秒。
[worker]
# 可选：Worker 并发数。
concurrency = 1
# 可选：due 轮询间隔秒数。
poll_interval_seconds = 5

# 可选区块：HTTP 安全策略。
[http_security]
# 可选：是否启用 origin allowlist。
allowed_origins_enabled = true
# 可选：允许的精确 HTTP(S) origin。
allowed_origins = ["http://127.0.0.1:8090"]
# 可选：登录限流次数。
login_rate_limit = 10
# 可选：变更限流次数。
mutation_rate_limit = 60
# 可选：限流窗口秒数。
rate_limit_window_seconds = 60
# 可选：Cookie Secure 属性；生产环境应开启。
cookie_secure = false

# 可选区块：会话策略。
[session]
# 可选：空闲过期秒数。
idle_ttl_seconds = 1800
# 可选：绝对过期秒数。
absolute_ttl_seconds = 604800
# 可选：公开会话并发上限。
max_public_sessions = 5
```

敏感文件的稳定字段集合：`database_url`、`redis_url`、`session_signing_key`、
`ehentai_cookie`、`mooglemail_cookie`、`admin_actor`、`admin_secret`、
`equipment_data_key_id`、`equipment_data_key`。新增外部凭据必须显式
扩展该结构，禁止任意键值透传。Admin 凭据只由 Admin 装配根读取，`admin_secret` 至少需要
16 个字节；Admin 凭据和装备数据密钥的两项字段都必须成对出现，才能建立对应能力。

`equipment_data_key` 必须是 32 字节标准 Base64 数据密钥，`equipment_data_key_id` 是非秘密
轮换标识。两者只由 Admin 装配根读取，用于封装装备 key；任何一个字段都不得进入日志、DTO、
审计 metadata 或浏览器。

相对 `secrets_file` 路径以主配置文件所在目录解析。敏感文件不得提交，部署时应限制为仅运行
账号可读。

Worker 的 concurrency 和 `poll_interval_seconds` 必须为正数；当敏感配置中的 database_url 使用
sqlite: 时，配置层只允许 concurrency 为 1，以落实 Lite 档的单主机、单写者约束。轮询间隔只
负责唤醒持久化调度器，不能替代数据库 due/租约状态。

`http_security` 只保存 origin、速率限制和 Cookie 属性等非敏感策略；`session` 保存 Public
与 Admin 会话的空闲/绝对时限及 Public 并发上限。登录 key、Admin secret、会话 token 和数据库
连接串不得写入主配置。

Admin 主配置的 `runtime_config_file` 指向独立的非敏感 runtime TOML，仓库模板位于
`ops/config/examples/runtime/admin.toml`。该文档由严格应用模型
解析，包含 listener、安全策略、会话时限、metrics/tracing 公共参数和 Worker control socket，
不包含 database URL、Cookie、密码、签名密钥或装备数据密钥。PostgreSQL 完整档通过 Admin
HTTP 的 GET/validate/PUT 读取和原子更新；更新写入后必须重启进程，SQLite Lite 不提供该
完整更新能力。运行中的 runtime 文件必须是受信 regular file，目录不可被 group/world 写入，
文件权限为 `0640`；仓库示例只用于复制模板。

## 环境变量边界

`AUCTION_CONFIG_FILE` **只用于选择主配置文件**，不能覆盖任何单个配置项。业务模块也不得自行
读取环境变量。

## 凭据保护

- 敏感值包装为 `SecretString`，其 `Debug` 输出为 `[REDACTED]`。
- `AppConfig` 的 `Debug` 只报告 `database_configured` 布尔值，不输出连接串。
- TOML 解析失败时只报告文件路径，**不回显原始行**——解析器错误可能带出连接串或 Cookie。
- 读取凭据必须通过显式 getter（`database_url()`、`session_signing_key()` 等）；调用方不得
  把返回值写入日志、错误消息或 DTO。

## 兼容性

当前格式不兼容旧的单文件敏感字段，旧格式必须以明确错误失败。配置字段变化必须同时更新解析
代码、`ops/config/examples/`、相关 README 和部署说明。
