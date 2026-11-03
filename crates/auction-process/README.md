# auction-process

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md)

原生进程与首次设置服务共用的生命周期原语，只依赖 tokio。

| 接口 | 说明 |
| --- | --- |
| `TerminationSignals` | 在装配依赖之前订阅 SIGINT 与 SIGTERM，订阅失败即阻断启动；可连续 `recv`，用于识别“排空期间的第二次信号” |
| `wait_for_drain(signals, timeout, drain)` | 等待 drain 完成、第二次终止信号或超时三者之一；信号与超时优先，完成回执复核原截止，返回 `DrainOutcome::{Completed, TimedOut, Interrupted}` |
| `DEFAULT_DRAIN_TIMEOUT` | 保留通用调用方的 10 秒默认值；三个角色改由配置传入预算 |
| `DrainCoordinator` / `DrainActivity` | 关闭新登记入口，唤醒准备期工作；围栏前登记的交接和关键段均继续由原 owner poll、阻止排空结束，结果提交后 complete，未收口 drop 只减少活动计数并通知 observer |
| `TaskGuard` | tokio 任务的 abort-on-drop 守卫：drain 被超时或二次信号取消时不会留下脱离的后台任务 |

三个角色从角色 TOML 的 `[shutdown]` 加载普通预算（默认 10 秒）。Worker 在普通收口前，另以一次共享关键段预算（默认 60 秒）等待已登记交接或外部写入的叶子 owner；尚未登记的准备期工作立即取消。交接与关键段等待不替代任务 join，也不延长业务请求、租约或证据截止。第二次信号优先强制取消，非零退出并跳过可选 tracing 刷新；超时不改写持久状态。

协调器只依赖 Tokio，不读配置或理解业务种类。应用的窄生命周期端口由 Worker 装配根桥接；`enter_critical` 在围栏调用前登记交接，围栏调用开始即受保护。交接和关键段复用 `CRITICAL` 状态及活动计数，与 close 共用入口锁；关闭先赢时不发起围栏，登记先赢时排空会等待围栏往返。登记不授权外写，围栏提交应答未知时仍不能 POST。结果提交之后才调用 complete，活动 drop 不是持久化成功证明。配置、外层 unit 和回退的权威说明见[排空与停止预算](../../ops/config/README.md#排空与停止预算)。

服务 owner 回收后仍监听终止信号；tracing 刷新期间再收到信号时，装配根直接非零退出，避免不可取消的刷新线程延迟强退。
