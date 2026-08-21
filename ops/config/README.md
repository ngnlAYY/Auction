# TOML 配置

运行时配置统一使用两个 TOML 文件：主配置保存普通设置，敏感配置保存凭据。进程会在
`ops/config/{role}.toml` 存在时加载对应主配置；`AUCTION_CONFIG_FILE` 只负责选择另一个
主配置，单个配置项不会从环境变量读取。

已经纳入版本控制的示例位于 [`examples/`](examples/)。主配置必须通过 `secrets_file`
引用敏感文件；相对路径以主配置所在目录解析。示例主配置引用的 `secrets.toml` 只包含
安全的本地占位值，不能直接用于生产环境。

生产或本地真实配置可按以下方式准备：

```bash
cp ops/config/examples/secrets.toml ops/config/auction.secret.toml
chmod 600 ops/config/auction.secret.toml
```

然后把主配置的 `secrets_file` 指向 `../auction.secret.toml`。`*.secret.toml` 和
`*.local.toml` 已被 Git 忽略，但部署者仍需保证备份、文件所有者和读取权限正确。
生产环境宜为 Public、Admin、Worker 分别挂载敏感文件，并只提供该角色实际需要的字段；
仓库中的单一 `secrets.toml` 只是共享结构示例。

以下字段只能出现在敏感 TOML：

- `database_url`：数据库连接串；
- `redis_url`：启用 Redis 缓存时的连接串；
- `session_signing_key`：会话签名密钥；
- `ehentai_cookie`、`mooglemail_cookie`：外部服务 Cookie。

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
