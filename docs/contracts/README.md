# 契约导航与变更路径

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [验证状态](docs/status.md)

本目录登记版本化行为、数据所有权和正确性约束，与源码、迁移同步维护；实现与运行证据见[验证状态](docs/status.md)，路由或表存在不代表业务已验收。

## 契约入口

| 文档 | 冻结内容 | 源码 owner |
| --- | --- | --- |
| [HTTP](docs/contracts/routes.md) | Public/Admin 版本化路径、初始化、健康、认证、缓存、方法与输入输出边界；编号与统计只在路由契约维护 | [auction-http](crates/auction-http/README.md)、[auction-contracts](crates/auction-contracts/README.md) |
| [实时事件](docs/contracts/events.md) | kind/schema、Public v1、sequence、Catch-up 与快照恢复 | contracts、application realtime、HTTP 和 Public decoder |
| [数据库对象](docs/contracts/schema.md) | PostgreSQL 对象的逻辑 owner、SQL owner 与历史迁移范围 | [auction-storage](crates/auction-storage/README.md)、[迁移](migrations/postgres/README.md) |
| [领域不变量](docs/contracts/invariants.md) | I 编号、幂等、锁、金额、生命周期、审计与外部副作用 | [domain](crates/auction-domain/README.md)、[application](crates/auction-application/README.md) |

配置字段见[配置手册和逐字段 TOML 示例](ops/config/README.md)；页面列宽见[表格契约](docs/design/table-layouts.md)；敏感邮件证据见[诊断契约](ops/mail/diagnostics.md)。

## 修改顺序

1. 明确 owner、能力档位与受影响的不变量；兼容既有格式先确定映射边界。
2. 依次修改领域/端口/DTO、数据库、外部适配与 HTTP/Web/Worker 表面，并同步本目录及模块 README；已发布 wire 不兼容时新增版本，迁移只追加。
3. 运行静态门禁；需要运行态结论时另行取得授权、执行环境验证并保存证据。

数据库行不得直接作 DTO，`command_id` 不得当请求关联 ID，stream sequence 不得当状态版本，空成功响应不得掩盖能力缺失；外部未知结果与本地已提交事实必须分开表达。
