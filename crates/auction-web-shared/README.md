# auction-web-shared

[文档导航](docs/README.md) · [Public 前端](crates/auction-web-public/README.md) · [Admin 前端](crates/auction-web-admin/README.md)

Public 与 Admin 两个 Dioxus CSR 前端共用的浏览器基础能力与设计地基。只编译到 `wasm32-unknown-unknown` 场景使用，不依赖任何后端 crate；两端各自的业务页面、译文与路由不放在这里。

## 模块

| 模块 | 内容 |
| --- | --- |
| [`crates/auction-web-shared/src/transport/`](crates/auction-web-shared/src/transport) | 同源 HTTP 请求的统一边界：10 秒总预算（响应头、正文读取与取消共用），`AbortController` 取消；`browser_lock` 封装原生 Web Locks 的有界独占守卫 |
| [`crates/auction-web-shared/src/bootstrap/`](crates/auction-web-shared/src/bootstrap) | 首屏启动门禁：WASM 就绪前显示静态骨架，就绪后在 `#main` 上标记 `data-app-ready` 切换 |
| [`crates/auction-web-shared/src/i18n/`](crates/auction-web-shared/src/i18n) | 语言集合（en、zh-CN、zh-TW、ja、ko）、`localStorage` 键 `hv.lang`、`<html lang>` 同步；Public 默认英文，Admin 默认简体中文 |
| [`crates/auction-web-shared/src/theme/`](crates/auction-web-shared/src/theme) | 深色（默认）/浅色主题，`localStorage` 键 `hv.theme`，经 `<html data-theme>` 生效 |
| [`crates/auction-web-shared/src/query/`](crates/auction-web-shared/src/query) | URL query 与组件状态双向同步、popstate 生命周期 |
| [`crates/auction-web-shared/src/visibility/`](crates/auction-web-shared/src/visibility) | 页面隐藏时挂起轮询与重连退避，恢复可见后继续 |
| [`crates/auction-web-shared/src/media/`](crates/auction-web-shared/src/media) | 把 VisualViewport 投影为 CSS 变量，供软键盘与缩放时的固定弹窗使用；断点切换只由 CSS `@media` 完成 |
| [`crates/auction-web-shared/src/clock/`](crates/auction-web-shared/src/clock)、[`crates/auction-web-shared/src/format/`](crates/auction-web-shared/src/format) | 秒级共享时钟；金额与比例的展示格式 |
| [`crates/auction-web-shared/src/commands/`](crates/auction-web-shared/src/commands) | 两阶段确认命令的泛型状态机（准备 → 确认 → 提交 → 结果/未知） |
| [`crates/auction-web-shared/src/components/`](crates/auction-web-shared/src/components) | 三态面板（加载/错误/空）、模态弹窗外壳、分页器（含每页数量预设）、安全 Markdown 渲染（链接校验）、长文本截断浮层、表格列比例与单元格提示、本地化时间 |
| [`crates/auction-web-shared/src/errors/`](crates/auction-web-shared/src/errors)、[`crates/auction-web-shared/src/validation/`](crates/auction-web-shared/src/validation) | 稳定错误码分类基础；wire 字段格式谓词（UUID、标识等） |
| [`crates/auction-web-shared/src/a11y/`](crates/auction-web-shared/src/a11y) | 通用无障碍交互（焦点管理等） |

## 样式

`crates/auction-web-shared/style/base.css` 是两端共享的设计地基：设计 token（颜色、字体栈、字号、间距、圆角、阴影、层级、触控尺寸、安全区）、reset 与通用组件样式；`crates/auction-web-shared/style/mobile.css` 为触控断点的通用覆盖。所有样式使用 `@layer foundation, surface, desktop, mobile` 分层，surface 的样式只能覆盖 token 或追加组件规则。设计约定详见 [Public 设计](crates/auction-web-public/DESIGN.md) 与 [Admin 设计](crates/auction-web-admin/DESIGN.md)。

`crates/auction-web-shared/assets/favicon.svg` 为两端共用图标。

## 约定

- 不引入 webfont 或任何第三方运行时资源；CSP 只允许同源脚本（另需 `wasm-unsafe-eval`），不能使用内联脚本。
- 偏好键（`hv.lang`、`hv.theme`）是与旧系统共享的契约，不得重命名。
- 共享组件只承载展示与交互机制，不持有业务数据或命令；业务状态由各 surface 的稳定宿主组件拥有。
