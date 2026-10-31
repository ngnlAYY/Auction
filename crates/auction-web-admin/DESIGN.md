# Admin 前端设计约定

[文档导航](../../docs/README.md) · [模块说明](README.md) · [共享层](../auction-web-shared/README.md) · [Public 设计](../auction-web-public/DESIGN.md)

管理后台面向运营与运维人员：在一个高密度、可预测的界面里观察系统状态、处理异常、执行有版本约束的命令。与 Public 共享设计 token 与通用组件，但页面、样式选择器（`admin-*`）与文案完全独立。

## 原则

- **服务端是唯一的业务状态来源。** 页面展示服务端投影，并结合状态、capabilities、排期与浏览器当前时间控制按钮；生命周期、结算、投递的最终迁移与权限由服务端决定。
- **业务命令可追溯、可重试。** 业务变更带 `command_id`，有并发约束时携带期望版本；多数已提交命令由页面级未决宿主持有，配置与规则命令由已认证外壳持有，凭据录入由敏感表单持有。结果未知时冻结原负载与 UUID，提供原样重试。登录、退出、校验与首次部署使用各自的请求协议。
- **危险动作二次确认。** 取消场次、进入 Final、提前结算、撤下 lot、吊销 key、拉黑、投递对账等都经确认弹窗；提交中弹窗不可被 Escape、遮罩或关闭按钮打断。
- **不泄漏敏感信息。** 凭据 Cookie 与论坛密码只驻留录入表单内存，未知结果时冻结保留，成功后清空；一次性登录 key 由稳定页面交付，操作员确认前阻止新 Key 请求覆盖结果；诊断信息只展示后端给出的安全分类与标识。
- **局部降级。** 仪表盘的各分区独立返回 `status` 与 `error_code`，某一分区不可用不影响其余分区展示。

## 视觉

- 继承共享 [`crates/auction-web-shared/style/base.css`](../auction-web-shared/style/base.css) 的深色默认主题、系统字体与 token；浅色主题同样只改写 token。
- Admin 专属 token 定义在 [`crates/auction-web-admin/assets/admin.css`](assets/admin.css) 的 `:root`：表单控件中性灰边框与底色（`--color-input-border`、`--color-input-bg`）、高明度状态徽章三色（`--color-status-ok/warn/error`，配 12% 透明衬底）、危险按钮底色 `--color-danger-button`（白字对比约 5.7:1，与错误文字色分离），以及一组 `--admin-text-*`、`--admin-space-*` 密度变量。浅色主题在同一层覆盖这些 token。
- 表格是主要的信息载体：高密度、单行截断（`title` 保留全文）、长标识在容器内换行，不引入横向滚动。状态徽章必须同时包含文字。

## 布局与断点

| 条件 | 布局 |
| --- | --- |
| `width > 48rem` 且指针为 `fine` 或 `none`，或者 `width > 64rem` | 左侧 12rem 宽的竖向导航，内容区在右；导航随正常文档流布局 |
| `width > 64rem` | 仪表盘分区按 2 栏排列 |
| `min-width: 100rem` | 仪表盘分区按 3 栏排列 |
| `max-width: 48rem`，或 `max-width: 64rem` 且 `pointer: coarse` | 手机导航折叠为菜单；窄屏表单按各自规则重排，一般触控目标 2.75rem，分页控件为 2rem |
| `print` | 隐藏导航、头部动作、告警横幅与弹窗按钮组 |

仪表盘分区使用多栏（CSS columns）按 DOM 顺序逐栏阅读，避免等高网格留下空白。手机与桌面导航由 [`crates/auction-web-admin/src/app/surface/auth/navigation/desktop.rs`](src/app/surface/auth/navigation/desktop.rs)、[`crates/auction-web-admin/src/app/surface/auth/navigation/mobile.rs`](src/app/surface/auth/navigation/mobile.rs) 分别提供，只接收展示状态与事件；业务子树、表单草稿与未决命令只挂载一份，断点切换不会重新请求或丢失状态。

## 交互模式

| 模式 | 用法 |
| --- | --- |
| 列表 + 行内动作 | 场次、lot、凭据、用户、投递、审计等；按钮结合服务端状态与 capabilities 显示，筛选与分页不替换已提交命令 |
| 确认弹窗 | 共享 `.ui-dialog` 外壳；危险动作用 `admin-btn-danger`，说明后果并在需要时要求填写原因 |
| 手动刷新 + 自动轮询开关 | 仪表盘与实时事件页标题右侧，轮询在页面隐藏时暂停 |
| 先校验后提交 | 配置与规则先请求服务端规范化、校验当前草稿，再以已有 fingerprint/revision 基线保存或发布；编辑草稿后撤回校验状态 |
| 先选择后展示 | 结算报表：先选世界与期号，再读取对应场次的权威报告 |
| 未决区 | 各稳定 owner 的 `PendingCommandPanel` 展示已提交命令的执行中、拒绝或未知状态及原样重试入口；布局变化不复制命令或替换 UUID，未知状态不能直接忽略 |

分页器与查询状态由各页面拥有。`use_admin_page` 只展示请求身份与每页数量均匹配的结果；邮箱轮询、邮件详情等手工资源也绑定各自目标，切换时不沿用旧实体快照。URL 写回只替换所属查询键，保留其他分区参数；实时事件、失败与 lag 分区独立分页，数量偏好分别保存。手机保留相邻页导航与数量选择，隐藏自定义数量和跳页输入。

## 表单

- 字段级校验：控件旁即时显示错误并通过 `aria-describedby` 关联，控件描边与错误文字同色系。
- 配置页把“业务世界即时开关”（写数据库，立即生效）与“runtime 配置”（写 runtime TOML，部分字段需重启 Worker 生效）明确分区。
- 首次部署页独立于认证外壳，宽屏双列、窄屏单列，不展示任何密钥摘要。

## 无障碍

- 使用原生表格语义（`caption`、`th scope`）、原生按钮与表单控件；表头排序按钮占满单元格，箭头仅作视觉提示（`aria-hidden`）。
- 键盘遍历行内操作时提供与悬停相同的行定位样式。
- 反馈区域成功使用 `role="status"`，失败使用 `role="alert"`。
