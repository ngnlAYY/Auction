# auction-web-admin

[文档导航](../../docs/README.md) · [设计约定](DESIGN.md) · [HTTP 路由](../../docs/contracts/routes.md) · [Admin API 应用](../../apps/auction-admin-api/README.md)

管理后台：Dioxus 0.7 客户端渲染（CSR）WASM 应用，由 `auction-admin-api` 的 `web-embed` 构建内嵌并同源提供。与公开站点是相互独立的 surface：独立的组件树、路由、样式与译文，不复用 Public 组件，也不导入存储、配置或凭据访问代码。界面默认简体中文。

## 登录与会话

- 登录表单以 HTTP Basic 方式把 actor 与 secret 提交到 `POST /api/v1/admin/session`；二者来自 Admin 敏感配置的 `admin_actor` / `admin_secret`（未配置时 Admin 无法登录）。
- 成功后服务端下发 HttpOnly 会话 Cookie 与 CSRF Cookie，并在会话 DTO 中返回 CSRF token；前端使用 DTO 中的 token。已认证的业务写入及退出在 `x-csrf-token` 头回传该值；登录和首次部署使用各自的认证路径。
- 外壳同时读取 `/health/ready`：未就绪时显示降级状态。会话被确认前不允许提交写命令；临时会话重读失败保留原草稿与未决命令并撤回写许可，确认身份（actor 或 CSRF token）变化时销毁整棵已认证页面状态。

## 页面

导航顺序与页面职责：

| 路径 | 名称 | 内容 |
| --- | --- | --- |
| `/` | 仪表盘 | 一次读取聚合快照：健康与缺失进程、Worker control、进程心跳、邮件轮询、realtime、凭据、投递、履约、恢复与告警分区；可暂停/恢复收件或立即触发一次轮询 |
| `/mails` | 收件审核 | 各邮箱的轮询进度与诊断，入站邮件列表、处理结果与详情 |
| `/auctions` | 拍卖场次 | 场次列表与生命周期动作（预览、开拍、进入 Final、提前结算、取消）、修改排期、新建场次、论坛同步状态、手工录入装备 |
| `/auctions/{auction_id}/lots` | lot 列表 | 草稿 lot 的撤下与起拍价修改、代理事件历史、手工录入 |
| `/cookies` | Cookie 池 | 邮件/论坛凭据的 Cookie 录入或论坛账号登录获取、校验、改名、启用、停用与删除；游戏 CK 按所选世界提取，secret/密码只驻留当前录入表单内存 |
| `/users` | 用户与 Key | 创建用户、签发/重新签发/吊销登录 key、绑定论坛账号 ID、拉黑与解除 |
| `/renames` | 改名回滚 | 改名历史与回滚 |
| `/deliveries` | 投递队列 | 邮件 outbox：释放、重试，以及对结果未知投递的人工对账（成功/失败/重试） |
| `/outbox` | 实时事件 | realtime 事件页、消费 lag、失败记录与重放 |
| `/settlement` | 结算报表 | 选择世界与期号后查看结算与恢复报告，确认恢复动作 |
| `/config` | 参数配置 | 业务世界即时开关；runtime 配置的类型化编辑、服务端校验与原子写入 |
| `/rules-content` | 规则内容 | 五种语言规则文档的编辑、校验、预览与发布 |
| `/audit` | 审计日志 | 只读审计记录的筛选与分页 |
| `/status` | 服务说明 | 静态说明页 |
| `/setup` | 首次部署 | 不经过登录外壳；部署码验证通过后填写初始配置（见 [auction-setup](../auction-setup/README.md)） |

兼容别名以 `replace` 跳转到正式地址并保留查询与片段：`/dashboard → /`、`/auction → /auctions`、`/mail → /mails`、`/credentials → /cookies`、`/delivery → /deliveries`、`/rules → /rules-content`；已登录访问 `/login` 按安全的 `return` 参数跳回站内目标，缺省回到仪表盘。其余路径显示 404 页面。新增页面须同步修改 `crates/auction-web-admin/src/app/surface/router.rs`、后端 [`crates/auction-http/src/assets/routes.rs`](../auction-http/src/assets/routes.rs) 与[路由文档](../../docs/contracts/routes.md)。

## 写命令约定

- 业务变更命令带浏览器生成的 `command_id`，需要并发约束的操作还携带服务端投影给出的版本（`expected_state_version`、`expected_fencing_version`、配置 fingerprint 等）；登录、退出、校验与首次部署不套用统一业务命令形状。
- 多数已提交命令由页面级“未决命令”宿主持有：行或弹窗卸载不会取消请求；结果不确定时保留原负载，只能原样重试，不会换新 `command_id`。宿主离开页面时销毁；配置与规则草稿及其发布命令由已认证外壳持有，可跨站内页面保留。凭据录入含敏感负载，使用当前表单自己的冻结与重试状态。
- 状态迁移与最终权限校验由服务端完成；按钮结合服务端状态、capabilities 和排期投影控制可用性，浏览器的展示判断不构成写入授权。
- 敏感值（凭据 Cookie、论坛密码、一次性登录 key）只驻留内存，不写入 `localStorage`、URL 或日志；凭据录入结果未知时保留原表单用于原样重试，成功后清空敏感字段。

列表读取绑定筛选、实体、页码与每页数量，旧请求快照只供同查询后台刷新展示。共享分页器的数量预设为 10/20/50/100，支持端点上限内的自定义正整数；URL 显式数量优先于各表的独立本地偏好，手机隐藏自定义数量与跳页输入。数量变化回到对应列表首页，不清空未决命令。

## 结构

| 路径 | 内容 |
| --- | --- |
| `crates/auction-web-admin/src/api/` | 同源 HTTP 访问、CSRF、DTO 解码，按 auction/content/fulfillment/identity/operations 划分 |
| `crates/auction-web-admin/src/app/surface/` | 路由、认证外壳与导航（桌面/手机）、分页、未决命令宿主 |
| `crates/auction-web-admin/src/app/auction/` | 拍卖场次、lot、手工录入、结算与恢复 |
| `crates/auction-web-admin/src/app/fulfillment/` | 收件审核、投递队列 |
| `crates/auction-web-admin/src/app/identity/` | 凭据、用户、改名 |
| `crates/auction-web-admin/src/app/operations/` | 仪表盘、实时事件、配置、审计、服务说明、首次部署 |
| `crates/auction-web-admin/src/app/content/` | 规则文档 |
| `crates/auction-web-admin/assets/` | `crates/auction-web-admin/assets/admin.css`、`crates/auction-web-admin/assets/desktop.css`、`crates/auction-web-admin/assets/mobile.css` |
| `crates/auction-web-admin/i18n/` | Fluent 译文：`zh-CN`（默认）、`en`、`zh-TW`、`ja`、`ko`，键集合与 `crates/auction-web-admin/i18n/en.ftl` 一致 |

## 构建

```bash
cargo check -p auction-web-admin --target wasm32-unknown-unknown
bash ci/release/web-bundle.sh admin
```

页面标题固定为“Auction 管理站点”，发布门禁据此识别 payload。`ci/release/web-bundle.sh` 验证受控工具后用 `wasm-release` profile 生成 `target/dx/build/auction-web-admin/web/public`，并绑定源码、工具、环境指纹及 payload 清单；完整门禁通过才复用已有产物。随后构建 `auction-admin-api` 的 `web-embed` 制品才能内嵌页面，单独编译 API 不生成 Web payload。前端只依赖契约 DTO 与共享浏览器机制，不启用 Dioxus fullstack/server，也不建立 WebSocket 连接。
