# auction-config

TOML 配置解析与敏感配置加载。

## 两层文件结构

主配置只放非敏感设置，并通过必填的 `secrets_file` 引用独立敏感 TOML：

```toml
listen_address = "127.0.0.1:8090"
secrets_file = "secrets.toml"
log_filter = "info,auction=debug"

[cache]
backend = "disabled"
```

敏感文件的稳定字段集合：`database_url`、`redis_url`、`session_signing_key`、
`ehentai_cookie`、`mooglemail_cookie`。新增外部凭据必须显式扩展该结构，禁止任意键值透传。

相对 `secrets_file` 路径以主配置文件所在目录解析。敏感文件不得提交，部署时应限制为仅运行
账号可读。

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
