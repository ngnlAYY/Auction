# 实现与验证状态

[文档导航](README.md) · [架构](architecture.md) · [默认验证范围](development.md#默认检查)

本页统一说明能力与证据的区别。本次于 2026-09-20 按工作区源码、Cargo 清单、迁移、配置模板和构建脚本
核对文档，源码基线为 `7830d38`，已包含首次设置、PostgreSQL 五字段配置和可选 `auction-server`。
工作区原有的 `ci/release.sh` 未提交修改予以保留；本次文档修订不对应新发布版本，也不构成运行验收。

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
| 迁移 | PostgreSQL 37、SQLite 12 个追加迁移文件 | 任一目标数据库已经运行到该版本 |
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

2026-09-20 使用仓库 Rust/Cargo `1.98.0` 和 just `1.58.0` 执行以下命令，退出码均为 0：

- `just fmt`、`git diff --check`；
- `just docs-check`：覆盖 49 份文档，本地链接、锚点、导航、包 README 与笔记结构通过；
- `just repository-policy-check`、`just layout-check`、`just config-example-check`、`just i18n-check`。

本轮只修改 Markdown，保留原有脚本改动；未运行 Cargo 编译/Clippy/WASM 全套门禁或发布构建，
也未启动服务、连接数据库、执行迁移或行为测试。上述结果不能替代 `just verify`、制品或生产验收。

## 源码核对发现的实现缺口

以下为静态源码核对发现的问题，本次仅登记，不修改实现，也不降低既有契约：

| 问题 | 源码依据与影响 | 待闭合边界 |
| --- | --- | --- |
| 设置门禁拒绝响应缺少安全头 | [access.rs](../crates/auction-setup/src/server/access.rs) 的 `setup_origin` 403 提前返回，跳过后续 `Cache-Control: no-store` 与 `Referrer-Policy: no-referrer` 设置 | 保留[全部设置响应禁止缓存](contracts/routes.md#首次部署初始化)的要求；不能把成功路径已加头视为拒绝路径也已覆盖 |
| 首次初始化未复用 PostgreSQL 历史迁移兼容层 | [initialization/mod.rs](../crates/auction-storage/src/database/initialization/mod.rs) 直接运行原始 `sqlx::migrate!`；[migrations/mod.rs](../crates/auction-storage/src/database/migrations/mod.rs) 为 `000005`/`000015` 提供的保留字别名执行修正未在此入口使用 | 存在首次空库迁移的解析兼容风险；应复用受限执行视图并保持历史 checksum，不修改已发布 SQL。未经修复及另行授权验证，不能宣称向导到三角色启动链路已验收 |

初始化失败应保留配置和数据库现场，停止进程后按[合并部署维护](../apps/server/README.md#生命周期与维护)
显式处理，不通过删除配置、修改历史迁移或自动重试来绕过问题。

## 仍需运行证据的事项

- PostgreSQL 并发竞价、资格变化、序列化冲突、故障重启、损坏事实与恢复边界。
- SQLite 单主机串行写入、文件锁、维护窗口和备份恢复；不能替代 PostgreSQL 验证。
- 真实 MoogleMail/E-Hentai 的认证、未知写入结果、Sent/Balance 证明与人工恢复。
- Public/Admin 完整页面、五语言、认证组合、严格 decoder、实时帧和断线恢复。
- 非空生产数据、目标主机权限与服务编排、制品切换、备份恢复和回执保管。
- 首次设置的本机/HTTPS 代理门禁、未知保存结果与空库初始化；合并部署的 readiness、信号、父进程强杀、迁移失败和升级。

后续获得授权开展行为验证时，应登记制品/源码身份、后端与版本、脱敏环境、时间、输入、预期与实际结果、
证据保存位置和限制。敏感证据保存在受控目录，项目文档只记录安全引用和结论。
静态验证、页面样式完成、空表或路由存在均不足以关闭上述事项。

## 未登记为现有能力

Public 当前实时通道是 WebSocket 加 HTTP Catch-up；未注册 SSE endpoint。
外部 realtime 广播 sink、retention 数据删除/归档 mutation、自动 OpenAPI 生成也不属于当前交付能力。
要新增这些能力，需要独立契约、owner、授权边界与决策记录。
