# Admin 前端设计约定

[文档导航](docs/README.md) · [模块说明](crates/auction-web-admin/README.md) · [共享层](crates/auction-web-shared/README.md) · [Public 设计](crates/auction-web-public/DESIGN.md)

管理后台面向运营与运维人员：在一个高密度、可预测的界面里观察系统状态、处理异常、执行有版本约束的命令。与 Public 共享设计 token 与通用组件，但页面、样式选择器（`admin-*`）与文案完全独立。

## 原则

- **服务端是唯一的状态来源。** 页面只展示服务端投影与 capabilities；按钮是否出现、是否可用、可选动作集合都来自投影，浏览器不推导生命周期、结算或投递状态迁移。
- **写命令可追溯、可重试。** 每次提交都带 `command_id` 与期望版本；已提交命令由页面级未决宿主持有，结果未知时显示在“未决”区域，只能原样重试。
- **危险动作二次确认。** 取消场次、进入 Final、提前结算、撤下 lot、吊销 key、拉黑、投递对账等都经确认弹窗；提交中弹窗不可被 Escape、遮罩或关闭按钮打断。
- **不泄漏敏感信息。** 凭据 Cookie 只在录入表单中短暂存在；一次性登录 key 只显示一次；诊断信息只展示后端给出的安全分类与标识。
- **局部降级。** 仪表盘的各分区独立返回 `status` 与 `error_code`，某一分区不可用不影响其余分区展示。

## 视觉

- 继承共享 [`crates/auction-web-shared/style/base.css`](crates/auction-web-shared/style/base.css) 的深色默认主题、系统字体与 token；浅色主题同样只改写 token。
- Admin 专属 token 定义在 [`crates/auction-web-admin/assets/admin.css`](crates/auction-web-admin/assets/admin.css) 的 `:root`：表单控件中性灰边框与底色（`--color-input-border`、`--color-input-bg`）、高明度状态徽章三色（`--color-status-ok/warn/error`，配 12% 透明衬底）、危险按钮底色 `--color-danger-button`（白字对比约 5.7:1，与错误文字色分离），以及一组 `--admin-text-*`、`--admin-space-*` 密度变量。浅色主题在同一层覆盖这些 token。
- 表格是主要的信息载体：高密度、单行截断（`title` 保留全文）、长标识在容器内换行，不引入横向滚动。状态徽章必须同时包含文字。

## 布局与断点

| 条件 | 布局 |
| --- | --- |
| `width > 48rem` 且精确指针（或 `width > 64rem`） | 左侧固定 12rem 的竖向导航，内容区在右 |
| `width > 64rem` | 仪表盘分区按 2 栏排列 |
| `min-width: 100rem` | 仪表盘分区按 3 栏排列 |
| `max-width: 48rem`，或 `max-width: 64rem` 且 `pointer: coarse` | 手机布局：导航折叠为菜单，表单与网格单列，触控目标 2.75rem |
| `print` | 隐藏导航、头部动作、告警横幅与弹窗按钮组 |

仪表盘分区使用多栏（CSS columns）按 DOM 顺序逐栏阅读，避免等高网格留下空白。手机与桌面导航由 [`crates/auction-web-admin/src/app/surface/auth/navigation/desktop.rs`](crates/auction-web-admin/src/app/surface/auth/navigation/desktop.rs)、[`crates/auction-web-admin/src/app/surface/auth/navigation/mobile.rs`](crates/auction-web-admin/src/app/surface/auth/navigation/mobile.rs) 分别提供，只接收展示状态与事件；业务子树、表单草稿与未决命令只挂载一份，断点切换不会重新请求或丢失状态。

## 交互模式

| 模式 | 用法 |
| --- | --- |
| 列表 + 行内动作 | 场次、lot、凭据、用户、投递、审计等；行内按钮按 capabilities 显示 |
| 确认弹窗 | 共享 `.ui-dialog` 外壳；危险动作用 `admin-btn-danger`，说明后果并在需要时要求填写原因 |
| 手动刷新 + 自动轮询开关 | 仪表盘与实时事件页标题右侧，轮询在页面隐藏时暂停 |
| 先校验后提交 | 配置与规则：先请求服务端校验，得到规范化结果与 fingerprint/revision 后才允许保存或发布 |
| 先选择后展示 | 结算报表：先选世界与期号，再读取对应场次的权威报告 |
| 未决区 | 外壳中统一列出结果未知的命令及重试入口，布局变化不复制命令或替换 UUID |

## 表单

- 字段级校验：控件旁即时显示错误并通过 `aria-describedby` 关联，控件描边与错误文字同色系。
- 配置页把“业务世界即时开关”（写数据库，立即生效）与“runtime 配置”（写 runtime TOML，部分字段需重启 Worker 生效）明确分区。
- 首次部署页独立于认证外壳，宽屏双列、窄屏单列，不展示任何密钥摘要。

## 无障碍

- 使用原生表格语义（`caption`、`th scope`）、原生按钮与表单控件；表头排序按钮占满单元格，箭头仅作视觉提示（`aria-hidden`）。
- 键盘遍历行内操作时提供与悬停相同的行定位样式。
- 反馈区域成功使用 `role="status"`，失败使用 `role="alert"`。
