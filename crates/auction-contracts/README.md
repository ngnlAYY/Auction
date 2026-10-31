# auction-contracts

[文档导航](../../docs/README.md) · [HTTP 路由](../../docs/contracts/routes.md) · [实时事件](../../docs/contracts/realtime.md)

Public/Admin HTTP、WebSocket 与首次设置接口的稳定 DTO。后端（`auction-http`、`auction-setup`）与浏览器（`auction-web-public`、`auction-web-admin`）共用同一份类型，任何字段变化都会同时影响两端编译，因此本 crate 只依赖 `serde`、`serde_json`、`chrono`、`uuid`，可编译到 `wasm32-unknown-unknown`。

DTO 只描述线上形状，不含业务规则；从应用投影到 DTO 的映射由 `auction-http` 负责。

## 结构

| 路径 | 内容 |
| --- | --- |
| [`crates/auction-contracts/src/contracts/dto/shared.rs`](src/contracts/dto/shared.rs) | 响应外壳 `ApiResponse`、错误外壳 `ApiErrorResponse`、健康探针、Public 实时事件信封、字段反序列化边界 |
| [`crates/auction-contracts/src/contracts/dto/public/`](src/contracts/dto/public) | 账号与会话、拍卖目录（拍卖、lot、装备、补偿池、论坛镜像、竞价日志与价格历史）、竞价与代理命令、装备标记与提醒偏好 |
| [`crates/auction-contracts/src/contracts/dto/admin/`](src/contracts/dto/admin) | 拍卖与排期、lot、装备录入与词典、论坛预览、邮件状态与入站审查、规则、runtime 配置、凭据、用户、投递、恢复、结算报表与目录、realtime 运维、审计、世界启停 |
| [`crates/auction-contracts/src/contracts/setup.rs`](src/contracts/setup.rs) | 首次设置请求与稳定结果码 |

## 线上约定

- 成功响应：`{"code": "...", "message": "...", "data": …}`；`ApiResponse::created` 使用 `created`，`ApiResponse::accepted` 使用 `accepted`，具体 HTTP 状态与 code 组合按[路由契约](../../docs/contracts/routes.md#响应外壳)冻结，不能仅按“创建”动作推断。
- 错误响应：`{"code": "...", "message": "...", "data": null | {...}, "request_id": null | "..."}`；`code` 是稳定的机器可读值，客户端不应解析 `message`。Public/Admin 的关联标识始终在响应头 `x-request-id`，常规 handler 错误的 JSON 字段可能为 null。
- 健康探针：`/health/live`、`/health/ready` 返回 `{"code","message","data":{"live"|"ready": bool}}`；`/health` 与 `/api/v1/health` 直接返回 `{"service","status","version"}`，版本来自运行进程自身。首次设置也使用独立的 `SetupResponse` / `SetupErrorResponse`，不套业务 API 外壳。
- 时间统一为 RFC 3339 字符串；金额为整数 Credits；标识为规范小写 UUID 字符串。
- 部分字段为兼容旧版 Go 服务保留了“必须出现但可为 null”或“null 视为零值”的反序列化语义，相关辅助函数在 [`crates/auction-contracts/src/contracts/dto/shared.rs`](src/contracts/dto/shared.rs) 中显式标注，新增字段不要随意放宽。
- 标有 `deny_unknown_fields` 的 DTO 在浏览器解码时拒绝额外字段；该约束不改变服务端序列化的字段集合。
- 提供 `Debug` 的敏感 DTO 会隐藏登录 key、凭据 Cookie、Admin 口令及 CSRF token；首次设置凭据请求不实现 `Debug`。

## 变更规则

- 已发布字段不得改名或改变语义；新增字段优先设为可选，并同时更新后端映射与两端页面。
- 新增或删除路由时同步更新 [路由清单](../../docs/contracts/routes.md)；实时事件的种类与负载变化同步更新 [实时事件](../../docs/contracts/realtime.md)。
- 本 crate 的任何改动都会触发两端浏览器 payload 重建（发布脚本的 Web 输入指纹包含本 crate）。
