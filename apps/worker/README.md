# auction-worker

持久化生命周期与集成 Worker 进程入口。

源码布局：src/main.rs 只负责 Tokio 入口，src/bootstrap/mod.rs 负责配置、缓存、安全心跳和
Worker 生命周期装配。

Worker **不监听业务写入 HTTP 表面**，只从配置读取数据库与缓存设置，然后运行
`auction_jobs::run_persistent_with_settlement_until_shutdown()`；没有数据库时才保留不执行任务的启动骨架。

```bash
AUCTION_CONFIG_FILE=ops/config/examples/worker.toml \
  cargo run -p auction-worker
```

Worker 配置没有 `listen_address`——该角色的默认监听地址为 `None`。

## Cargo feature

- `cache-redis`：编译 Redis 缓存客户端。
- `postgres`：编译 PostgreSQL 完整存储和持久化 Worker 调度。
- `sqlite`：编译 SQLite Lite 存储和持久化 Worker 调度。

## 部署约束

完整生产档（PostgreSQL）下 Worker 并发可扩展。Lite 档（SQLite）下实例数和并发都必须为一，
因为写入是串行事务；运行手册需说明文件锁、锁等待和备份假设。

有数据库时，轮次 due、租约、尝试次数、完成标记和失败原因写入 `scheduled_rounds`；
PostgreSQL 完整档还把结算 workflow 的领取、稳定命令和完成 fence 写入 `auctions` 与
`auction_commands`，并把结算履约的 claim/fencing、delivery 准备/激活和 delivery outbox
终态写入数据库；SQLite Lite 明确不启用结算 workflow 与完整履约。当前装配尚未提供外部
`DeliveryTransport`，因此不会发送 MoogleMail 请求。Worker 在数据库可用时发布持久化启动/就绪/
周期/停止心跳；内存 interval 只负责唤醒，不能代替持久化游标。没有数据库的启动骨架不执行任何任务。
