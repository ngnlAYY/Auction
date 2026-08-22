# 管理端浏览器界面

Dioxus 客户端渲染的管理端 SPA。路由集合按旧版 Auction-BF 的
`manifests/admin.routes.json` 冻结，共 21 条。

- `src/main.rs`：薄启动入口。
- `src/api/`：管理端拍卖、lot、delivery/realtime/mail 安全投影、CSRF 会话读取和人工状态命令请求。
- `src/app/`：管理端路由表、外壳与按职责拆分的页面组件。
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
公开端也不得依赖本 crate。拍卖列表已接入 PostgreSQL 只读安全投影、world/status 筛选和分页，
lot 页面也已接入同一安全投影；创建拍卖命令和管理端创建表单已接入 PostgreSQL 完整档。
draft pending lot 还提供起拍价修改与撤回按钮；动作携带新 `command_id`、快照版本和 CSRF，
成功后重新读取权威 lot 页面，不在浏览器端复制 lot 状态机。
delivery 页面已接入 PostgreSQL 安全投影和 release/retry/
reconcile-retry 动作；命令携带 `command_id`、CSRF 和 fencing version，服务端仍负责最终校验。
Status 页面已接入 realtime lag、未解决 failure 安全投影和显式 replay 表单；replay 的公开
payload 由操作员输入，服务端负责 schema、幂等、审计和事件关系校验。
规则页面已接入 Admin 规则读取、候选校验和带 revision fence 的发布表单；浏览器只发送
版本化 DTO，不持有数据库或凭据状态。
邮件页面已接入 A16-A19 的轮询状态、入站安全摘要和暂停/恢复动作；动作携带 CSRF、当前
状态版本与新命令 ID。页面不接收原始邮件正文、Cookie、轮询游标或外部凭据；A20 触发按钮
已由 HTTP/Worker 链路接入，返回的是排队状态，不代表业务邮件处理完成。
改名页面已接入 A29 的改名历史筛选和安全表格；A30 回滚按钮待应用层版本 fence、命令账本和
名称冲突规则完成后再提供。
管理命令必须走版本化 REST 契约，不在浏览器端重建状态机；凭据与密钥材料不进入浏览器。
