# auction-contracts

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

稳定 REST、实时消息、错误和分页 DTO；只拥有对外字段与序列化形状，不承载数据库行、应用读模型或领域写入。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/contracts/dto/public](crates/auction-contracts/src/contracts/dto/public) | Public 请求、响应与实时投影 |
| [src/contracts/dto/admin](crates/auction-contracts/src/contracts/dto/admin) | Admin 请求与安全响应 |
| [src/contracts](crates/auction-contracts/src/contracts) | 公共信封与稳定导出 |
| [src/contracts/setup.rs](crates/auction-contracts/src/contracts/setup.rs) | 独立首次部署输入、阶段与错误码 |

## 职责与实现边界

`lib.rs` 与 `contracts/mod.rs` 只负责导出，`contracts/dto/` 按通用、Public、Admin 和业务职责拆分；HTTP 与两端浏览器
复用这些 DTO，数据库行与领域聚合在边界转换。路由、状态码、字段语义与版本规则见
[HTTP 契约](docs/contracts/routes.md)和[实时契约](docs/contracts/events.md)，字段清单不在本页复制。

## DTO 布局与消费方

[dto/public/watch.rs](crates/auction-contracts/src/contracts/dto/public/watch.rs) 定义装备标记的完整集合、CAS 请求和
可重放响应：持久 version 为正 i64，缺项才表示 false/version 0，取消保留正版本 tombstone，旧 channel 恒为 mm 占位；
[dto/public/watch_preference.rs](crates/auction-contracts/src/contracts/dto/public/watch_preference.rs) 定义账号渠道
GET/PUT（响应 user_id/channel/version，请求 command_id/channel/expected_version），未知字段一律拒绝。
两者都不把数据库任务行或凭据发给浏览器。

## 稳定投影

论坛同步开关和镜像 DTO 由 `dto/public/catalog/forum.rs` 唯一拥有，Admin 认证状态 GET 复用同一安全字段形状而不复制 DTO。

- `HealthResponse` 的服务名和版本由装配根传入，不能用本 crate 版本代替运行进程版本。
- `ApiErrorResponse` 是统一安全错误信封，列表与 dashboard 只暴露声明的安全字段，不透传内部 payload。
- `SetupRequest`/`SetupResponse` 属于独立设置服务：`completed` 只表示配置保存，`initialize_and_start` 说明后续动作；
  `AdminRuntimeConfigResponse` 区分候选、fingerprint 与重启要求，`managed` 不表示热加载。
- `MAX_PUBLIC_BID_AMOUNT` 供浏览器即时反馈，最终金额校验仍由领域层完成；`PublicForumResponse` 的楼层水位与
  P21 布尔开关相互独立，未知开关通过错误表达而非默认 false。

## 命名与依赖边界

新增 JSON 字段用 `snake_case`，已发布名称通过 serde 映射兼容，字段/路由/错误/事件变化必须同步契约；依赖仅
serde、serde_json、chrono 与 uuid（见 [Cargo.toml](crates/auction-contracts/Cargo.toml)），不得引入 Axum、SQLx 或适配器类型。

## 维护与验证

```bash
cargo check --locked -p auction-contracts
```

仅静态编译；默认检查与禁止默认运行的测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
