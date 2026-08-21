# auction-worker

持久化生命周期与集成 Worker 进程入口。

Worker **不监听业务写入 HTTP 表面**，只从配置读取数据库与缓存设置，然后运行
`auction_jobs::run_until_shutdown()`。

```bash
AUCTION_CONFIG_FILE=ops/config/examples/worker.toml \
  cargo run -p auction-worker
```

Worker 配置没有 `listen_address`——该角色的默认监听地址为 `None`。

## Cargo feature

- `cache-redis`：编译 Redis 缓存客户端。

## 部署约束

完整生产档（PostgreSQL）下 Worker 并发可扩展。Lite 档（SQLite）下实例数和并发都必须为一，
因为写入是串行事务；运行手册需说明文件锁、锁等待和备份假设。

任何情况下 Worker 游标都必须持久化，内存游标不足以支持重放和重启。
