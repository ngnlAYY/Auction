# 项目文档导航

[项目首页](../README.md) · [运维入口](../ops/README.md) · [CI 入口](../ci/README.md)

按任务找入口，不必逐文件阅读。系统设计与 owner 统一在架构，开发命令与文档维护统一在开发指南，
部署和 systemd 安装统一在运维入口；精确契约和历史证据保持独立。

## 按任务阅读

| 想做什么 | 入口 |
| --- | --- |
| 玩家注册、寄售、竞价、付款或返还 | [中文玩家指南](player-guide.md) |
| 理解系统数据流、模块归属与能力 | [设计与架构](architecture.md) |
| 准备工具、编译、静态检查 | [开发指南](development.md) |
| 查询依赖、MSRV、编译参数与取舍 | [技术栈](tech-stack.md) |
| 配置、部署、备份恢复或排错 | [运维入口](../ops/README.md) |
| 理解 CI、审计或发布脚本 | [CI 说明](../ci/README.md) |
| 区分已实现、历史验证和未验收 | [状态页](status.md) |
| 修改项目或维护文档 | [协作规范](../AGENTS.md)、[文档维护](development.md#文档维护) |

## 契约与界面

这些文件按不同 owner 保留，不能混成一份通用大手册。变更顺序见[契约导航](contracts/README.md)。

| 文档 | 查什么 |
| --- | --- |
| [HTTP](contracts/routes.md) | 路由、字段、状态码与安全边界 |
| [实时事件](contracts/events.md) | 事件版本、sequence、WebSocket 与恢复 |
| [数据库](contracts/schema.md) | 表、账本、持久事实与唯一 owner |
| [领域不变量](contracts/invariants.md) | 幂等、事务、锁、竞价、审计与外部副作用 |
| [Public 设计](../crates/auction-web-public/DESIGN.md) / [Admin 设计](../crates/auction-web-admin/DESIGN.md) | 各自页面、视觉与交互 |
| [表格布局](table-layouts.md) | 列顺序、百分比、长文本与窄屏结构 |

## 模块与专项手册

| 范围 | 入口与用途 |
| --- | --- |
| apps/* README | [进程表](../README.md#运行边界)列出五个程序，各模块 README 说明装配与命令 |
| crates/* README | [owner 表](architecture.md#crate-所有权)链接全部能力模块，不在此重复列一份 |
| 配置示例 | [配置手册](../ops/config/README.md)：逐字段说明、公共/角色/敏感/runtime 分层 |
| 指定硬件构建 | [AMD KVM 候选](../ops/build/README.md)：两份 CPU 配置及有限历史证据 |
| 数据库灾备 | [恢复手册](../ops/deploy/RECOVERY.md)、[v28 接管](../ops/deploy/LEGACY_V28_ADOPTION.md)：不同前提的受控流程 |
| 邮件排错 | [诊断与证据](../ops/mail-diagnostics.md)：加密原文、安全投影与未知资产核对 |
| 数据库迁移 | [迁移导航](../migrations/README.md)、[PostgreSQL](../migrations/postgres/README.md)：追加迁移及历史文件边界 |

本导航通过上述入口覆盖全部正式文档，不包含本地工具状态、生成产物和私有证据。
新增、移动或删除时同步所属入口并运行 `just docs-check`；链接检查不证明业务或运行验收。
