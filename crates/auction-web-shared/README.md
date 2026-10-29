# 浏览器端共享基础能力

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

Public 与 Admin 共用、无业务偏向的浏览器基础能力。只共享稳定的基础组件和浏览器机制：业务负载、错误文案、
Public 竞价与 Admin 管理状态留在各 surface，本 crate 不依赖存储、配置或凭据访问路径，也不拥有两端 Fluent 译文。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/components](crates/auction-web-shared/src/components) | 表格、分页、弹窗、状态、时间与安全 Markdown |
| [src/commands](crates/auction-web-shared/src/commands)、[src/query](crates/auction-web-shared/src/query) | 确认命令与 URL 状态 |
| [src/transport](crates/auction-web-shared/src/transport)、[src/errors](crates/auction-web-shared/src/errors) | 同源传输与通用错误分类 |
| [src/i18n](crates/auction-web-shared/src/i18n)、[src/format](crates/auction-web-shared/src/format) | 语言偏好、时间、金额与 ppm 百分比展示 |
| [src/a11y](crates/auction-web-shared/src/a11y)、[src/bootstrap](crates/auction-web-shared/src/bootstrap) | 焦点、背景滚动与首屏门禁 |
| [src/clock](crates/auction-web-shared/src/clock)、[src/media](crates/auction-web-shared/src/media) | 共享时钟与可见视口 |
| [src/visibility](crates/auction-web-shared/src/visibility) | 页面可见性感知等待与分片睡眠，供 Public 外壳和 Admin 自动轮询共用 |
| [style/base.css](crates/auction-web-shared/style/base.css)、[style/mobile.css](crates/auction-web-shared/style/mobile.css) | 共享设计变量／组件与独立移动规则 |

## 设计变量与组件

`base.css` 提供颜色、字号、间距、圆角、层级等变量及 `.ui-*` 样式，`mobile.css` 拥有触控字段、分页和底部弹窗规则；
两端业务 BEM 类与设备展示留在各自 assets。所有样式显式声明 `foundation, surface, desktop, mobile` 层顺序，
不依赖 Dioxus 不保证的 head 插入顺序；分别作为静态 CSS asset 打包，不使用未被当前 dx 递归打包的相对 `@import`。
共享组件不再提供 `.ui-empty-state*` 兼容别名；Public 专属空态仍由本端维护。
同时移除无人消费的强制 sheet 变体与旧表格结构恢复规则。
颜色对比按三层深色底计算：`--color-border-control` 是输入框、选择器等控件的边界色，对 bg/surface/raised
均不低于 3:1（WCAG 1.4.11），普通分隔线仍用更暗的 `--color-border`；`--color-danger` 只用于错误文字和边框，
在 raised 底上不低于 4.5:1，白字危险底色由各 surface 的深红变量提供。系统声明 `prefers-contrast: more` 时，屏幕样式
只调亮分隔线、控件边界和次要文字，品牌色、布局与打印配色不变。
标题与长段落使用可折行规则，原生选择框沿用深色表面，控件共享 2px 可见焦点、滚动定位留白及触控反馈；
触控规则保留双指缩放，鼠标悬浮样式只在支持 hover 的设备生效。数字展示使用等宽数字特性，代码仍使用等宽字体。
表格基础规则为 100% 容器宽度、固定布局与完整百分比列宽，不采用最小宽度或横向滚动；`TableColumns`
归一化调用方权重，业务组件拥有比例。Public 在本端移动样式中将同一业务表重排为卡片；Admin、Markdown 和打印保留表格，详见[列宽登记](docs/design/table-layouts.md)。

`Pager` 钳制页码并同步跳页输入，纠偏调用可选 `on_page_clamp`（省略时回退 `on_page_change`），用户操作仍调用
`on_page_change`；URL 调用方可分别使用 Replace / Push，避免纠偏污染历史。缺 total 时使用 has_more 语义，
调用方显式提供数量选项才启用数字页码/省略号，固定数量的列表也使用同一套共享控件样式。
数字页码窗口用固定七项数组生成，渲染时不分配临时页码 Vec；首尾、当前页邻居与省略号布局保持一致。
`components/pager/size.rs` 提供显式启用的正整数输入，只接受 ASCII 正整数及调用方注入的可选上限；
溢出失败关闭，本地数量偏好和 URL 策略不由共享层持有。
自定义数量的应用按钮保留可操作状态；非法提交显示已有本地化错误并将焦点返回输入框，不触发数量变更。
输入通过 `aria-describedby` 关联规则说明，仍按 ASCII 正整数和调用方上限校验。
分页视觉由 `style/base.css` 与 `style/mobile.css` 单独拥有，surface 通用表单规则须排除 `.ui-pager` 后代。
导航在 DOM 首位，随后是权威总数、数量选择、自定义数量与跳页；分页整体右对齐，桌面横排或折行后各行仍靠右。
桌面按钮与输入使用 `--size-control-compact`（28px）高度，数量选择框为 24px 高、左右各 4px 内边距，保留原生选项宽度和共享小号字；跳页输入宽 4rem。
手机与触控横屏的分页与 PC 一样单行：‹ 当前页及前后各一页 › · 权威总数 · 每页数量；控件高 32px（容器内覆盖
`--size-control-compact` 与 `--field-min-block-size`），选择框仍为 16px 字号。页码窗口中当前页的相邻项始终是数字页，
CSS 只隐藏其余页码与省略号，首尾页由前后按钮逐页到达；总数空间不足时单行省略。自定义数量与跳页只在 PC 提供，
手机以 CSS 隐藏原表单，不复制或重挂载、不重置未提交草稿。当前页使用金色配深色正文及 `aria-current="page"`；
不裁切分页内容或建立横向滚动，不用 CSS 重排 Tab 顺序。
移动布局只改变 CSS，`has_more`、`max_page`、渲染期钳制、`on_page_clamp` 与账户单页隐藏时保留宿主的语义不变。

`AsyncState` 区分加载、空态和错误的可访问播报；`Dialog` 只拥有外壳与关闭语义（点击背景保持打开，busy 时关闭
入口禁用且 Escape 不冒泡到背景），面板保留 `tabindex="-1"`，没有可用子控件时接管焦点与 Tab 循环；
busy 结束后若焦点仍在面板，Tab 与 Shift+Tab 分别进入首项和末项。
焦点循环、恢复与滚动锁由外壳 hook 统一处理；`TruncatedText` 支持悬停、键盘/触控固定与选择，
不复制输入或弹窗正文到 title。原生事件回调必须恢复 Dioxus Runtime/Scope，延迟任务和监听随卸载清理。
Tab 循环扫描只保留首尾两个可聚焦控件，Rust 侧额外存储为 O(1)；控件筛选、DOM 顺序与空面板回退保持一致。
按钮或链接内的 `TruncatedText` 须设置 `focusable=false`；此时内联浮层也不加入 Tab 顺序，避免嵌套焦点。
截断文本的 compact/medium/wide 档位只是展示上限，同时不得超过所在容器宽度；窄屏仍保留完整内容提示。

手机（48rem 以下）及触控横屏（64rem 以下）采用底部弹窗，编辑控件保持 16px 字号与 44px 最小高度；
分页器、筛选栏等紧凑容器可设置 `--field-min-block-size` 统一降低其后代字段高度，
分页与操作组允许换行。共享安全区变量供两端页面留出系统边缘空间；弹窗按可见视口限制尺寸和滚动范围。
`TruncatedText` 的浮层按可见视口定位，视口移动或缩放时关闭；弹窗中的浮层挂载在所属弹窗内。
内容改变后在 DOM 提交后重测截断并关闭旧正文浮层；尺寸监听读取最新缩写判定，
不要求固定尺寸盒子额外产生 ResizeObserver 通知。尺寸重测时截断值未变不写入，空浮层不重复置空；
表格普通文本提示仅在活动目标及其后代发生内容变化时重读，其他行的更新不会触发全文测量。

## 命令与浏览器状态

`use_confirm_command` 按负载签名冻结 `command_id`：相同输入重试复用、成功清空、失败保留输入与身份；
提交中或累计未知时不能由迟到编辑替换负载，也不能重复提交。任务归创建 hook 的稳定 owner，
结果未知只重放冻结请求，后续认证/限流拒绝不解除未知。宿主位置由各 surface 决定，数据读取分支卸载不能冒充用户取消。
冻结签名比较借用原字符串，只复制已存在的 UUID；不因确认或重试额外分配一份签名。
`use_query_state` 只读写声明的 query 键，popstate 经同一解析器恢复；共享时钟和监听均绑定所属组件生命期。
`BaseStylesheet` 安装 `media/viewport.rs` 的 VisualViewport 监听，将宽高及偏移投影到 CSS 变量，
供软键盘出现或缩放时的弹窗布局使用；卸载恢复原样式并移除监听，缺少 API 时使用 vh/dvh 回退。
尺寸与偏移按数值比较，仅变化项才格式化并写入 CSS；重复 scroll/resize 通知不再分配四份字符串或读取四次 CSS 属性。
原值恢复使用固定四项数组，监听失败与卸载均保持原有清理边界。该优化没有运行时吞吐或移动浏览器测量结论。
断点结构切换只由 CSS `@media` 承担；无人消费的 `matchMedia` hook 已移除，`web-sys` 不再启用 MediaQueryList 相关 feature。
`PageVisibility` 由持有者安装一次 `visibilitychange` 监听，恢复可见时先执行调用方回调再唤醒等待者；每个等待 future
只登记一个 Waker 槽位，取消立即注销。监听回调只捕获必要状态，避免引用环；显式清理或最后一个持有者释放时自动卸载监听。
`sleep_while_visible` 按 1 秒分片计时，隐藏时挂起剩余时长且最后一片后再复核可见性；
非浏览器目标的安装与清理均为无操作，供静态检查编译。

## 传输与错误

`send_get`/`send_request` 使用 gloo-net 0.7、gloo-timers 0.4，默认以 10 秒预算读取响应头与正文；
`send_request_with_timeout` 可为单次请求显式指定预算，仍复用唯一 fetch 与错误路径，无自动重试。
超时或取消通过 AbortController 终止 fetch。本 crate 只声明 Gloo HTTP feature，JSON/WebSocket 由实际调用的 surface 增补；
独立 WASM 检查不能依赖合并 feature。响应保留状态/头，DTO 解码归 surface；observed 入口在响应头到达时通知
Public 校时。CSRF/Basic Auth 由 Admin 构造请求，不能在共享层变成业务耦合。`TransportError` 返回稳定结构化分类，
`classify_base_error` 只识别共用错误，业务码交 surface 映射，不得用展示字符串决定重试。
`transport/browser_lock.rs` 提供通用 `BrowserLock`：只调用安全上下文的原生 Web Locks，按同源同名独占，
等待有界且可取消，回调持有至原生 request Promise 收口；不支持时明确失败，没有存储锁或标签页内伪替代。
调用方先获取锁再构造请求，按作用域保证请求取消在释放锁之前完成。共享传输超时先 abort，
再消费 fetch/正文终止结果；业务锁名称、Cookie 策略和译文仍归各 surface。

## Markdown、可访问性与启动

`components/markdown/` 解析 Markdown/GFM 并生成安全 DOM，原始 HTML 作为文本，链接协议经白名单校验；
反斜杠及控制字符在 trim 前拒绝，站内分类使用同一规则，防止浏览器归一化成外部 authority。
链接标签最多 1 KiB、目标最多 4 KiB，自动链接同样限制目标长度；超限结构按文本处理，不截断整个正文。
解析器缓存下一个闭合符位置，避免大量未闭合链接重复扫描重叠窗口；递归深度仍有上限。
移除首行标题时直接累计正文，省去行向量和完整正文副本；保留 BOM、CRLF、末尾换行以及仅移除首个空行的原行为。
块级表格切分逐项借用正文单元格，不构造临时行向量；分隔线判定直接扫描字节，不建立去空格副本。
表格列权重用固定栈缓冲采样前 32 行，直接遍历行内节点，达到前 96 个可见字符后停止；嵌套节点共享同一字符预算。
既有 Markdown 子集、ASCII 空格规则、链接白名单、字符宽度、列宽与 DOM 结构保持不变。
代码块保存完整起始围栏；只有同类、不短于起始长度且尾部仅空白的围栏才闭合，
单个标记、过短围栏和带正文的标记行继续作为代码显示。
代码块和行内代码声明 `translate="no"`，避免浏览器自动翻译改变代码内容。
`a11y/` 提供初始聚焦、Tab 循环、背景滚动锁与关闭后焦点恢复，跳过禁用、隐藏或 inert 祖先下的控件，
只在真实换页时聚焦主内容。`bootstrap/` 在必要首屏
请求完成（含错误态）后只触发一次骨架退出，15 秒未就绪只提示慢加载，真实 WASM panic 才写失败标记。金额缩写使用
整数精确格式；`Time` 与两端时间辅助函数直接订阅当前 I18n 语言，并调用
`i18n::format_localized_time_for_language` 格式化 Asia/Shanghai 时间，不等待 `<html lang>` 同步 effect，
也不参与业务时钟判断；不再保留读取 DOM 语言的兼容入口。
格式化器仅缓存最近语言的 `Intl.DateTimeFormat`，表格共用同一对象；时间值及输出不缓存，
语言和 Asia/Shanghai 选项不变。

## 语言选择

支持集合固定为 `en`、`zh-CN`、`zh-TW`、`ja`、`ko`，偏好键固定 `hv.lang`。未知值回退到 surface 默认语言，
存储不可用不导致崩溃；Public 默认 en，Admin 默认 zh-CN，中文简繁不互相回退。`LanguagePicker` 使用原生 select，
同步 option、偏好与 `<html lang>`，label 由 surface 翻译后传入；各 surface 独立维护五语言文案。
动态译文的最外层 Fluent 隔离符在已拥有的 `String` 中原地移除，不再分配第二份整段文本；嵌套隔离符及无配对
结束符的处理保持原有规则，译文内容与五语言选择行为不变。

## 主题选择

`theme/` 拥有深色（默认）与浅色两套主题，偏好键固定 `hv.theme`，只接受 `dark`/`light`，缺失、非法或存储不可用时
使用深色。`use_theme_effects` 由两端根组件调用，首次渲染即写入 `<html data-theme>` 与 `<meta name="theme-color">`，
应用显示前已是目标主题；`ThemePicker` 与语言切换器同样使用原生 select，文案由 surface 翻译后传入。
浅色 token 只在 base.css 的 `:root[data-theme="light"]` 重写颜色与阴影，正文、链接、状态文字对三层底色不低于 4.5:1，
控件边界不低于 3:1；高对比度模式和打印另有对应规则，任一主题打印都使用白底纸面。surface 层自有颜色 token 须在本端
样式中提供浅色值。静态启动骨架无法在 WASM 就绪前读取偏好（CSP 禁止内联脚本），始终以深色显示。
启动文档的未分层兜底不得持续覆盖这些分层主题规则：深色 `color-scheme` 只作用于尚无 `data-theme` 的根元素，
`html/body` 背景跟随 `--color-bg`（加载前回退深色），使主题恢复及打印配色能够生效。

## 维护与验证

```bash
cargo check --locked -p auction-web-shared --target wasm32-unknown-unknown
```

仅静态编译；默认门禁与行为测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
路由、交互或字段变化须同步所属设计与契约，编译通过不等于浏览器验收。
