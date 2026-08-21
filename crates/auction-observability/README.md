# auction-observability

tracing 初始化、指标约定、健康检查语义和关联 ID。

`init(service, configured_filter)` 安装 JSON 格式的 `tracing-subscriber`。未配置
`log_filter` 时默认 `info,{service}=debug`。

## 结构化日志约定

使用结构化字段，不要把值拼进消息文本：

```rust
info!(auction_id = %id, command_id = %command_id, "bid accepted");
```

事件名可以保留稳定的英文，字段名必须清楚。

## 禁止记录的内容

会话凭据、论坛 Cookie、邮件 Cookie、密码、数据库连接串和其他密钥材料**不得**出现在日志、
DTO、快照或提交的配置中。配置解析错误也不得回显 TOML 原文。

## 待补齐

指标接口预留 Prometheus，但当前尚未引入 metrics 依赖。健康检查逻辑目前实现在
`auction-http`，按架构表的所有权划分应迁移到本 crate；这一项记录在
`docs/tech-stack.md` 的已知风险中。
