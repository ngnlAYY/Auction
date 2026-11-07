# W13 回归门禁

[docs/plans/architecture-improvement.md](../architecture-improvement.md) · [docs/plans/architecture-improvement.md#2-执行须知](../architecture-improvement.md#2-执行须知) · [docs/plans/architecture-improvement.md#5-每个工作项的交付规范](../architecture-improvement.md#5-每个工作项的交付规范) · [docs/plans/architecture-improvement.md#7-进度](../architecture-improvement.md#7-进度) · [docs/plans/architecture-improvement/baseline.md](baseline.md) · [docs/plans/architecture-improvement/decisions.md](decisions.md)

| 项 | 内容 |
| --- | --- |
| 范围 | 回归门禁：计数棘轮、panic 与溢出防护、文档链接检查、定时依赖审计、依赖升级节奏 |
| 优先级 | P1 |
| 里程碑 | W13.1–W13.5 随 M1；W13.6 随 M6 |
| 依赖 | — |
| 迁移 | 否 |
| 估算 | 3–4 天 |
| 关联缺陷 | —（详见[docs/plans/architecture-improvement.md#已知缺陷](../architecture-improvement.md#已知缺陷)） |
| 关联决策 | 11、46（详见[docs/plans/architecture-improvement/decisions.md](decisions.md)） |

| 项 | 内容 |
| --- | --- |
| W13.1 | `ci/checks/check.sh` 新增计数棘轮：`#[path]`、`use super::*`，以及 jobs、应用、HTTP、存储、集成与 apps 各层的 `Err(_)` 与 `map_err(\|_\| …)` 数量只降不升（存储层 1430 处、集成层 359 处同样纳入，W0.8 的下降随之锁定）；基线文件随仓库提交，下降时同步更新 |
| W13.2 | workspace clippy `unwrap_used`、`expect_used` 设为 deny，测试代码豁免 |
| W13.3 | release profile 开启 `overflow-checks`：整数溢出转为任务 panic、失败关闭，不再静默回绕；评估对 `panic = "unwind"` 任务隔离与 Worker 常驻任务退出语义的影响；工作项执行中的 panic 计入该项尝试次数，超过上限转人工，不随租约到期无限重领 |
| W13.4 | 文档静态检查（M1）：Markdown 相对链接目标存在、同页锚点存在（不访问网络），并入 `just verify`；须先于 W5.4 让告警正文引用处置手册锚点 |
| W13.5 | 定时依赖审计（M1）：CI 每周对锁定依赖运行 cargo-audit，W7.1 后同时审计最近一个发布 tag；新公告以失败作业提示，不自动改依赖 |
| W13.6 | 依赖与工具链升级节奏（M6）：按发布冻结日历每季度安排一次升级窗口（Rust 工具链、Tokio LTS、Axum 与 tungstenite、sqlx、Dioxus），dependabot 只作提示；升级版本运行 W6 测试，涉及数据库或 HTTP 栈时加跑 W12.3 负载演练 |

验收：`just verify` 包含新检查；人为增加一处 `#[path]`、`unwrap()` 或断链时门禁失败；定时审计作业按周运行；下一次升级窗口写入运维手册的发布日历。回退：移除对应检查即可，不影响运行时（W13.3 可单独回退 profile）。
