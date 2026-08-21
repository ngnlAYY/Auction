# TOML 配置

运行时配置统一使用 TOML。进程会在 `ops/config/{role}.toml` 存在时加载对应文件；使用
`AUCTION_CONFIG_FILE` 时，它只负责选择另一个 TOML 文件，单个配置项不会从环境变量读取。

已经纳入版本控制的示例位于 [`examples/`](examples/)。本地开发时，可以复制示例到运行时
路径，也可以将 `AUCTION_CONFIG_FILE` 指向示例文件。

不要提交包含密码、令牌、Cookie 或其他敏感信息的真实配置。需要本地覆盖时使用被 Git 忽略
的 `.local.toml` 或 `.secret.toml` 文件，并保持配置结构与示例一致。
