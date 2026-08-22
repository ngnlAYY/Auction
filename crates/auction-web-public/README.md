# 公开端浏览器界面

Dioxus 客户端渲染的公开端 SPA。路由集合按旧版 Auction-BF 的
`manifests/public.routes.json` 冻结，共 14 条，修改前必须先确认外部 URL 契约。

- `src/main.rs`：薄启动入口。
- `src/api/`：按 session、账户、规则、黑名单、只读查询、实时事件和命令职责拆分的同源 Public API DTO 解码与请求，
  不接触存储模型。
- `src/app/`：按 session、账户、规则、黑名单、只读页面、实时事件和命令组件拆分的公开端路由表、外壳与页面组件。
- `assets/public.css`：公开端 surface 样式，沿用旧版公共拍卖页的 BEM 类名与视觉契约。
- `i18n/*.ftl`：5 种语言的 Fluent 译文（`en`、`zh-CN`、`zh-TW`、`ja`、`ko`），默认英文。

## 国际化

所有用户可见文案都经 `t!` 从 `i18n/*.ftl` 取用，组件里不得硬编码字符串。译文以
`include_str!` 编译期内嵌，首屏不额外请求语言资源。

Fluent 消息 ID 不允许包含点号，旧系统的 `nav.home` 一类键改为 `nav-home`。

`t!` 在键缺失时 **panic**，因此新增文案必须同步更新全部 5 个 `*.ftl`；缺一种语言就会让该
语言下的页面崩溃。语言集合、偏好持久化和 `<html lang>` 同步由 `auction-web-shared` 提供，
本 crate 只拥有自己的译文。
  设计变量来自 `auction-web-shared`。

构建：

```bash
just web-build-public   # dx build -p auction-web-public --platform web
just web-serve-public   # 本地开发监听
```

产物输出到 `target/dx/build/auction-web-public/web/public/`，由 `auction-http` 内嵌进
公开端二进制。未构建产物时后端仍可编译，静态路由返回 503。

当前首页看板、lot 详情、规则文档、公开黑名单、竞价日志、价格历史、Public session、账户竞价/代理读取、live lot
普通出价和持久化实时事件游标页已经通过同源 HTTP 读取或提交 Public API；已认证会话会让实时页自动订阅只读
WebSocket，只消费安全事件信封，并在服务端要求权威快照时停止推进。加载、错误、空数据和命令确认状态均使用五语言
资源。代理完整命令和其余管理交互仍需按契约逐片接入。
本 crate 只允许依赖稳定 HTTP DTO 与 `auction-web-shared`，不得导入存储、配置、凭据或管理端
组件。
