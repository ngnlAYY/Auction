# 项目文档导航

[项目首页](README.md) · [运维入口](ops/README.md) · [CI 入口](ci/README.md)

按任务找入口，不必逐文件阅读。设计、owner 在架构，开发命令与文档维护在开发指南，部署与 systemd 在运维入口，精确契约和历史证据保持独立。

## 按任务阅读
- 玩家注册、寄售、竞价、付款或返还：[中文玩家指南](docs/guide/player-guide.md)。
- 系统数据流、模块归属与能力：[设计与架构](docs/design/architecture.md)；依赖、MSRV 与编译取舍：[技术栈](docs/design/tech-stack.md)。
- 工具准备、编译、静态检查与 CI 发布：[开发指南](docs/guide/development.md)、[CI 说明](ci/README.md)。
- 配置、部署、备份恢复或排错：[运维入口](ops/README.md)；追加式迁移：[PostgreSQL](migrations/postgres/README.md)。
- 已实现、历史验证与未验收边界：[状态页](docs/status.md)；修改项目或维护文档：[协作规范](AGENTS.md)、[文档维护](docs/guide/development.md#文档维护)。

## 契约与界面

这些文件按不同 owner 保留，不能混成一份通用大手册；变更顺序见[契约导航](docs/contracts/README.md)。

- [HTTP](docs/contracts/routes.md)：路由、字段、状态码与安全边界；[实时事件](docs/contracts/events.md)：事件版本、sequence、WebSocket 与恢复。
- [数据库](docs/contracts/schema.md)：表、账本、持久事实与唯一 owner；[领域不变量](docs/contracts/invariants.md)：幂等、事务、锁与外部副作用。
- [Public 设计](crates/auction-web-public/DESIGN.md) / [Admin 设计](crates/auction-web-admin/DESIGN.md)：各自页面、视觉与交互；[表格布局](docs/design/table-layouts.md)：列顺序、百分比与窄屏。

## 模块与专项手册

- 进程与模块：[进程表](README.md#运行边界)列出五个程序，各 apps/*、crates/* README 说明装配与能力，[owner 表](docs/design/architecture.md#crate-所有权)不在此重复。
- 配置与构建：[配置手册](ops/config/README.md)逐字段说明公共/角色/敏感/runtime 分层；[AMD KVM 候选](ops/build/README.md)两份 CPU 配置及有限历史证据。
- 运维专项：[恢复手册](ops/deploy/RECOVERY.md)、[v28 接管](ops/deploy/LEGACY_V28_ADOPTION.md)、[邮件诊断](ops/mail/diagnostics.md)、[迁移导航](migrations/postgres/README.md)。

本导航通过上述入口覆盖全部正式文档，不含本地工具状态、生成产物和私有证据；新增、移动或删除时同步所属入口，并人工核对全部链接与锚点。
