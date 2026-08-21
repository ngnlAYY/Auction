# 管理端浏览器界面

Dioxus 客户端渲染的管理端 SPA。路由集合按旧版 Auction-BF 的
`manifests/admin.routes.json` 冻结，共 21 条。

- `src/main.rs`：路由表、管理外壳与页面组件。
- `assets/admin.css`：管理端 surface 样式。设计变量来自 `auction-web-shared`。
- `i18n/*.ftl`：5 种语言的 Fluent 译文（`en`、`zh-CN`、`zh-TW`、`ja`、`ko`），默认简体中文。

## 国际化

所有用户可见文案都经 `t!` 从 `i18n/*.ftl` 取用，组件里不得硬编码字符串。译文以
`include_str!` 编译期内嵌，首屏不额外请求语言资源。

Fluent 消息 ID 不允许包含点号，旧系统的 `nav.home` 一类键改为 `nav-home`。

`t!` 在键缺失时 **panic**，因此新增文案必须同步更新全部 5 个 `*.ftl`；缺一种语言就会让该
语言下的页面崩溃。语言集合、偏好持久化和 `<html lang>` 同步由 `auction-web-shared` 提供，
本 crate 只拥有自己的译文。

构建：

```bash
just web-build-admin   # dx build -p auction-web-admin --platform web
just web-serve-admin   # 本地开发监听
```

产物输出到 `target/dx/build/auction-web-admin/web/public/`，由 `auction-http` 内嵌进管理端
二进制。公开端二进制不包含这些资源。

管理端与公开端是不同 surface：本 crate 独立拥有自己的组件树和路由树，不复用公开端组件，
公开端也不得依赖本 crate。管理命令必须走版本化 REST 契约，不在浏览器端重建状态机；凭据与
密钥材料不进入浏览器。
