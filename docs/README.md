# 项目文档导航

[项目首页](../README.md)

新读者先看架构和开发指南；实现修改从模块 README 进入契约；部署从配置、发布与恢复手册进入。
本页列出全部正式文档，不包含生成产物、本地工具状态或私有运行证据。新增、移动或删除文档时同步导航。

## 项目与开发

| 文档 | 仓库位置 |
| --- | --- |
| [Auction Rust](../README.md) | `README.md` |
| [Auction Rust 系统设计](../DESIGN.md) | `DESIGN.md` |
| [AGENTS.md — Auction Rust](../AGENTS.md) | `AGENTS.md` |
| [系统架构与模块所有权](architecture.md) | `docs/architecture.md` |
| [技术栈与取舍](tech-stack.md) | `docs/tech-stack.md` |
| [开发与静态验证](development.md) | `docs/development.md` |
| [实现与验证状态](status.md) | `docs/status.md` |
| [文档维护与决策记录](documentation.md) | `docs/documentation.md` |
| [静态验证、文档门禁与发布脚本](../ci/README.md) | `ci/README.md` |

## 契约与界面

| 文档 | 仓库位置 |
| --- | --- |
| [契约导航与变更路径](contracts/README.md) | `docs/contracts/README.md` |
| [HTTP 路由与响应契约](contracts/routes.md) | `docs/contracts/routes.md` |
| [实时事件与客户端恢复契约](contracts/events.md) | `docs/contracts/events.md` |
| [数据库对象与所有权契约](contracts/schema.md) | `docs/contracts/schema.md` |
| [领域不变量与写入语义](contracts/invariants.md) | `docs/contracts/invariants.md` |
| [表格布局与列宽契约](table-layouts.md) | `docs/table-layouts.md` |

## 应用进程

| 文档 | 仓库位置 |
| --- | --- |
| [admin-api · auction-admin-api](../apps/admin-api/README.md) | `apps/admin-api/README.md` |
| [auctionctl](../apps/auctionctl/README.md) | `apps/auctionctl/README.md` |
| [public-api · auction-public-api](../apps/public-api/README.md) | `apps/public-api/README.md` |
| [worker · auction-worker](../apps/worker/README.md) | `apps/worker/README.md` |
| [server · auction-server](../apps/server/README.md) | `apps/server/README.md` |

## 共享能力

| 文档 | 仓库位置 |
| --- | --- |
| [auction-application](../crates/auction-application/README.md) | `crates/auction-application/README.md` |
| [auction-cache](../crates/auction-cache/README.md) | `crates/auction-cache/README.md` |
| [auction-config](../crates/auction-config/README.md) | `crates/auction-config/README.md` |
| [auction-contracts](../crates/auction-contracts/README.md) | `crates/auction-contracts/README.md` |
| [auction-domain](../crates/auction-domain/README.md) | `crates/auction-domain/README.md` |
| [auction-http](../crates/auction-http/README.md) | `crates/auction-http/README.md` |
| [auction-integrations](../crates/auction-integrations/README.md) | `crates/auction-integrations/README.md` |
| [auction-jobs](../crates/auction-jobs/README.md) | `crates/auction-jobs/README.md` |
| [auction-observability](../crates/auction-observability/README.md) | `crates/auction-observability/README.md` |
| [auction-process](../crates/auction-process/README.md) | `crates/auction-process/README.md` |
| [auction-setup](../crates/auction-setup/README.md) | `crates/auction-setup/README.md` |
| [auction-storage](../crates/auction-storage/README.md) | `crates/auction-storage/README.md` |
| [auction-web-admin · 管理端浏览器界面](../crates/auction-web-admin/README.md) | `crates/auction-web-admin/README.md` |
| [auction-web-public · 公开端浏览器界面](../crates/auction-web-public/README.md) | `crates/auction-web-public/README.md` |
| [auction-web-shared · 浏览器端共享基础能力](../crates/auction-web-shared/README.md) | `crates/auction-web-shared/README.md` |

## 配置、部署与运维

| 文档 | 仓库位置 |
| --- | --- |
| [运行配置与敏感文件](../ops/config/README.md) | `ops/config/README.md` |
| [部署与发布手册](../ops/deploy/README.md) | `ops/deploy/README.md` |
| [systemd 服务与顺序控制](../ops/deploy/systemd/README.md) | `ops/deploy/systemd/README.md` |
| [PostgreSQL 备份与恢复手册](../ops/deploy/RECOVERY.md) | `ops/deploy/RECOVERY.md` |
| [canonical PostgreSQL v28 接管手册](../ops/deploy/LEGACY_V28_ADOPTION.md) | `ops/deploy/LEGACY_V28_ADOPTION.md` |
| [邮件诊断与审查证据](operations/mail-diagnostics.md) | `docs/operations/mail-diagnostics.md` |

## 数据库迁移

| 文档 | 仓库位置 |
| --- | --- |
| [数据库迁移导航](../migrations/README.md) | `migrations/README.md` |
| [PostgreSQL 完整档迁移](../migrations/postgres/README.md) | `migrations/postgres/README.md` |
| [SQLite Lite 迁移](../migrations/sqlite/README.md) | `migrations/sqlite/README.md` |

## 决策记录

既有笔记记录当时的决定与证据；当前实现缺口及核对快照见[状态说明](status.md)。

| 文档 | 仓库位置 |
| --- | --- |
| [Agent Note: 源码审计问题修复与恢复边界](../.agents/notes/implemented/bug-fix/2026-09-22-source-audit-remediation.md) | `.agents/notes/implemented/bug-fix/2026-09-22-source-audit-remediation.md` |
| [Agent Note: 可选单文件服务与首次配置后启动](../.agents/notes/implemented/architecture/2026-09-20-unified-server-supervisor.md) | `.agents/notes/implemented/architecture/2026-09-20-unified-server-supervisor.md` |
| [Agent Note: 全库文档重建与静态维护边界](../.agents/notes/implemented/process/2026-09-16-documentation-rebuild.md) | `.agents/notes/implemented/process/2026-09-16-documentation-rebuild.md` |
| [Agent Note: 发布包收敛到正式程序与通用配置](../.agents/notes/implemented/simplification/2026-09-18-release-artifact-scope.md) | `.agents/notes/implemented/simplification/2026-09-18-release-artifact-scope.md` |
| [Agent Note: 无配置首次启动与管理页初始化](../.agents/notes/implemented/feature/2026-09-17-first-run-setup.md) | `.agents/notes/implemented/feature/2026-09-17-first-run-setup.md` |
| [Agent Note: PostgreSQL 连接参数独立填写](../.agents/notes/implemented/feature/2026-09-18-database-connection-fields.md) | `.agents/notes/implemented/feature/2026-09-18-database-connection-fields.md` |
