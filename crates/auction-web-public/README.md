# 公开端浏览器界面

[文档导航](../../docs/README.md) · [Public 设计](./DESIGN.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

Dioxus 纯客户端渲染的公开端 SPA，拥有公开读取、竞价交互、会话与实时恢复。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [页面与源码布局](#页面与源码布局)
- [国际化](#国际化)
- [构建与资源边界](#构建与资源边界)
- [权威数据与命令交互](#权威数据与命令交互)
- [监听器所有权与实时恢复](#监听器所有权与实时恢复)
- [依赖与首屏边界](#依赖与首屏边界)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/app/surface](src/app/surface) | 路由、外壳、看板、目录与查询状态 |
| [src/app/auction](src/app/auction) | 拍卖读取、普通竞价与代理交互 |
| [src/app/content](src/app/content)、[src/app/identity](src/app/identity) | 规则、论坛、账户及认证页面 |
| [src/api](src/api) | 同源请求、时钟采样与严格 DTO 解码 |
| [assets](assets)、[i18n](i18n) | Public 样式与五语言译文 |

## 职责与实现边界

只消费稳定 HTTP/实时契约和共享浏览器能力，不依赖 Admin UI、存储模型、配置或服务端密钥。
15 条浏览器路径由 Route、[后端入口清单](../auction-http/src/assets/routes.rs)和[路由契约](../../docs/contracts/routes.md)共同维护。
页面结构、视觉与交互详见 [Public 设计](./DESIGN.md)，本页只说明实现边界。

## 页面与源码布局

`main.rs` 为薄启动入口，`app/` 与 `api/` 按 auction、content、identity 分组。
`surface/board/refresh.rs` 拥有资源失效与合并读取；`auction/commands/` 按普通出价、代理创建/降额、确认及未知结果重试拆分。

论坛页先读取独立同步开关，明确关闭不挂载帖子轮询，未知状态不能当关闭；启用后只展示本地镜像和真实处理楼层。
首页不读取论坛状态，`/status` 只作静态说明。目录、历史和账户的筛选/分页通过 URL 恢复，响应须与当前选择的身份对齐。

## 国际化

业务文案统一通过 `t!` 读取本 crate 的 `i18n/*.ftl`，默认 en，编译期内嵌。
消息 ID 不含点号，新键同步五种语言，防止缺键 panic；`hv.lang` 和 `<html lang>` 由共享层维护。
规则请求把 locale 作为资源依赖，迟到响应不能覆盖当前语言；正文用安全 Markdown 展示。

## 构建与资源边界

```bash
just web-build-public
```

该任务使用 `dx bundle`，复制到 `target/dx/build/auction-web-public/web/public/`，
供 Public 的显式 `web-embed` 制品内嵌。无 SSR/hydration、独立 JavaScript 业务代码或第二套前端构建链。
后端保留 `allow_missing`：无产物可编译，已登记页面返回 503；未知路径保持 404。
本地开发和发布流程见[开发指南](../../docs/development.md#浏览器与发布构建)。

## 权威数据与命令交互

目录、lot、规则、账户、价格历史等均读取同源 API，不使用会被误认成权威数据的示例行。
服务端 capability 决定动作；Lite 缺失能力明确展示，空装备链接不生成伪造 URL。
完整游戏链接只消费受限 DTO，不能放入持久化缓存、日志或事件。

普通出价使用整数 K 输入，代理支持约定的精确缩写；新命令最高 1B，服务端仍是金额、资格和状态的最终权威。
提交前确认，失败保留冻结输入与命令 ID；结果未知只能重试原请求，不能用新价格门禁阻断幂等重放。
命令宿主按 lot/用户 UUID 隔离，切换目标或身份销毁旧状态，成功结果不被后台快照刷新覆盖。

当前世界选择只展示 enabled world，历史筛选仍可访问已关闭世界的 settled 事实。
倒计时通过 HTTP Date 估算服务端时钟，仅用于展示；终态、缺失时间与依赖错误按契约明确表达。

## 监听器所有权与实时恢复

已认证外壳订阅 discovery，拍卖页订阅相应单拍卖流；`/status` 不再建连接。
持久化游标、连续 sequence/version 与权威快照决定恢复；`refetch_required` 保留高水位和现有 socket，
不能直接用事件 payload 推导价格、领先者或代理权限。协议详见[实时契约](../../docs/contracts/events.md)。

普通竞价事件只失效相关已挂载资源，从首次通知起合并 100 毫秒；进行中的读取先完成，再补读未消费通知。
生命周期、discovery 或缺口立即触发权威刷新。浏览器不因没有业务消息推断失联，心跳由服务端负责。
visibilitychange、popstate、socket 和回调随所属外壳卸载清理，游标不能因重建 owner 退回零。

所有表格维持百分比列宽和行结构，完整文本提示及弹窗焦点/滚动锁复用共享层；
列宽与细节分别由[表格登记](../../docs/table-layouts.md)和设计文档维护。

## 依赖与首屏边界

HTTP/stream wire DTO 直接复用 `auction-contracts`；本地只持有查询、展示和交互状态。
Gloo 在本 surface 显式启用 JSON/WebSocket，HTTP 基础能力继承工作区；不能依赖 Admin 或合并构建补齐 feature。
会话读取失败保留错误与重试入口，不能伪装匿名；登录 return 只接受单斜杠开头的站内路径。
首屏 15 秒未就绪只提示慢加载，真实 panic 才显示失败，后续就绪回调不能覆盖该状态。

## 维护与验证

```bash
cargo check --locked -p auction-web-public --target wasm32-unknown-unknown
```

仅静态编译；默认门禁与行为测试范围见[开发指南](../../docs/development.md)，运行证据见[验证状态](../../docs/status.md)。
路由、交互或字段变化须同步所属设计与契约，编译通过不等于浏览器验收。
