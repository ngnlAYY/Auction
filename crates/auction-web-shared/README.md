# 浏览器端共享基础能力

本 crate 只放 Public 与 Admin 都需要、且没有业务偏向的内容：

- `style/base.css`：primitive/semantic 设计变量（颜色、字号、间距、圆角、层级和表面阴影）、基础排版
  和语言切换器样式。两个 surface 的样式都建立在这些变量之上；窄屏表单
  控件保持可缩放字号，`prefers-reduced-motion` 会关闭非必要平滑滚动和过渡动画。
  另含三组从两端重复块提取的通用组件样式：`.ui-pager*` 分页器（与两端原规则逐字一致）、
  `.ui-async-state*`/`.ui-empty-state*` 状态面板（两者共享同一组规则），以及
  `.ui-table--responsive`（窄断点卡片模式，依赖 td 的 `data-label`）与 `.ui-table--scroll`
  （横向滚动容器）响应式表格工具类。
- `src/i18n/mod.rs`：受支持语言集合、`hv.lang` 偏好持久化、`<html lang>` 同步和
  `LanguagePicker` 组件。
- `src/components/mod.rs`：无领域语义的共享基础组件。
- `src/components/pager.rs`：`Pager` 长列表分页器，合并自两端原先近逐字重复的
  `PublicPager`/`AdminPager`。保留钳制契约：渲染期先 clamp 页码、交互从钳制页码出发、
  越界 effect 只纠偏一次；`total` 缺失时退化为 `has_more` 游标语义。上一页/下一页/跳页
  文案与「第 X 页 / 共 Y 页」状态模板全部由 props 传入（`status: Callback<(i64, i64), String>`），
  DOM class 保持 `ui-pager*` 不变。
- `src/components/async_state.rs`：`AsyncState` 三态面板（`AsyncStateKind::Loading` /
  `Error` / `Empty`），统一自 Public 的 `RetryPlaceholder`、`Placeholder` 与 Admin 的
  `AdminPanel`。loading/empty 用 `role="status"` + `aria-live="polite"`，error 用
  `role="alert"`；`retry_label` 与 `on_retry` 同时提供才渲染重试按钮。
- `src/components/time.rs`：`Time` 组件，包装 `format_localized_time` 并输出带原始
  RFC3339 `datetime` 属性的 `<time>` 元素。
- `src/media/mod.rs`：`use_media_query(query) -> Signal<bool>`，基于 `matchMedia` +
  `change` 监听的响应式媒体查询；非 WASM 构建返回恒 `false`。
- `src/clock/mod.rs`：秒级共享时钟。外壳用 `use_second_ticker()` 创建并提供 context，
  消费方用 `use_second_tick() -> Signal<u64>` 订阅同一修订计数，不再各自起一秒定时器；
  内部 interval 惰性启停（无订阅者时不运行），生命周期由外壳的 context 范围决定。
- `src/components/markdown/{mod,model,blocks,inline,render}.rs`：共享 Markdown/GFM 展示器的
  模型、块级解析、行内解析和安全 DOM 渲染；原始 HTML 只作为转义文本展示，链接目标经过
  协议白名单校验。`MarkdownDocument` 提供共享主题容器，`MarkdownBlocks` 只输出安全块节点，
  供必须由 surface 保留既有容器和排版的页面使用。
- `src/a11y/mod.rs`：共享 `aria-modal` 弹窗的动态初始聚焦、Tab 焦点循环、背景滚动锁和
  关闭后触发器焦点恢复，以及仅在 SPA 路由真正变化后聚焦主内容的通用 hook；首屏和同路由
  数据刷新不会抢占焦点，模块不包含 Public/Admin 业务语义。
- `src/bootstrap/mod.rs`：Public/Admin 共用的首屏启动门禁。静态骨架会持续覆盖尚未就绪的
  Dioxus 树，必要请求完成（包括进入错误结果）后只触发一次交叉淡出；后续刷新和认证状态变化
  不会重新遮住界面。
- `src/format/mod.rs`：Public/Admin 共用且不参与业务判断的金额展示规则。

## 语言选择

受支持语言为 `en`、`zh-CN`、`zh-TW`、`ja`、`ko`，顺序是本仓库冻结的浏览器契约。中文按脚本
区分简繁，两者不互相回退。

`localStorage` 键固定为 `hv.lang`，是已发布的兼容键，**不得重命名**。存储值必须在受支持集合内，否则回退到该 surface
的默认语言；存储不可用（隐私模式、策略禁用）时不让界面崩溃。

`LanguagePicker` 使用原生 `<select>`：它自带键盘操作、屏幕阅读器语义和移动端原生选择面板，
不需要额外的可访问性实现。组件同时显式同步当前 option 的 `selected` 状态，保证刷新后的原生
下拉显示与 `hv.lang`、`<html lang>` 一致。其 `label` 由调用方传入已翻译文本，因此本 crate
不含任何文案。

各 surface 自己拥有译文资源与默认语言常量（`DEFAULT_PUBLIC_LOCALE` / `DEFAULT_ADMIN_LOCALE`
在本 crate 定义，但译文在各 surface）。

`asset!` 的绝对路径按声明所在 crate 的包根解析，因此样式随本 crate 自包含分发，依赖它的
二进制在 `dx build` 时会自动收集，各 surface 不需要重复声明。

以下内容**不属于**本 crate，必须留在对应的 surface crate：

- 公开竞价相关组件与 `public-auction-*` 规则 → `auction-web-public`
- 管理命令相关组件与 `admin-*` 规则 → `auction-web-admin`

本 crate 不得依赖 `auction-storage`、`auction-config` 或任何凭据访问路径。
