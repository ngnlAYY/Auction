# auction-contracts

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

稳定 REST、实时消息、错误和分页 DTO；不承载数据库或领域写入。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [DTO 布局与消费方](#dto-布局与消费方)
- [稳定投影](#稳定投影)
- [命名与依赖边界](#命名与依赖边界)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/contracts/dto/public](src/contracts/dto/public) | Public 请求、响应与实时投影 |
| [src/contracts/dto/admin](src/contracts/dto/admin) | Admin 请求与安全响应 |
| [src/contracts](src/contracts) | 公共信封与稳定导出 |
| [src/contracts/setup.rs](src/contracts/setup.rs) | 独立首次部署输入、阶段与错误码 |

## 职责与实现边界

本 crate 拥有对外字段和序列化形状；路由、状态码、字段语义与版本规则分别见
[HTTP 契约](../../docs/contracts/routes.md)和[实时契约](../../docs/contracts/events.md)。

## DTO 布局与消费方

`lib.rs` 与 `contracts/mod.rs` 只负责导出，`contracts/dto/` 按通用、Public、Admin 和业务职责拆分。
HTTP 与两端浏览器复用这些 DTO；数据库行、应用读模型和领域聚合仍保持独立，在边界转换。

[dto/public/watch.rs](src/contracts/dto/public/watch.rs) 定义装备标记的完整集合、CAS 请求和可重放响应。
持久 version 为正 i64，缺项才表示 false/version 0；取消保留正版本 tombstone，请求禁止未知字段。
旧 channel 恒为 mm 兼容占位。[dto/public/watch_preference.rs](src/contracts/dto/public/watch_preference.rs)
定义账号渠道 GET/PUT：响应 user_id/channel/version，请求 command_id/channel/expected_version；
MM/PM 闭集与正 i64 版本，未知字段拒绝；不把数据库任务行或凭据发给浏览器。

## 稳定投影

论坛同步开关和镜像 DTO 由 `dto/public/catalog/forum.rs` 唯一拥有；Admin 认证状态 GET 复用同一安全字段形状，
不复制 DTO，也不包含凭据或 Worker claim。浏览器两个 surface 仍独立实现读取和展示。

- `HealthResponse` 的服务名和版本由装配根传入，不能用本 crate 的版本代替运行进程版本。
- `ApiErrorResponse` 统一安全错误信封；Admin 列表与 dashboard 只暴露声明的安全字段，不透传内部 payload。
- `SetupRequest`/`SetupResponse` 属于独立设置服务；`completed` 只表示配置保存，`initialize_and_start` 说明后续动作，不表示 readiness。
- `AdminRuntimeConfigResponse` 区分文档候选、fingerprint 与重启要求；`managed` 不表示热加载或当前已生效。
- `MAX_PUBLIC_BID_AMOUNT` 供浏览器即时反馈，HTTP 编译时核对领域常量，最终金额校验仍由领域层完成。
- `PublicForumResponse` 的真实楼层水位与 P21 布尔开关相互独立；未知开关通过错误表达，不能默认成 false。

具体 DTO 字段、可省略项、分页和后端能力统一在契约文档维护，不在 README 复制清单。

## 命名与依赖边界

新增 JSON 字段使用 `snake_case`，已发布名称通过 serde 映射兼容；字段、路由、错误或事件变化必须同步契约。
依赖为 serde、serde_json、chrono 与 uuid，见 [Cargo.toml](Cargo.toml)；不得引入 Axum、SQLx 或适配器类型。

## 维护与验证

```bash
cargo check --locked -p auction-contracts
```

仅静态编译；默认检查与禁止默认运行的测试范围见[开发指南](../../docs/development.md)，
运行证据见[验证状态](../../docs/status.md)。修改公共行为时同步对应契约。
