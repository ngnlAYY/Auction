# 浏览器端共享基础能力

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

Public 与 Admin 共用、无业务偏向的浏览器基础能力。

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
| [src/components](src/components) | 表格、分页、弹窗、状态、时间与安全 Markdown |
| [src/commands](src/commands)、[src/query](src/query) | 确认命令与 URL 状态 |
| [src/transport](src/transport)、[src/errors](src/errors) | 同源传输与通用错误分类 |
| [src/i18n](src/i18n)、[src/format](src/format) | 语言偏好、时间与金额展示 |
| [src/a11y](src/a11y)、[src/bootstrap](src/bootstrap) | 焦点、背景滚动与首屏门禁 |
| [src/clock](src/clock)、[src/media](src/media) | 共享时钟与媒体查询 |
| [style/base.css](style/base.css) | 设计变量与共享组件样式 |

## 职责与实现边界

只共享稳定的基础组件和浏览器机制。业务负载、错误文案、Public 竞价与 Admin 管理状态留在各 surface；
本 crate 不依赖存储、配置或凭据访问路径，也不拥有两端的 Fluent 译文。

## 设计变量与组件

`base.css` 提供颜色、字号、间距、圆角、层级等变量及 `.ui-*` 样式；两端业务 BEM 类留在各自 assets。
表格一律 100% 容器宽度、固定布局与完整百分比列宽，不采用最小宽度、横向滚动或窄屏卡片化。
`TableColumns` 归一化调用方权重；业务组件拥有比例，Markdown 按表头和典型内容估算，见[列宽登记](../../docs/table-layouts.md)。

`Pager` 钳制页码并同步跳页输入，缺 total 时使用 has_more 语义；`AsyncState` 区分加载、空态和错误的可访问播报。
`Dialog` 只拥有外壳与关闭语义，busy 时关闭入口禁用；焦点与滚动锁由外壳 hook 统一处理。
普通文本提供完整提示，`TruncatedText` 支持悬停、键盘/触控固定及选择，不复制输入或弹窗正文到 title。
原生事件回调必须恢复 Dioxus Runtime/Scope，延迟任务和监听随组件卸载清理。

## 命令与浏览器状态

`use_confirm_command` 按负载签名冻结 `command_id`：相同输入重试复用，成功清空，失败保留输入与身份。
结果不确定时仅重放冻结请求，后续认证/限流拒绝不能清除原不确定状态；宿主卸载取消任务，子表单替换不取消宿主请求。
业务分类与文案由 surface 注入。

`use_query_state` 只读写声明的 query 键，popstate 经同一解析器恢复，与 Router 共存。
`use_second_ticker` 在外壳提供惰性共享秒级时钟，订阅者不各自创建定时器；`use_media_query` 管理自己的 change 监听。
所有监听、闭包与任务均绑定所属组件生命期。

## 传输与错误

`send_get`/`send_request` 用统一 10 秒预算读取响应头与正文；超时或取消通过 AbortController 终止 fetch。
响应保留状态/头，DTO 解码归 surface；observed 入口在响应头到达时通知 Public 校时，避免正文传输延迟污染样本。
CSRF/Basic Auth 由 Admin 构造请求，不能在共享层变成业务耦合。

`TransportError` 返回稳定结构化分类；`classify_base_error` 仅识别共用错误，业务码交 surface 的 Fluent 映射。
不得用展示字符串比较决定重试或业务行为。

## Markdown、可访问性与启动

`components/markdown/` 解析 Markdown/GFM 并生成安全 DOM，原始 HTML 作为文本，链接协议经白名单校验。
`a11y/` 提供初始聚焦、Tab 循环、背景滚动锁与关闭后焦点恢复；只在真实换页时聚焦主内容，首屏/同页刷新不抢焦点。

`bootstrap/` 在必要首屏请求完成（含错误态）后只触发一次骨架退出；15 秒未就绪只提示慢加载，
真实 WASM panic 才写失败标记，后续就绪不能覆盖。金额缩写使用整数精确格式，完整值用于提示；
时间显示按当前语言与既有 Asia/Shanghai 时区格式化，不参与业务时钟判断。

## 语言选择

支持集合固定为 `en`、`zh-CN`、`zh-TW`、`ja`、`ko`，偏好键固定 `hv.lang`。
未知值回退到 surface 默认语言，存储不可用不导致崩溃；Public 默认 en，Admin 默认 zh-CN。
中文简繁不互相回退。

`LanguagePicker` 使用原生 select，同步 option、偏好与 `<html lang>`；label 由 surface 翻译后传入。
各 surface 独立维护全部五语言文案，共享样式通过本 crate 的 asset 声明随构建收集。

## 维护与验证

```bash
cargo check --locked -p auction-web-shared --target wasm32-unknown-unknown
```

仅静态编译；默认门禁与行为测试范围见[开发指南](../../docs/development.md)，运行证据见[验证状态](../../docs/status.md)。
路由、交互或字段变化须同步所属设计与契约，编译通过不等于浏览器验收。
