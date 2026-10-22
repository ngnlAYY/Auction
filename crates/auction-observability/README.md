# auction-observability

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

结构化日志、进程指标、心跳和健康安全投影。本 crate 只提供可观测性原语和约定，不拥有业务状态、数据库 SQL 或
停止顺序：装配根传入角色身份及已校验配置，存储适配器实现心跳和连接池窄端口。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/health](crates/auction-observability/src/health) | 心跳模型、发布端口与 dashboard 安全投影 |
| [src/metrics](crates/auction-observability/src/metrics) | 进程、连接池、业务、Worker 与 tracing 指标 |
| [src/telemetry](crates/auction-observability/src/telemetry) | tracing 配置、有界批处理、exporter 与健康 |

## 实现布局与 tracing

`init_json_stderr(configured_filter)` 提供运维命令专用的 JSON stderr 入口：只使用调用方显式过滤器，不读环境、不启用 OTLP，
不附加父 span 字段，保留 stdout 命令结果；调用方负责限定已审查的安全 target。

`init(service, configured_filter)` 安装 JSON tracing，缺省过滤器为 `info,{service}=debug`；`init_with_config`
可增加默认关闭的 OTLP/HTTP exporter，使用异步有界队列和有界退出刷新。Public/Worker 参数来自角色配置，
Admin 可由 runtime 覆盖，应用装配只传入敏感 TOML 中的 OTLP headers；exporter 不监听操作系统信号，
装配根收口业务资源后调用刷新和关闭。

追踪使用 OpenTelemetry API/SDK/OTLP 0.33.1 与 tracing-opentelemetry 0.34.0，保留自有有界队列、丢弃/失败计数
和关闭预算。OTLP builder 显式使用 `RetryPolicy::disabled()`，不接受 0.33 默认新增的三次重试；保留上游默认
64 MiB 请求体上限（编码及压缩后），超限计入导出失败，不影响业务命令。未启用 gRPC 或 OTel 内部日志。
上游 HTTP builder 会合并环境导出头，`auction-config` 在启用 tracing 时拒绝这类环境注入，装配根只传入
校验后的 TOML 配置（见[配置边界](ops/config/README.md#文件职责)）。
批处理器在关闭等待超时或被丢弃时取消所属任务，不让 exporter 和批次定时器脱离生命周期。
导出状态、失败计数与时间戳在同一锁内更新和读取，防止并发心跳取得相互矛盾的状态组合；
逐 span 的队列丢弃计数仍使用原子路径，不争用该锁。

## 日志约定与禁止内容

使用稳定事件名和具名字段，不拼接业务值（如 `info!(auction_id = %id, command_id = %command_id, "bid accepted")`）；
逐笔业务证据以数据库事实与追加审计为准，指标和日志不能替代。Cookie、密码、会话 token、数据库连接串、
数据密钥和未脱敏原始响应不得进入日志或指标；配置解析失败不回显 TOML 原文，邮件原始证据走
[独立加密诊断](ops/mail/diagnostics.md)。

## 健康 dashboard

`HeartbeatStore` 提供心跳读写，`HealthService` 输出有界安全状态，不返回 details 原文。装配根发布启动、就绪、
周期和停止心跳；周期重新检查数据库/schema readiness、审计失败计数和 tracing 健康。依赖或审计异常降级 readiness，
tracing degraded 只降级状态。各业务 owner 提供 dashboard 汇总，未装配、缺能力或读取失败均显式
unknown/source_unavailable；`with_details` 只接受冻结的非敏感对象，Worker 论坛开关经专用端口读取，
不把整个 details 暴露给浏览器；内部详情直接使用 `serde_json::Value`，本 crate 不额外声明未使用的直接 serde 依赖。响应语义见 [Dashboard 契约](docs/contracts/routes.md#dashboard-与-readiness)。
心跳 details 的 64 KiB 校验直接流式计数紧凑 JSON 字节，超过预算立即拒绝，不为校验分配完整序列化副本；
UTF-8、转义及对象限制与实际持久化编码保持一致。

## 指标与剩余验收证据

Public/Admin 的 `/metrics` 默认关闭，Worker 使用经 peer 认证的 control Unix socket 与同一 registry。进程资源、
连接池快照/实际 acquisition 累计、固定 Worker loop 标签和业务结果/耗时均保持低基数，不以用户、拍品、命令、
邮箱或原始错误作为 label；存储边界之外的原生 SQLx 池不计入 acquisition 累计。

Prometheus 只启用 `process`，关闭默认 `protobuf` 并使用上游 `plain_model`；进程、池和 tracing collector 继续经
`TextEncoder` 输出 Prometheus 0.0.4 文本，指标名、label、bucket 和媒体类型契约不变，也不切换 OpenMetrics。
队列满计入 exporter 丢弃数；静态编译和已注册指标不证明真实部署采集成功，运行证据集中在状态文档。
tracing 丢弃与导出失败 counter 以原子水位领取快照增量，并行 scrape 不会重复累计同一次失败；
指标名称和 counter 类型保持不变。

## 维护与验证

```bash
cargo check --locked -p auction-observability
```

仅静态编译；默认检查与禁止默认运行的测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
