# 项目文档导航

[项目首页](../README.md)

新读者先看系统设计、架构和开发指南；界面修改进入对应设计，部署从配置和部署手册开始。
精确契约、操作命令和历史证据由各自文档维护，模块 README 提供源码入口和关键边界。
本页列出全部正式文档，不包含生成产物、本地工具状态或私有运行证据。新增、移动或删除文档时同步导航。

## 项目与开发

| 文档 | 用途 |
| --- | --- |
| [Auction Rust](../README.md) | 产品、进程入口与快速开始 |
| [Auction Rust 系统设计](../DESIGN.md) | 系统协作与核心数据流 |
| [AGENTS.md — Auction Rust](../AGENTS.md) | 必须遵守的协作与交付规则 |
| [系统架构与模块所有权](architecture.md) | 模块 owner、依赖方向和后端能力矩阵 |
| [技术栈与取舍](tech-stack.md) | 锁定版本、技术取舍与升级要求 |
| [开发与静态验证](development.md) | 工具准备、编译与静态验证命令 |
| [实现与验证状态](status.md) | 源码能力、历史检查和待补运行证据 |
| [文档维护](documentation.md) | 文档职责、去重与更新规则 |
| [静态验证、文档门禁与发布脚本](../ci/README.md) | 静态门禁、工作流和发布脚本入口 |

## 契约与界面

| 文档 | 用途 |
| --- | --- |
| [契约导航与变更路径](contracts/README.md) | 契约的查阅与变更路径 |
| [HTTP 路由与响应契约](contracts/routes.md) | API/浏览器入口、字段和状态码 |
| [实时事件与客户端恢复契约](contracts/events.md) | 事件登记、序号、WebSocket 与恢复 |
| [数据库对象与所有权契约](contracts/schema.md) | 表、账本、持久事实和唯一 owner |
| [领域不变量与写入语义](contracts/invariants.md) | 事务、幂等、竞价及外部副作用语义 |
| [Public 公开端设计](../crates/auction-web-public/DESIGN.md) | 公开端信息架构、视觉和竞价交互 |
| [Admin 管理端设计](../crates/auction-web-admin/DESIGN.md) | 管理端布局、表单、诊断和确认流程 |
| [表格布局与列宽契约](table-layouts.md) | 业务列顺序、百分比与长文本显示 |

## 应用进程

| 文档 | 用途 |
| --- | --- |
| [admin-api · auction-admin-api](../apps/admin-api/README.md) | 管理 API/页面及设置入口的装配 |
| [auctionctl](../apps/auctionctl/README.md) | 配置、迁移、运维 CLI 命令 |
| [public-api · auction-public-api](../apps/public-api/README.md) | 公开 API/页面及实时 relay 的装配 |
| [worker · auction-worker](../apps/worker/README.md) | 持久任务、外部集成及关闭顺序 |
| [server · auction-server](../apps/server/README.md) | 可选单文件 supervisor 与维护门禁 |

## 共享能力

| 文档 | 用途 |
| --- | --- |
| [auction-application](../crates/auction-application/README.md) | 用例、窄端口及事务编排 |
| [auction-cache](../crates/auction-cache/README.md) | disabled/Redis 适配与回源预算 |
| [auction-config](../crates/auction-config/README.md) | 配置解析、校验和 runtime 文件适配 |
| [auction-contracts](../crates/auction-contracts/README.md) | 稳定 DTO、错误和 wire 版本 |
| [auction-domain](../crates/auction-domain/README.md) | 领域实体、规则及纯计算 |
| [auction-http](../crates/auction-http/README.md) | HTTP 输入、安全边界及资源服务 |
| [auction-integrations](../crates/auction-integrations/README.md) | 论坛、邮件、装备和外部证明 |
| [auction-jobs](../crates/auction-jobs/README.md) | 调度、重试、游标和任务编排 |
| [auction-observability](../crates/auction-observability/README.md) | tracing、指标、健康与心跳投影 |
| [auction-process](../crates/auction-process/README.md) | 信号、有界 drain 和任务守卫 |
| [auction-setup](../crates/auction-setup/README.md) | 独立首次设置与初始化授权 |
| [auction-storage](../crates/auction-storage/README.md) | SQL、锁、迁移和后端实现边界 |
| [auction-web-admin · 管理端浏览器界面](../crates/auction-web-admin/README.md) | 管理端源码入口与 API 消费 |
| [auction-web-public · 公开端浏览器界面](../crates/auction-web-public/README.md) | 公开端源码入口与实时恢复 |
| [auction-web-shared · 浏览器端共享基础能力](../crates/auction-web-shared/README.md) | 通用组件、语言和传输机制 |

## 配置、部署与运维

| 文档 | 用途 |
| --- | --- |
| [运行配置与敏感文件](../ops/config/README.md) | 配置文件关系、敏感值与 runtime 接管 |
| [部署与发布手册](../ops/deploy/README.md) | 构建发布、预检、迁移和启动步骤 |
| [AMD KVM 定向编译候选](../ops/build/README.md) | AMD 定向参数、有限测量及候选证据 |
| [systemd 服务与顺序控制](../ops/deploy/systemd/README.md) | 服务模板、权限和启动/停止顺序 |
| [PostgreSQL 备份与恢复手册](../ops/deploy/RECOVERY.md) | 备份、校验、重建和恢复门禁 |
| [canonical PostgreSQL v28 接管手册](../ops/deploy/LEGACY_V28_ADOPTION.md) | 限定历史 schema 的显式接管 |
| [邮件诊断与审查证据](operations/mail-diagnostics.md) | 邮件诊断、安全证据与旧资产核对 |

## 数据库迁移

| 文档 | 用途 |
| --- | --- |
| [数据库迁移导航](../migrations/README.md) | 两后端迁移入口和兼容执行边界 |
| [PostgreSQL 完整档迁移](../migrations/postgres/README.md) | 完整档追加迁移与历史兼容 |
| [SQLite Lite 迁移](../migrations/sqlite/README.md) | Lite 迁移范围与本地写入限制 |
