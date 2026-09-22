# 浏览器端共享基础能力

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

两个浏览器 surface 共享的无业务偏向基础能力。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [设计变量与组件](#设计变量与组件)
- [命令与浏览器状态](#命令与浏览器状态)
- [传输与错误](#传输与错误)
- [Markdown、可访问性与启动](#markdown可访问性与启动)
- [语言选择](#语言选择)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/components](src/components) | 表格、分页、对话框、状态、时间和安全 Markdown |
| [src/transport](src/transport) | 同源 HTTP 传输 |
| [src/commands](src/commands) | 可重试确认命令状态 |
| [src/i18n](src/i18n) | 语言集合和 hv.lang |
| [src/a11y](src/a11y) | 弹窗焦点及背景滚动 |
| [style](style) | 基础设计变量与样式 |

## 职责与实现边界

本 crate 只放 Public 与 Admin 都需要、且没有业务偏向的内容：

## 设计变量与组件

- [style/base.css](style/base.css)：primitive/semantic 设计变量（颜色、字号、间距、圆角、层级和表面阴影）、基础排版
  和语言切换器样式。两个 surface 的样式都建立在这些变量之上；窄屏表单
  控件保持可缩放字号，`prefers-reduced-motion` 会关闭非必要平滑滚动和过渡动画。
  另含四组从两端重复块提取的通用组件样式：`.ui-pager*` 分页器（与两端原规则逐字一致）、
  `.ui-async-state*`/`.ui-empty-state*` 状态面板（两者共享同一组规则）、
  所有业务与 Markdown 表格采用 100% 宽度、百分比列宽和固定表格布局；每张业务表按预计内容
  独立分配完整列比例。窄屏保留表头与行结构，超长数据和控件文字截断，不提供横向滚动条；
  `.ui-table--responsive` 仅保留兼容类名，`.ui-table--fit` 为受容器约束的包装类。
  另有 `.ui-dialog*` 模态弹窗外壳（含 `.ui-dialog--sheet`
  显式抽屉变体；bp-sm 以下媒体查询自动切换为贴底抽屉）。
- [src/i18n/mod.rs](src/i18n/mod.rs)：受支持语言集合、`hv.lang` 偏好持久化、`<html lang>` 同步和
  `LanguagePicker` 组件。
- [src/components/mod.rs](src/components/mod.rs)：无领域语义的共享基础组件。
- [src/components/table_columns.rs](src/components/table_columns.rs)：`TableColumns` 将调用方的列权重归一化为百分比并输出
  `colgroup`。业务组件拥有各自的比例，见 [表格列宽登记](../../docs/table-layouts.md)；
  Markdown 按表头与前 32 行可见长度的第 75 百分位估算权重，并限制极端长文本的影响。
- [src/components/table_hints.rs](src/components/table_hints.rs)：由 `BaseStylesheet` 安装一次的表格提示委托。普通单元格、表头
  与动作文字提供原生完整文本提示，当前悬浮内容随 DOM 文本更新；已声明的 title 和复杂文本
  浮层保持各自所有权，不复制表单输入或弹窗内容到 title。
- [src/components/truncated_text.rs](src/components/truncated_text.rs) 与 [src/components/truncated_text/interaction.rs](src/components/truncated_text/interaction.rs)：复杂长文本的
  截断展示、180 毫秒悬停宽限、点击/Enter/Space 固定、Escape 关闭与浮层内滚动选择。
  内联和 portal 浮层共享事件清理；移入浮层不会提前关闭，页面滚动或外部点击会关闭定位浮层。
  原生 `mouseleave`/`focusout` 事件通过组件的稳定回调恢复 Dioxus Runtime 与 Scope，再创建
  延迟关闭任务；组件卸载同时取消任务并移除监听，避免原生回调直接 `spawn` 引发 WASM panic。
  `focusable=false` 的链接内文本保留链接行为；普通文本和操作提示不引入第二套业务状态。
- [src/components/pager.rs](src/components/pager.rs)：`Pager` 长列表分页器，合并自两端原先近逐字重复的
  `PublicPager`/`AdminPager`。保留钳制契约：渲染期先 clamp 页码、交互从钳制页码出发、
  挂载及页码、总量变化后同步跳页输入并通知父级纠偏；`total` 缺失时退化为 `has_more` 游标语义。上一页/下一页/跳页
  文案与「第 X 页 / 共 Y 页」状态模板全部由 props 传入（`status: Callback<(i64, i64), String>`），
  DOM class 保持 `ui-pager*` 不变。
- [src/components/async_state.rs](src/components/async_state.rs)：`AsyncState` 三态面板（`AsyncStateKind::Loading` /
  `Error` / `Empty`），统一自 Public 的 `RetryPlaceholder`、`Placeholder` 与 Admin 的
  `AdminPanel`。loading/empty 用 `role="status"` + `aria-live="polite"`，error 用
  `role="alert"`；`retry_label` 与 `on_retry` 同时提供才渲染重试按钮。
- [src/components/dialog.rs](src/components/dialog.rs)：`Dialog` 模态弹窗外壳，统一自 Public 竞价/代理弹窗与
  Admin 的 12 处弹窗。提供遮罩、标题栏（`aria-labelledby` 接线）、关闭按钮与
  Escape / 遮罩点击 / 关闭按钮三条关闭路径；`busy=true` 时三条路径全部禁用（与两端
  提交中禁止关闭的守卫语义一致）。组件只产出 markup 与关闭语义，焦点循环、初始聚焦、
  关闭后焦点恢复与背景滚动锁仍由外壳层 `use_dialog_focus_trap` 全局处理；两端迁移时
  统一以 `true` 调用（原 public=true / admin=false 不一致，统一锁定背景滚动）。移动形态
  由 base.css 的 bp-sm 媒体查询自动切换为贴底抽屉，组件不感知断点。

## 命令与浏览器状态

- [src/commands/mod.rs](src/commands/mod.rs)：`use_confirm_command` 泛型两阶段确认命令状态机，吸收自 Public
  的 bid/proxy create/proxy authorization 三份骨架与 Admin 的 `stable_command_id`。
  `begin(签名, build)` 在签名不变时复用已冻结的 `command_id`、变更时重新生成；
  `submit` 成功清空全部命令状态，失败保留输入、`command_id` 与错误并退出确认态。
  调用方负责分类结果是否不确定；`uncertain` 在原命令重试期间保持，成功取得原结果或取消后清除，
  供 surface 展示只读的冻结负载重试入口，避免用新状态校验阻断幂等重放。
  后续 401/429 等前置拒绝不能清除先前的不确定状态。提交任务归属 hook 创建时的宿主作用域，
  表单子组件替换不取消请求；宿主卸载仍会取消任务，不使用越过身份作用域的全局任务。
  文案、负载形状、错误类型与提交动作全部由调用方决定。
- [src/components/time.rs](src/components/time.rs)：`Time` 组件，包装 `format_localized_time` 并输出带原始
  RFC3339 `datetime` 属性的 `<time>` 元素。
- [src/media/mod.rs](src/media/mod.rs)：`use_media_query(query) -> Signal<bool>`，基于 `matchMedia` +
  `change` 监听的响应式媒体查询；非 WASM 构建返回恒 `false`。
- [src/clock/mod.rs](src/clock/mod.rs)：秒级共享时钟。外壳用 `use_second_ticker()` 创建并提供 context，
  消费方用 `use_second_tick() -> Signal<u64>` 订阅同一修订计数，不再各自起一秒定时器；
  内部 interval 惰性启停（无订阅者时不运行），生命周期由外壳的 context 范围决定。
- [src/query/mod.rs](src/query/mod.rs)：`use_query_state(keys, parse, serialize)` URL query 双向同步 hook，
  收敛 Admin settlement 页的直写 history 模式与 Public 外壳的 popstate 恢复逻辑。挂载时
  从地址栏解析初值，`push`/`replace` 写回（只替换本 hook 声明管理的键，同页多实例互不
  覆盖），popstate 时经同一 `parse` 路径恢复状态；`read_query_pairs`/`write_query_pairs`
  为低层读写原语。状态形状与键名全部由调用方注入，与 Dioxus Router 共存（不接管路由）。
  原生 popstate 监听通过组件的稳定回调恢复 Runtime 与 Scope，允许解析器读取调用方的
  Context；监听器随组件卸载移除，避免公开端返回无 query 地址时在运行时之外消费 Context。

## 传输与错误

- [src/transport/mod.rs](src/transport/mod.rs)：同源请求共享传输边界，合并两端逐行相同的 fetch 封装。
  `send_get`/`send_request` 以统一 10 秒预算完整读取响应头和正文，再返回保留状态与响应头的内存响应；
  超时或调用方任务取消时由 `AbortController` 终止 fetch。正文解码仍由各 surface 拥有。
  `send_get_observed`/`send_request_observed` 供 Public 在响应头到达时校时，避免正文传输延迟污染时钟样本。传输层同时处理
  gloo-net 0.6.0 空 QueryParams workaround；错误为结构化 `TransportError`
  （`Timeout`/`Network(Option<u16>)`/`Decode`），`code()` 给出契约稳定 transport code，
  彻底取代原先的 "请求超时" 字符串比较。Admin 的 CSRF/Basic Auth 不进本层，由 surface
  在 `build` 闭包叠加 header。
- [src/errors/mod.rs](src/errors/mod.rs)：稳定错误码的通用分类基础层。`classify_base_error` 只识别两端共享的
  通用码（传输层三码、`http_4xx/5xx` 兜底、`*_conflict`/`stale_*`/`invalid_*` 模式）并归为
  `BaseErrorKind`，业务码返回 `None`；`base_api_error_message(code, callback)` 由 surface
  回调按类别取 `t!` 文案（与 `Pager` 的 props 注入同一模式），本 crate 不含任何译文。

## Markdown、可访问性与启动

- `src/components/markdown/{mod,model,blocks,inline,render}.rs`：共享 Markdown/GFM 展示器的
  模型、块级解析、行内解析和安全 DOM 渲染；原始 HTML 只作为转义文本展示，链接目标经过
  协议白名单校验。`MarkdownDocument` 提供共享主题容器，`MarkdownBlocks` 只输出安全块节点，
  供必须由 surface 保留既有容器和排版的页面使用。
- [src/a11y/mod.rs](src/a11y/mod.rs)：共享 `aria-modal` 弹窗的动态初始聚焦、Tab 焦点循环、背景滚动锁和
  关闭后触发器焦点恢复，以及仅在 SPA 路由真正变化后聚焦主内容的通用 hook；首屏和同路由
  数据刷新不会抢占焦点。恢复触发器焦点前先释放共享状态借用，避免同步 `focusin` 写回时
  重入冲突导致页面异常；模块不包含 Public/Admin 业务语义。
- [src/bootstrap/mod.rs](src/bootstrap/mod.rs)：Public/Admin 共用的首屏启动门禁。静态骨架会持续覆盖尚未就绪的
  Dioxus 树，必要请求完成（包括进入错误结果）后只触发一次交叉淡出；后续刷新和认证状态变化
  不会重新遮住界面。等待超过 15 秒只提示加载较慢；仅真实 WASM panic 会设置 `data-app-failed`
  并显示运行异常横幅，后续就绪回调不能覆盖失败状态，控制台保留原始诊断。
- [src/format/mod.rs](src/format/mod.rs)：Public/Admin 共用且不参与业务判断的金额展示规则。K/M/B 缩写必须精确，
  M/B 最多保留三位小数并正确补前导零；不能把 1.05M 显示成 1.5M。完整整数格式另供提示和
  可访问名称使用，不以浮点或四舍五入掩盖真实价格。

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

## 维护与验证

修改本模块时同步对应[契约](../../docs/contracts/README.md)与源码入口；跨模块理由按[决策记录规则](../../docs/documentation.md)维护。

```bash
cargo check --locked -p auction-web-shared --target wasm32-unknown-unknown
```

该命令只做静态编译；完整门禁见[开发指南](../../docs/development.md)，运行证据边界见[验证状态](../../docs/status.md)。
