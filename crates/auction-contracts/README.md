# auction-contracts

[docs/README.md](../../docs/README.md) · [docs/contracts/routes.md](../../docs/contracts/routes.md) · [docs/contracts/realtime.md](../../docs/contracts/realtime.md)

Public/Admin HTTP、WebSocket 与首次设置接口的稳定 DTO。后端（`auction-http`、`auction-setup`）与浏览器（`auction-web-public`、`auction-web-admin`）共用同一份类型，任何字段变化都会同时影响两端编译，因此本 crate 只依赖 `serde`、`serde_json`、`chrono`、`uuid`，可编译到 `wasm32-unknown-unknown`。

DTO 只描述线上形状，不含业务规则；从应用投影到 DTO 的映射由 `auction-http` 负责。

## 结构

| 路径 | 内容 |
| --- | --- |
| [crates/auction-contracts/src/dto/response.rs](src/dto/response.rs) | 响应外壳 `ApiResponse`、错误外壳 `ApiErrorResponse`、健康探针、Public 实时事件信封、字段反序列化边界 |
| [crates/auction-contracts/src/dto/public](src/dto/public) | Public 账号与会话、拍卖目录（拍卖、lot、装备、补偿池、履约进度、论坛镜像、竞价日志与价格历史）、两端共用的装备大类对照表 `EQUIPMENT_CATEGORY_CODES`（公开分类键、规范大类名、lot 代码前缀）、两端共用的 Public 列表页长上限 `MAX_PUBLIC_PAGE_SIZE`（见[docs/contracts/routes.md#列表页长](../../docs/contracts/routes.md#列表页长)）、竞价与代理命令、装备标记与提醒偏好、一口价挂单（`consignment/`：服务状态、公开目录与详情、本人挂单的各种视图与解析进度、登记/改价/取消命令与回执；v1 路由与 v2 收件路由均已挂载，见[docs/contracts/routes.md#一口价挂单](../../docs/contracts/routes.md#一口价挂单)） |
| [crates/auction-contracts/src/dto/admin](src/dto/admin) | Admin 会话、拍卖与排期、lot、装备录入与词典、邮件状态与入站审查、规则、runtime 配置、凭据、用户与改名历史、投递、恢复、结算报表与目录、结算退件案件（`returns.rs`：列表、详情、处理命令与浏览器按闭集解码所用的状态、动作、阻断原因、原因码、资产位置与事件常量）、导航待办（`include=settlement_returns` 时才输出的退件计数）、realtime 运维、审计、世界启停、一口价挂单的运营视图与下架命令（`consignment/`，v1 路由与 v2 收件路由均已挂载，见[docs/contracts/routes.md#一口价挂单](../../docs/contracts/routes.md#一口价挂单)）；各列表共用的数字分页信封（严格/宽松两种解码，原名字保留为类型别名） |
| [crates/auction-contracts/src/setup/mod.rs](src/setup/mod.rs) | 首次设置请求与稳定结果码 |

## 线上约定

- 成功响应：`{"code": "...", "message": "...", "data": …}`；`ApiResponse::created` 使用 `created`，`ApiResponse::accepted` 使用 `accepted`，具体 HTTP 状态与 code 组合按[docs/contracts/routes.md#响应外壳](../../docs/contracts/routes.md#响应外壳)冻结，不能仅按“创建”动作推断。
- 错误响应：`{"code": "...", "message": "...", "data": null | {...}, "request_id": null | "..."}`；`code` 是稳定的机器可读值，客户端不应解析 `message`。Public/Admin 的关联标识始终在响应头 `x-request-id`，常规 handler 错误的 JSON 字段可能为 null。
- 健康探针：`/health/live`、`/health/ready` 返回 `{"code","message","data":{"live"|"ready": bool}}`；`/health` 与 `/api/v1/health` 直接返回 `{"service","status","version"}`，版本来自运行进程自身。首次设置也使用独立的 `SetupResponse` / `SetupErrorResponse`，不套业务 API 外壳。
- 时间统一为 RFC 3339 字符串；金额为整数 Credits；标识为规范小写 UUID 字符串。
- 浏览器默认按 JavaScript 安全整数范围校验整数。唯一例外是只用于展示的非负 `i64` 纳秒时长（Public 补偿逐件明细、补偿流水与 Admin 结算报告的 `leader_held_nanos`）。Rust 浏览器精确解码并按整数截断展示，Solid 按 `number` 近似解码并接受舍入后的上界 `2^63`。金额、计数、版本、身份、分页、排序和其他判定仍按各自原有契约校验。
- 部分字段为兼容已发布的 JSON 契约（沿用 Go `encoding/json` 的解码语义）保留了“必须出现但可为 null”或“null 视为零值”的反序列化语义，相关辅助函数在 [crates/auction-contracts/src/dto/response.rs](src/dto/response.rs) 中显式标注，新增字段不要随意放宽。`deserialize_required_option` 公开给 Admin 的严格镜像使用，只能经 `#[serde(deserialize_with = …)]` 生效（包装类型无法让字段成为必填）。
- 标有 `deny_unknown_fields` 的 DTO 在浏览器解码时拒绝额外字段；该约束不改变服务端序列化的字段集合。
- `SettledAuctionSummaryResponse` 对应[已结算场次摘要](../../docs/contracts/routes.md#已结算场次摘要)，四个键均须出现，`effective_end_at` 可为 `null`。完整响应样本见 [crates/auction-contracts/tests/fixtures/public/auction-summary.json](tests/fixtures/public/auction-summary.json)。
- 提供 `Debug` 的敏感 DTO 会隐藏登录 key、凭据 Cookie、Admin 口令及 CSRF token；首次设置凭据请求不实现 `Debug`。

## 变更规则

- 已发布字段不得改名或改变语义；新增字段优先设为可选，并同时更新后端映射与两端页面。
- 浏览器按闭集校验响应：Public 的路由专属 decoder 用 `auction-web-shared` 的 wire 谓词拒绝未知键，Admin 的严格镜像与带 `deny_unknown_fields` 的 DTO 同样拒绝额外字段；只有宽松解码的响应才忽略未知字段，新增字段不能依赖这一点。浏览器构建号不一致时只提示刷新，同一部署窗口内旧页面仍会读取新服务端：已发布路由的缺省响应多出一个键，按闭集解码的旧页面这次读取就会失败，直到刷新。
- 响应新增字段的兼容方式：缺省响应保持原字段集合，新字段只在新页面显式请求时输出，即新增可选查询参数（如 `/api/v1/auctions/current` 的 `include=proxy_default_mode`、lot 目录的 `fields=summary`）或新版本路由（如 `/api/v2/auctions`）。浏览器 payload 与服务端在同一制品中发布，新页面可以在同一版本里依赖它；代价在回滚方向：回到不认识该参数或路由的旧服务端后，新页面的这次读取被拒绝（未登记的查询参数为 400 `invalid_query`），直到按构建号提示刷新。删除或改名响应字段、或让可选变为必填，都必须按破坏性变更处理。
- 前后端构建号在同一部署窗口内由 `x-auction-build` 响应头与实时 WebSocket 首帧报告；构建号只用于提示刷新，不参与版本协商，也不放宽任何字段校验。
- 新增或删除路由时同步更新 [docs/contracts/routes.md](../../docs/contracts/routes.md)；实时事件的种类与负载变化同步更新 [docs/contracts/realtime.md](../../docs/contracts/realtime.md)。
- 本 crate 的任何改动都会触发两端浏览器 payload 重建（发布脚本的 Web 输入指纹包含本 crate）。


## 出站通知模板

`AdminMailTemplateListResponse` 按固定顺序给出每个模板键的 `AdminMailTemplateDto`：编辑用正文、内置默认正文、是否自定义、是否满足现行规则、版本与占位符集合；`AdminMailTemplateUpdateRequest` 的 `body` 为 `null` 表示恢复默认。数据库修订事实由 HTTP 显式映射，DTO 不暴露命令摘要或审计关联。契约见[docs/contracts/routes.md#出站通知模板](../../docs/contracts/routes.md#出站通知模板)。

## 运行时开关只读投影

`RuntimeSwitchListResponse` 分开承载数据库最新意图与 Admin 自身应用状态；`RuntimeSwitchIntentDto` 和 `RuntimeSwitchAppliedDto` 由 HTTP 显式映射。revision 只表示该来源已观察事实，不能当作三角色共同确认。契约见[docs/contracts/routes.md#运行时开关只读接口](../../docs/contracts/routes.md#运行时开关只读接口)。
