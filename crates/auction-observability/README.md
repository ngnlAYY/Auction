# auction-observability

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

结构化日志、进程指标、心跳和健康安全投影。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [实现布局与 tracing](#实现布局与-tracing)
- [结构化日志约定](#结构化日志约定)
- [禁止记录的内容](#禁止记录的内容)
- [健康 dashboard](#健康-dashboard)
- [指标与剩余验收证据](#指标与剩余验收证据)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/health](src/health) | 心跳模型、发布端口与 dashboard 安全投影 |
| [src/metrics](src/metrics) | 进程、连接池、业务、Worker 与 tracing 指标 |
| [src/telemetry](src/telemetry) | tracing 配置、有界批处理、exporter 与健康 |

## 职责与实现边界

提供可观测性原语和约定，不拥有业务状态、数据库 SQL 或停止顺序。
装配根传入角色身份及已校验配置，存储适配器实现心跳和连接池窄端口。

## 实现布局与 tracing

`init(service, configured_filter)` 安装 JSON tracing，缺省过滤器为 `info,{service}=debug`。
`init_with_config` 可增加默认关闭的 OTLP/HTTP exporter，使用异步有界队列和有界退出刷新。
Public/Worker 参数来自角色配置，Admin 可由 runtime 覆盖；应用装配只传入敏感 TOML 中的 OTLP headers。
exporter 不监听操作系统信号；装配根收口业务资源后调用刷新和关闭。

追踪使用 OpenTelemetry API/SDK/OTLP 0.33.1 与 tracing-opentelemetry 0.34.0，继续保留自有有界队列、
丢弃/失败计数、安全错误分类及关闭预算。OTLP builder 显式使用 `RetryPolicy::disabled()`，不接受 0.33
默认新增的三次重试，避免静默增加请求次数或占用导出/关闭预算。保留上游默认 64 MiB 请求体上限
（编码及压缩后的限制）；超限计入导出失败，不影响业务命令或触发业务重试。未启用 gRPC 或 OTel 内部日志。

上游 HTTP builder 会合并环境中的导出头。`auction-config` 在启用 tracing 时拒绝这类环境注入，
不修改全局环境、不回显值；装配根仅传入校验后的 TOML 配置，详见[配置边界](../../ops/config/README.md#文件职责)。

## 结构化日志约定

使用稳定事件名和具名字段，不拼接业务值：

```rust
info!(auction_id = %id, command_id = %command_id, "bid accepted");
```

逐笔业务证据以数据库事实与追加审计为准，指标和日志不能替代。

## 禁止记录的内容

Cookie、密码、会话 token、数据库连接串、数据密钥和未脱敏原始响应不得进入日志或指标。
配置解析失败不回显 TOML 原文；邮件原始证据走[独立加密诊断](../../docs/operations/mail-diagnostics.md)。

## 健康 dashboard

`HeartbeatStore` 提供心跳读写，`HealthService` 输出有界安全状态，不返回 details 原文。
装配根发布启动、就绪、周期和停止心跳；周期重新检查数据库/schema readiness、审计失败计数和 tracing 健康。
依赖或审计异常降级 readiness；tracing degraded 只降级状态，不单独改变应用 readiness。

各业务 owner 提供 dashboard 汇总，未装配、缺能力或读取失败均显式 unknown/source_unavailable。
`with_details` 只接受冻结的非敏感对象；Worker 的论坛开关通过专用端口读取，不把整个 details 暴露给浏览器。
内部详情直接使用 `serde_json::Value`，本 crate 不额外声明未使用的直接 serde 依赖。
HTTP 响应语义见[Dashboard 契约](../../docs/contracts/routes.md#dashboard-与-readiness)。

## 指标与剩余验收证据

Public/Admin 的 `/metrics` 默认关闭；Worker 使用经 peer 认证的 control Unix socket 与同一 registry。
进程资源、连接池快照/实际 acquisition 累计、固定 Worker loop 标签和业务结果/耗时均保持低基数，
不以用户、拍品、命令、邮箱或原始错误作为 label。存储边界之外的原生 SQLx 池不计入 acquisition 累计。

Prometheus 只启用 `process`，关闭默认 `protobuf` 并使用上游 `plain_model`；进程、池和 tracing collector
继续经 `TextEncoder` 输出 Prometheus 0.0.4 文本。指标名、label、bucket 和媒体类型契约不变，
不因此关闭 OTLP/HTTP-proto 的独立追踪编码栈，也不切换 OpenMetrics 或替换业务 recorder 端口。

队列满会计入 exporter 丢弃数；静态编译和已有指标注册不证明真实部署采集成功或输出逐字节等价，
运行证据集中在状态文档。

## 维护与验证

```bash
cargo check --locked -p auction-observability
```

仅静态编译；默认检查与测试范围见[开发指南](../../docs/development.md)，运行证据见[验证状态](../../docs/status.md)。
字段、路由或部署行为变化时同步对应契约和配置模板。
