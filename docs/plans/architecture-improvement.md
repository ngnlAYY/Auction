# 架构改进回顾

[docs/README.md](../README.md) · [docs/architecture.md](../architecture.md) · [docs/development.md](../development.md) · [ops/README.md](../../ops/README.md)

2026 年的架构改进已全部合入 `main`。本页只保留目标、原则与结果摘要；逐条用户决定（含被修订条目）在[docs/plans/architecture-improvement/decisions.md](architecture-improvement/decisions.md)只追加维护。正文路径以仓库根为起点。

## 目标与原则

在不削弱资金安全与审计保证的前提下，降低外部依赖的故障面、把人工处置收敛到证据，并建立可重复的发布、备份与安全基线。实施遵循 [AGENTS.md](../../AGENTS.md) 的全部领域不变量；每个交付批次一个阶段提交；默认门禁只做静态验证（`just verify`），行为变更运行受影响范围的测试；先可观测、再改行为，无预发环境时改变运行行为的改动带运行时开关与旧路径回退。

## 结果摘要

- 错误分类与可观测：类型化安全错误与 kind/phase 诊断门禁；HTTP、WebSocket、外部客户端与数据库事务指标；`127.0.0.1` 指标出口与 Prometheus 采集示例。
- 外部账号调度：Read Mail 独立循环与世界错峰、三层争用调度、Admin/CLI 统一经 Worker control、论坛按游标增量导入。
- 统一处置事实与规则上移：判失败、装备损失、confirm-sent 与告警收口共用处置事实；场次准入、状态机与排期改为纯函数判定。
- Admin 运维台：证据优先的详情、玩家支持视图、告警台确认/静默与处置手册锚点；外部 webhook 与存活心跳已实现但默认关闭。
- 发布与环境：GitHub Release 是唯一可部署来源；部署与回退串联严格预检、时段与在途发送门禁、迁移演练回执、双重校验与部署回执；运行时开关热生效。
- 数据生命周期：定时备份、WAL 归档与 R2 异机副本、迁移演练、恢复后外部核对；资产核对与只读一致性巡检。
- 回归门禁：源码计数棘轮、`unwrap`/`expect` 拒绝、release `overflow-checks`、安全 panic hook、文档链接检查与定时依赖审计。

## 验证边界

- `just verify` 是每次提交的最低要求，口径见[ci/checks/README.md](../../ci/checks/README.md)；行为变更随功能维护 `just test`，锁、隔离级别、幂等与迁移断言用 `just test-db` 在真实 PostgreSQL 18 上执行（见[docs/development.md#测试](../development.md#测试)）。
- 部署只经 GitHub Release 资产，流程与回退见[ops/README.md#升级](../../ops/README.md#升级)与[ops/deploy/RECOVERY.md](../../ops/deploy/RECOVERY.md)。
- 性能比较来自一次性隔离 PG18 的合成夹具，不是生产基线；影子差异、SLI 与告警处置由运维手册与模块文档拥有。

## 仍待决定

外部告警通道、存活心跳与 MoogleMail 运维通知默认关闭；Sent 发送基线、就绪审计与处置读侧模式保持影子口径，切换由用户显式决定。
