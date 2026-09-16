# 契约导航与变更路径

[文档导航](../README.md) · [架构](../architecture.md) · [验证状态](../status.md)

本目录拥有版本化行为的登记，不是外部实现的镜像。源码、迁移和本目录必须同步；
路由数量、表存在或 DTO 编译成功不能证明对应业务流程已验收。

## 契约入口

| 文档 | 冻结内容 | 源码 owner |
| --- | --- | --- |
| [HTTP](routes.md) | P01–P21、A01–A43、健康、认证、缓存、方法、状态与输入边界 | [auction-http](../../crates/auction-http/README.md)、[auction-contracts](../../crates/auction-contracts/README.md) |
| [实时事件](events.md) | kind/schema、Public v1、sequence、Catch-up 与快照恢复 | contracts、application realtime、HTTP 和 Public decoder |
| [数据库对象](schema.md) | 每个对象的逻辑 owner、SQL owner 和完整档/Lite 范围 | [auction-storage](../../crates/auction-storage/README.md)、[迁移](../../migrations/README.md) |
| [领域不变量](invariants.md) | I 编号、幂等、锁、金额、生命周期、审计与外部副作用 | [domain](../../crates/auction-domain/README.md)、[application](../../crates/auction-application/README.md) |

配置字段由[配置手册和逐字段 TOML 示例](../../ops/config/README.md)登记；
页面列宽见[表格契约](../table-layouts.md)；敏感邮件证据见[诊断契约](../operations/mail-diagnostics.md)。

## 修改顺序

1. 明确 owner、能力档位与受影响的不变量；兼容既有格式时先确定映射边界。
2. 修改领域/端口/DTO，再按需落地数据库、外部适配与 HTTP/Web/Worker 表面。
3. 同步本目录及模块 README；已发布 wire 不兼容时新增版本，迁移只追加。
4. 运行静态门禁；需要运行态结论时另行取得授权、执行相应环境验证并保存证据。

不把数据库行直接作为 DTO，不把 `command_id` 当请求关联 ID，不把 stream sequence 当状态版本，
也不把 SQLite 缺失能力用空成功响应掩盖。外部未知结果与本地已提交事实必须分开表达。
