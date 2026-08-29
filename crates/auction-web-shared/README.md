# 浏览器端共享基础能力

本 crate 只放 Public 与 Admin 都需要、且没有业务偏向的内容：

- `style/base.css`：设计变量（颜色、字号、间距、圆角）、基础排版、`auction-shell` 外壳
  布局和语言切换器样式。两个 surface 的样式都建立在这些变量之上。
- `src/i18n/mod.rs`：受支持语言集合、`hv.lang` 偏好持久化、`<html lang>` 同步和
  `LanguagePicker` 组件。
- `src/components/mod.rs`：无领域语义的共享基础组件。
- `src/components/markdown/{mod,model,blocks,inline,render}.rs`：共享 Markdown/GFM 展示器的
  模型、块级解析、行内解析和安全 DOM 渲染；原始 HTML 只作为转义文本展示，链接目标经过
  协议白名单校验。`MarkdownDocument` 提供共享主题容器，`MarkdownBlocks` 只输出安全块节点，
  供必须由 surface 保留旧容器和排版的页面使用。
- `src/a11y/mod.rs`：共享 `aria-modal` 弹窗的动态初始聚焦、Tab 焦点循环、背景滚动锁和
  关闭后触发器焦点恢复；不包含 Public/Admin 业务语义。
- `src/format/mod.rs`：Public/Admin 共用且不参与业务判断的金额展示规则。

## 语言选择

受支持语言为 `en`、`zh-CN`、`zh-TW`、`ja`、`ko`，顺序与旧系统语言切换器一致。中文按脚本
区分简繁，两者不互相回退。

`localStorage` 键固定为 `hv.lang`，与旧 `src/shared/i18n/createI18n.ts` 一致，**不得重命名**
——用户在新旧系统间切换时语言偏好靠它保持。存储值必须在受支持集合内，否则回退到该 surface
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
