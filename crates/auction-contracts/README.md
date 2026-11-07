# auction-contracts

[docs/README.md](../../docs/README.md) · [docs/contracts/routes.md](../../docs/contracts/routes.md) · [docs/contracts/realtime.md](../../docs/contracts/realtime.md)

Public/Admin HTTP、WebSocket 与首次设置接口的稳定 DTO。后端（`auction-http`、`auction-setup`）与浏览器（`auction-web-public`、`auction-web-admin`）共用同一份类型，任何字段变化都会同时影响两端编译，因此本 crate 只依赖 `serde`、`serde_json`、`chrono`、`uuid`，可编译到 `wasm32-unknown-unknown`。

DTO 只描述线上形状，不含业务规则；从应用投影到 DTO 的映射由 `auction-http` 负责。

## 结构

| 路径 | 内容 |
| --- | --- |
| [crates/auction-contracts/src/contracts/dto/shared.rs](src/contracts/dto/shared.rs) | 响应外壳 `ApiResponse`、错误外壳 `ApiErrorResponse`、健康探针、Public 实时事件信封、字段反序列化边界 |
| [crates/auction-contracts/src/contracts/dto/public](src/contracts/dto/public) | 账号与会话、拍卖目录（拍卖、lot、装备、补偿池、论坛镜像、竞价日志与价格历史）、竞价与代理命令、装备标记与提醒偏好 |
| [crates/auction-contracts/src/contracts/dto/admin](src/contracts/dto/admin) | 拍卖与排期、lot、装备录入与词典、论坛预览、邮件状态与入站审查、规则、runtime 配置、凭据、用户、投递、恢复、结算报表与目录、realtime 运维、审计、世界启停 |
| [crates/auction-contracts/src/contracts/setup.rs](src/contracts/setup.rs) | 首次设置请求与稳定结果码 |

## 线上约定

- 成功响应：`{"code": "...", "message": "...", "data": …}`；`ApiResponse::created` 使用 `created`，`ApiResponse::accepted` 使用 `accepted`，具体 HTTP 状态与 code 组合按[docs/contracts/routes.md#响应外壳](../../docs/contracts/routes.md#响应外壳)冻结，不能仅按“创建”动作推断。
- 错误响应：`{"code": "...", "message": "...", "data": null | {...}, "request_id": null | "..."}`；`code` 是稳定的机器可读值，客户端不应解析 `message`。Public/Admin 的关联标识始终在响应头 `x-request-id`，常规 handler 错误的 JSON 字段可能为 null。
- 健康探针：`/health/live`、`/health/ready` 返回 `{"code","message","data":{"live"|"ready": bool}}`；`/health` 与 `/api/v1/health` 直接返回 `{"service","status","version"}`，版本来自运行进程自身。首次设置也使用独立的 `SetupResponse` / `SetupErrorResponse`，不套业务 API 外壳。
- 时间统一为 RFC 3339 字符串；金额为整数 Credits；标识为规范小写 UUID 字符串。
- 部分字段为兼容旧版 Go 服务保留了“必须出现但可为 null”或“null 视为零值”的反序列化语义，相关辅助函数在 [crates/auction-contracts/src/contracts/dto/shared.rs](src/contracts/dto/shared.rs) 中显式标注，新增字段不要随意放宽。
- 标有 `deny_unknown_fields` 的 DTO 在浏览器解码时拒绝额外字段；该约束不改变服务端序列化的字段集合。
- 提供 `Debug` 的敏感 DTO 会隐藏登录 key、凭据 Cookie、Admin 口令及 CSRF token；首次设置凭据请求不实现 `Debug`。

## 变更规则

- 已发布字段不得改名或改变语义；新增字段优先设为可选，并同时更新后端映射与两端页面。
- 响应新增字段的兼容方式：服务端先发布“多写”版本（新增可选字段，旧浏览器忽略未知字段），确认部署完成后再让浏览器依赖它；浏览器构建号不一致时只提示刷新，因此同一部署窗口内新旧页面可能并存。`deny_unknown_fields` 只约束解码方向，新增响应字段不会让旧浏览器失败；反过来，删除或改名响应字段、或让可选变为必填，都必须按破坏性变更处理。
- 前后端构建号在同一部署窗口内由 `x-auction-build` 响应头与实时 WebSocket 首帧报告；构建号只用于提示刷新，不参与版本协商，也不放宽任何字段校验。
- 新增或删除路由时同步更新 [docs/contracts/routes.md](../../docs/contracts/routes.md)；实时事件的种类与负载变化同步更新 [docs/contracts/realtime.md](../../docs/contracts/realtime.md)。
- 本 crate 的任何改动都会触发两端浏览器 payload 重建（发布脚本的 Web 输入指纹包含本 crate）。


## 运行时开关只读投影

`RuntimeSwitchListResponse` 分开承载数据库最新意图与 Admin 自身应用状态；`RuntimeSwitchIntentDto` 和 `RuntimeSwitchAppliedDto` 由 HTTP 显式映射。revision 只表示该来源已观察事实，不能当作三角色共同确认。契约见[docs/contracts/routes.md#运行时开关只读接口](../../docs/contracts/routes.md#运行时开关只读接口)。
