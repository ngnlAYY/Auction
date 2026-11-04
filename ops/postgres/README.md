# PostgreSQL 18 服务端基线（W10.8）

[文档导航](../../docs/README.md) · [运维手册](../README.md) · [配置参考](../config/README.md) · [备份与恢复](../deploy/RECOVERY.md)

本目录提供生产 PostgreSQL 18 的最小配置片段，不是可直接替换发行版配置的完整文件。`postgresql.conf.example` 只列出 Auction 依赖的项；合并前必须保留主机已有的内存、WAL、认证、复制与备份设置。任何需要重启的参数都受 [W7.5 停机门禁](../README.md#数据库连接预算与启用顺序)约束。

## 连接预算

| 项 | 数量 |
| --- | ---: |
| Public / Admin / Worker 服务池 | 3 × 16 = 48 |
| 维护池（2 个并行 CLI） | 2 × 4 = 8 |
| 外部 guard、备份、exporter、人工会话 | 8 |
| 计划普通连接合计 | 64 |
| `max_connections` | 100 |
| `superuser_reserved_connections` | 3 |
| 普通连接上限 | 97 |
| 未分配余量 | 33 |

`auction-cli config check --strict` 的 `database_budget` 是声明值，不执行 `SHOW`，也不证明线上实际 reserve。增加实例、并行维护或 exporter 前重新计算。

## 应用顺序

1. 先完成备份与 W7.5 时段检查；记录当前 `SHOW max_connections`、`SHOW shared_buffers`、`SHOW shared_preload_libraries` 与 `pg_stat_statements` 是否存在。
2. 合并 `postgresql.conf.example` 到实际配置；确认 `listen_addresses` 只含 `127.0.0.1`（或本机 Unix socket），不开放公网。
3. 在 W7.5 允许的窗口重启 PostgreSQL。
4. 在目标库执行 `extensions.sql.example`，确认 `pg_stat_statements` 可查询。
5. 执行 `autovacuum.sql.example`；按实际表规模复核参数，不要盲目套用。
6. 运行下列只读核验并保留输出：

```sql
SHOW max_connections;
SHOW superuser_reserved_connections;
SHOW shared_preload_libraries;
SHOW log_min_duration_statement;
SHOW log_lock_waits;
SELECT extname FROM pg_extension WHERE extname = 'pg_stat_statements';
SELECT relname, n_live_tup, n_dead_tup, last_autovacuum, last_autoanalyze
FROM pg_stat_user_tables
WHERE relname IN ('realtime_streams','lots','users','realtime_events','audit_records','deliveries','inbound_mail','bids')
ORDER BY relname;
```

## 归档与恢复

`archive_mode` 在 W10.8 保持 `off`；W10.7 再启用 `archive_mode = on`、`archive_timeout = 300` 与经过 W10.2 加密/上传的 `archive_command`。不要在本阶段提前打开归档，避免产生未纳入上传器的 WAL。

## 回退

恢复原 `postgresql.conf`，按 W7.5 重启；`pg_stat_statements` 扩展可保留，表级 autovacuum 参数可用 `ALTER TABLE ... RESET (...)` 回退。连接预算或 reserve 变化必须重新核对 `config check --strict`。
