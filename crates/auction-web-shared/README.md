# auction-web-shared

[docs/README.md](../../docs/README.md) · [crates/auction-web-public/README.md](../auction-web-public/README.md) · [crates/auction-web-admin/README.md](../auction-web-admin/README.md)

Public 与 Admin 两个 Dioxus CSR 前端共用的浏览器基础能力与设计地基。运行目标为 `wasm32-unknown-unknown`，同时保留非 WASM 静态编译分支；后者不提供等价的浏览器运行行为（例如 URL 写回为空操作，HTTP 不启用浏览器超时取消）。不依赖任何后端 crate；两端各自的业务页面、译文与路由不放在这里。

## 模块

| 模块 | 内容 |
| --- | --- |
| [crates/auction-web-shared/src/net/transport](src/net/transport) | 同源 HTTP 请求的统一边界：默认 10 秒响应头与正文读取预算，超时通过 `AbortController` 取消并等待请求收口，组件任务取消也中止 fetch；允许单次请求显式指定预算，不自动重试；`browser_lock` 封装原生 Web Locks 的有界独占守卫 |
| [crates/auction-web-shared/src/ui/bootstrap](src/ui/bootstrap) | 首屏启动门禁：各 surface 判定必要请求已有结果（含失败）后，在 `#main` 标记 `data-app-ready` 揭示真实页面；panic hook 写入优先的 `data-app-failed`，显示静态错误横幅；另有闭集浏览器错误信标与构建号工具 |
| [crates/auction-web-shared/src/ui/build_notice](src/ui/build_notice) | 部署后旧页面检测：比较 `x-auction-build` 响应头/实时首帧与自身构建号，不一致时只提示刷新，不自动重载；一次性唤醒与无锁快路径 |
| [crates/auction-web-shared/src/platform/storage](src/platform/storage) | 浏览器 `localStorage` 的容错读写（`get_item`/`set_item`/`remove_item`/`keys`/`get_parsed`）：不可用、配额或损坏只返回失败/缺失，不 panic；单条 JSON 解析有 2 MB 上限；非 WASM 构建恒为空操作，不持有键名 |
| [crates/auction-web-shared/src/net/wire](src/net/wire) | wire 格式共享谓词：JSON 安全整数边界与严格键集合/闭包键集合校验，供两端 decoder 拒绝缺字段、未知字段与失真整数 |
| [crates/auction-web-shared/src/ui/i18n](src/ui/i18n) | 语言集合（en、zh-CN、zh-TW、ja、ko）、`localStorage` 键 `hv.lang`、`<html lang>` 同步；Public 默认英文，Admin 默认简体中文 |
| [crates/auction-web-shared/src/platform/theme](src/platform/theme) | 深色（默认）/浅色主题，`localStorage` 键 `hv.theme`，经 `<html data-theme>` 生效 |
| [crates/auction-web-shared/src/platform/sidebar](src/platform/sidebar) | 控制台侧栏收起偏好，`localStorage` 键 `hv.sidebar`；只接受 `collapsed`/`expanded`，缺失或损坏回退展开；纯展示偏好，不参与业务判断 |
| [crates/auction-web-shared/src/net/query](src/net/query) | URL query 与组件状态双向同步、popstate 生命周期；只改写调用方声明的键，保留其他 query、片段与 history state，用户操作用 push、程序纠偏用 replace |
| [crates/auction-web-shared/src/platform/visibility](src/platform/visibility) | 页面隐藏时挂起轮询与重连退避的剩余等待，恢复可见后继续；是否立即补读由 surface 回调决定，不主动关闭已有连接 |
| [crates/auction-web-shared/src/platform/media](src/platform/media) | 把 VisualViewport 投影为 CSS 变量，供软键盘与缩放时的固定弹窗使用；断点切换只由 CSS `@media` 完成 |
| [crates/auction-web-shared/src/platform/clock](src/platform/clock)、[crates/auction-web-shared/src/ui/format](src/ui/format) | 秒级共享时钟；金额与比例的展示格式 |
| [crates/auction-web-shared/src/net/commands](src/net/commands) | 两阶段确认命令的泛型状态机（准备 → 确认 → 提交 → 结果/未知） |
| [crates/auction-web-shared/src/ui/components](src/ui/components) | 三态面板（加载/错误/空）、模态弹窗外壳、分页器（`total` 稳定分页与 `has_more` 游标投影、`max_page` 导航边界、可选数字页码/每页数量/自定义正整数，选项、文案与上限由调用方注入）、安全 Markdown 子集（不执行原始 HTML，校验链接）、长文本截断浮层、表格列比例与单元格提示、本地化时间 |
| [crates/auction-web-shared/src/net/errors](src/net/errors)、[crates/auction-web-shared/src/ui/validation](src/ui/validation) | 稳定错误码分类基础；共享 UUID wire 格式谓词，其他业务字段校验留在各 surface |
| [crates/auction-web-shared/src/platform/a11y](src/platform/a11y) | 通用无障碍交互（焦点管理等） |

## 样式

`crates/auction-web-shared/style/base.css` 是两端共享的设计地基：设计 token（颜色、字体栈、字号、间距、圆角、阴影、层级、触控尺寸、安全区）、reset 与通用组件样式；`crates/auction-web-shared/style/mobile.css` 为触控断点的通用覆盖。所有样式使用 `@layer foundation, surface, desktop, mobile` 分层，surface 的样式只能覆盖 token 或追加组件规则。设计约定详见 [crates/auction-web-public/DESIGN.md](../auction-web-public/DESIGN.md) 与 [crates/auction-web-admin/DESIGN.md](../auction-web-admin/DESIGN.md)。

手机分页控件采用 2rem 高度，只保留当前及相邻页、前后按钮、总数与每页数量选择；自定义数量与跳页表单按 CSS 隐藏。`BaseStylesheet` 同时安装表格完整值提示与 VisualViewport 生命周期；它不负责主题或语言状态，这两项由各 surface 根组件分别安装。

`crates/auction-web-shared/assets/favicon.svg` 为两端共用图标。

## 约定

- 不引入 webfont 或任何第三方运行时资源；CSP 只允许同源脚本（另需 `wasm-unsafe-eval`），不能使用内联脚本。
- 偏好键（`hv.lang`、`hv.theme`、`hv.sidebar`）是与旧系统共享的契约，不得重命名。
- 共享组件只承载展示与交互机制，不读取业务 API 或决定业务规则；泛型 `ConfirmCommand` 可保存调用方注入的冻结负载与 UUID，其状态和提交任务的生命周期归各 surface 创建它的稳定宿主。
- 长文本截断组件全页共用一个 window `resize` 监听和一个 `ResizeObserver`：各实例只登记自己的重测回调，卸载时注销，最后一个实例卸载后释放监听；浮层“窗口变化即关闭”的监听仍按打开的浮层单独注册。
- Markdown 行内解析对链接和各类强调标记分别缓存后续闭合位置；未闭合标记与大量单星号不能反复扫描同一后缀，递归仍限制为八层。超长链接按原文本展示，不截断正文。
