# 实现与验证状态

[文档导航](README.md) · [架构](architecture.md) · [默认验证范围](development.md#默认检查)

本页统一说明能力与证据的区别。2026-10-03 在 `5cca279` 基线上完成最近全仓静态审计的 7 项源码修复，
范围与取舍见[本轮修复记录](../.agents/notes/implemented/bug-fix/2026-10-03-session-setup-audit-remediation.md)。
本轮不新增或改写迁移，不对应已发布制品，也不构成运行验收。此前 2026-09-22 的 29 项报告问题、
论坛 DTO 附项及三份追加迁移仍由[历史修复记录](../.agents/notes/implemented/bug-fix/2026-09-22-source-audit-remediation.md)拥有。

核对范围覆盖根目录、docs、apps、crates、ops、migrations、ci 的正式文档，以及既有决策笔记的引用与状态。
仍准确的内容按原样保留；历史笔记记录当时理由与验证声明，不将其改写成当前运行证据。

## 本页导航

- [能力范围](#能力范围)
- [证据口径](#证据口径)
- [本轮静态检查](#本轮静态检查)
- [源码核对发现的实现缺口](#源码核对发现的实现缺口)
- [仍需运行证据的事项](#仍需运行证据的事项)
- [未登记为现有能力](#未登记为现有能力)

## 能力范围

| 范围 | 源码中的实现边界 | 不能据此推导 |
| --- | --- | --- |
| HTTP 与页面 | Public 21、Admin 43 条 feature API；15/21 条浏览器入口 | 每条流程已经通过浏览器或部署验收 |
| PostgreSQL | 完整竞价、结算/恢复、论坛、邮件、凭据、运维 owner | 真实外部服务与生产数据正确性已获证明 |
| SQLite Lite | 共有竞价、会话、部分管理、持久调度和实时读取 | PostgreSQL 系统能力、行锁或并发语义对等 |
| 迁移 | PostgreSQL 39、SQLite 13 个追加迁移文件 | 任一目标数据库已经运行到该版本 |
| 发布 | API-only/embedded 隔离、payload 清单、确定性归档门禁 | 本次文档修改已经构建或部署了新制品 |
| 无配置首次启动 | 默认 loopback 设置页、五字段数据库输入；合并入口支持显式 HTTPS 代理及确认后空库初始化/启动 | 本轮已在真实主机完成设置、迁移、登录或浏览器验收 |
| 可选合并制品 | `auction-server` 自管三角色子进程，`auctionctl` 独立；默认分体包不变 | 已完成信号、父进程强杀、代理、迁移失败和升级运行验收 |
| 灾备 | 独立 backup/verify/rebuild/restore 与 canonical v28 接管流程 | 备份可用于未经验证的生产恢复或任意历史 schema |

注册数由[路由](contracts/routes.md)、[静态入口](../crates/auction-http/src/assets/routes.rs)及
[迁移目录](../migrations/README.md)核对。能力缺失应按契约返回明确错误，不能用空数据或示例数据替代。

## 证据口径

| 表述 | 必须有的依据 |
| --- | --- |
| 已实现 | 可定位的 owner、调用路径、契约与后端能力 |
| 静态检查通过 | 本次执行命令、退出结果、工具链及检查范围 |
| 隔离环境验证 | 明确环境、输入、目标版本、执行结果及保留的原始证据 |
| 生产验收完成 | 指定制品、真实配置/服务、生产数据边界、部署切换及恢复证据 |

既有文档使用过“局部 R1”描述 HTTP、竞价、结算、浏览器和 canonical PostgreSQL v28 接管/灾备的
部分隔离验证。本仓库正式文档没有提供足够完整的原始证据索引，本次也没有重跑这些流程。
因此这里保留为历史验证声明，不把它升级为本轮确认的运行结论。“R1”不能替代具体执行记录。

## 本轮静态检查

### 2026-10-03

使用仓库 Rust/Cargo `1.98.0`，以下命令退出码均为 0：

- `just verify`：格式、52 份文档、仓库/布局/配置/i18n、工作区和独立 PG/SQLite/Redis feature、
  all-features Clippy（`-D warnings`）与双 WASM 编译。
- `cargo check --locked --workspace --all-targets --all-features`。
- `cargo +1.94.1 check --locked --workspace --all-targets --all-features`。
- `just dependency-audit`：cargo-audit `0.22.2` 使用 `--deny yanked --deny unsound`，检查 543 个
  crate 条目，未命中漏洞、撤回或 unsound，保留一项 `fxhash` 停维提示；公告库提交为
  `f8dee89e1b2f2f1eaf548312df7655fe5202a302`。额外发现的撤回版本 chacha20 0.10.1 已升至 0.10.2。
- `git diff --check`；历史 SQL 文件无修改。

辅助 SQL 静态扫描覆盖 39 份 PostgreSQL 迁移与 737 个 Rust SQL 候选；两份受限兼容执行视图
解析通过，686 条可识别直接绑定链未见参数数量不匹配。历史原文保留保留字错误，由兼容视图执行；
HTTP/Redis 文字和动态拼接片段的误报已人工区分。8 份 PL/pgSQL 的解析器 JSON 输出存在工具限制，
不据此声称全部函数体、schema、事务或运行行为已验证。
LSP 未完成：固定工具链缺少 `rust-analyzer`，返回
`Unknown binary 'rust-analyzer' in official toolchain '1.98.0-x86_64-unknown-linux-gnu'.`；
可安装对应组件或修正 `pi-lsp.json` 的 server command，本轮不改全局工具链。
编译及 Clippy 结果独立有效，不冒充 LSP 通过。

未新增或运行行为测试、发布构建、浏览器流程、产品服务、数据库或迁移；上述结果不构成生产验收。

### 2026-09-22 历史记录

2026-09-22 使用仓库 Rust/Cargo `1.98.0` 和 just `1.58.0` 执行完整 `just verify` 及
`git diff --check`，退出码均为 0。完整门禁包括：

- `just fmt`、`git diff --check`；
- `just docs-check`：覆盖 51 份文档，本地链接、锚点、导航、包 README 与笔记结构通过；
- `just repository-policy-check`、`just layout-check`、`just config-example-check`、`just i18n-check`；
- 工作区 `--locked --all-targets` 编译、独立 PostgreSQL/SQLite storage 与 Redis cache 特性编译；
- 工作区 `--locked --all-targets --all-features` Clippy，`-D warnings`；
- Public/Admin 的 `wasm32-unknown-unknown` 编译。

本轮同时完成各修复范围的独立源码复核；完整门禁中发现的借用、错误映射及特性编译问题已修正后重跑。
未运行行为测试、发布构建、浏览器流程、服务、数据库或迁移。上述结果不能替代制品或生产验收。
历史流拍 Return、入站资产冲突、已错误归档的 Credits 与缺少排期来源的旧数据保留证据边界，
不会通过静默重写或自动重发宣称修复远端资产；见[邮件核对](operations/mail-diagnostics.md#历史资产处置核对)。

## 源码核对发现的实现缺口

最近全仓审计的 7 项问题均已在本轮源码修复；其中两项初始化缺口沿用此前登记，
不重算为首次发现。下表区分源码闭合与仍未执行的运行验证，不宣称全项目不存在其他缺陷：

| 问题 | 源码修复 | 待闭合边界 |
| --- | --- | --- |
| F1 · Admin 凭据轮换后旧会话仍有效 | [session 服务](../crates/auction-application/src/services/sessions/mod.rs) 异步启动先永久撤销旧会话，HMAC 绑定 actor/secret，读取复核 actor；两后端共用既有表 | 重启需重新登录；凭据切换、恢复旧配置、多实例维护仍缺运行证据 |
| F2 · 首次初始化绕过迁移兼容层 | [initialization](../crates/auction-storage/src/database/initialization/mod.rs) 在既有迁移锁内复用受限执行视图，原 SQL/checksum 不变 | 仍须独立验证空库迁移、失败现场与三角色启动 |
| F3/F6 · rustls/h2 漏洞公告 | [锁图](../Cargo.lock) 为 rustls 0.23.45、h2 0.4.19；rustls 下限同步，CI 新增依赖审计 | 版本范围修复不等于生产可达性或未知漏洞证明 |
| F4 · 设置慢请求与无界退出 | [access](../crates/auction-setup/src/server/access.rs) 限正文/异步处理 10 秒，令牌先于正文；[lifecycle](../crates/auction-setup/src/server/lifecycle.rs) 复用 5 秒请求头与 10 秒排空；终止不继续初始化 | 未执行慢请求、断连、信号或阻塞文件系统运行验证 |
| F5/F7 · 设置防嵌入及拒绝响应安全头 | 正式表面与设置复用 [headers](../crates/auction-http/src/http/security/headers.rs)，包括拒绝、框架错误及超时应用响应 | 未执行浏览器/代理验证 |

`fxhash 0.2.1` 的 RUSTSEC-2025-0057 是停维提示而非已确认漏洞，暂保留解析依赖，
2026-11-03 前复查上游兼容迁移；CI 不隐藏该提示，详情见[修复决定](../.agents/notes/implemented/bug-fix/2026-10-03-session-setup-audit-remediation.md)。

初始化失败应保留配置和数据库现场，停止进程后按[合并部署维护](../apps/server/README.md#生命周期与维护)
显式处理，不通过删除配置、修改历史迁移或自动重试来绕过问题。

## 仍需运行证据的事项

- PostgreSQL 并发竞价、资格变化、序列化冲突、故障重启、损坏事实与恢复边界。
- SQLite 单主机串行写入、文件锁、维护窗口和备份恢复；不能替代 PostgreSQL 验证。
- 真实 MoogleMail/E-Hentai 的认证、未知写入结果、Sent/Balance 证明与人工恢复。
- Public/Admin 完整页面、五语言、认证组合、Admin 重启/凭据轮换及旧配置恢复、严格 decoder、实时帧和断线恢复。
- 非空生产数据、目标主机权限与服务编排、制品切换、备份恢复和回执保管。
- 首次设置的本机/HTTPS 代理门禁、未知保存结果与空库初始化；合并部署的 readiness、信号、父进程强杀、迁移失败和升级。

后续获得授权开展行为验证时，应登记制品/源码身份、后端与版本、脱敏环境、时间、输入、预期与实际结果、
证据保存位置和限制。敏感证据保存在受控目录，项目文档只记录安全引用和结论。
静态验证、页面样式完成、空表或路由存在均不足以关闭上述事项。

## 未登记为现有能力

Public 当前实时通道是 WebSocket 加 HTTP Catch-up；未注册 SSE endpoint。
外部 realtime 广播 sink、retention 数据删除/归档 mutation、自动 OpenAPI 生成也不属于当前交付能力。
要新增这些能力，需要独立契约、owner、授权边界与决策记录。
