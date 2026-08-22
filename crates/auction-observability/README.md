# auction-observability

tracing 初始化、指标约定、健康检查语义和关联 ID。

源码布局：`src/lib.rs` 只保留模块声明和公共导出；tracing 实现在 `src/telemetry/mod.rs`，
心跳与 dashboard 安全投影实现在 `src/health/mod.rs`。

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

## 健康 dashboard

`HeartbeatStore` 定义服务心跳的发布/读取窄端口，`HealthService` 只生成不含 details 原文的
process/health 快照。Admin `/api/v1/admin/dashboard` 已接入该首片；未迁移的 mail、realtime、
credentials、delivery、fulfillment、recovery 和 alerting owner 会返回 `unknown`，不会返回示例
数据。Public/Admin/Worker 装配根已发布启动/就绪/周期/停止心跳；其余 owner 聚合仍由后续迁移
阶段装配。

## 待补齐

指标接口预留 Prometheus，但当前尚未引入 metrics 依赖；这一项记录在
`docs/tech-stack.md` 的已知风险中。
