# auction-jobs

Worker 任务编排：调度、重试、游标推进和生命周期任务。

源码布局：`src/lib.rs` 只保留模块声明和公共导出；Worker 生命周期与调度实现放在 `src/worker/`。

`run_persistent_until_shutdown()` 已接入持久化 `scheduled_rounds` claim、租约、尝试次数、
指数退避失败记录和 Ctrl-C 优雅退出；它通过生命周期应用端口执行 preview/open，不复制状态机。
`run_persistent_with_settlement_until_shutdown()` 在 PostgreSQL 完整档额外领取结算 workflow，
通过 `SettlementCommandPort` 执行稳定的 `settle_auction` 命令，并持久化完成 fence 与失败退避；
SQLite Lite 不启用结算 workflow。PostgreSQL 完整档还提供结算履约的 claim/fencing、
`pending → prepared → enqueued` 推进和 delivery outbox 的发送中/成功/失败边界；外部
`DeliveryTransport` 尚未由 Worker 装配，因此当前不会发出远程请求。实时消费 runner 已接入
Worker 循环，并通过 `realtime_consumer_offsets` / `realtime_failures` 提供连续游标、失败停靠
和显式 replay 边界；外部广播协议尚未冻结，因此装配根不注入 sink。论坛扫描、远程协议映射、
回执对账和完整外部投递重试仍将在后续阶段逐项接入。

## 边界约束

**不得创建第二套拍卖状态机。** 所有状态迁移必须调用 `auction-application` 的命令端口，
Worker 只负责「何时触发」和「失败后如何重试」。

Worker 游标和命令账本必须持久化：内存定时器或内存游标不足以支持重放、重启和故障转移。

外部投递发生在本地事务提交之后；外部失败不能回滚已经接受的竞价事实。

Lite 档下 Worker 实例数和并发都必须保持为一，但游标仍然要持久化。
入口接收配置层已校验的并发数；SQLite Lite 档固定为 1，不能依赖运行时自行降级。

本 crate 不暴露业务写入 HTTP 表面，也不包含 HTTP handler。
