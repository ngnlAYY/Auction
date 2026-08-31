# 数据库迁移

`postgres/` 保存完整生产 schema 的迁移，`sqlite/` 保存 Lite 档所需的 schema 子集。
两个目录不要求文件或数据库对象一一对应；两边都存在的领域对象必须保持名称和语义一致，
SQLite 未实现的完整生产能力必须在 Lite 能力清单或部署说明中明确。迁移只允许追加，已经
发布的文件禁止修改。已支持竞价事务的锁、唯一约束、审计事件和 outbox 语义必须与
[`docs/architecture.md`](../docs/architecture.md) 一起评审。

PostgreSQL `000001` 至 `000023` 已按新领域 owner 追加核心竞价、结算/履约、生命周期、
身份/外部命令、恢复/可观测性、论坛同步、MoogleMail 轮询、Final phase 调度、领先时间修正和
生命周期/operations alert/lot 结束时间数据库不变量、论坛未知写入的幂等人工再授权账本及
异世界 carrier dispatch 修正、旧 v28 bid INSERT 在滚动升级期间遗漏有效结束时间列的兼容投影，
已证明 MoogleMail 退信的独立 operations alert signal 闭集、operations alert 的非敏感
启用状态与成功扫描水位、配置状态发布时间与扫描水位的独立时钟语义，以及 HV 战斗页占用时
保留凭据和最后成功余额的 `busy` recovery 状态；
Admin A20 的全局唤醒由 Worker control 处理，不新增数据库触发字段。SQLite `000001` 至
`000008` 已提交共有 Lite 子集、轮次租约、用户管理、取消/生命周期元数据、Final phase fence、
领先时间和 lot 结束时间 shape 修正。两者仍只表示当前迁移阶段的 schema，不代表旧系统的全部行为或业务表面已经
等价；旧数据库表也不能未经审计地整体复制到任一后端。

除非用户明确要求，迁移目录不编写或运行功能测试用例；迁移结构变更仍需通过编译检查、
静态检查和人工审阅确认。
