# auction-process

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md)

原生进程与首次设置服务共用的生命周期原语，只依赖 tokio。

| 接口 | 说明 |
| --- | --- |
| `TerminationSignals` | 在装配依赖之前订阅 SIGINT 与 SIGTERM，订阅失败即阻断启动；可连续 `recv`，用于识别“排空期间的第二次信号” |
| `wait_for_drain(signals, timeout, drain)` | 等待 drain 完成、第二次终止信号或超时三者之一；信号与超时优先，完成回执复核原截止，返回 `DrainOutcome::{Completed, TimedOut, Interrupted}` |
| `DEFAULT_DRAIN_TIMEOUT` | 默认排空上限 10 秒 |
| `TaskGuard` | tokio 任务的 abort-on-drop 守卫：drain 被超时或二次信号取消时不会留下脱离的后台任务 |

三个角色的退出语义因此一致：第一次信号开始有界排空（停止接收新请求/新任务，等待在途工作），超时或第二次信号时强制中止剩余任务并以非零状态退出。systemd 示例 unit 的 `TimeoutStopSec=45s` 覆盖 10 秒排空加 tracing 刷新；`auction-server` 为三角色顺序关闭预留 135 秒。
