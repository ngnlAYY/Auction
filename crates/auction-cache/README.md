# auction-cache

[文档导航](docs/README.md) · [配置参考](ops/config/README.md#缓存)

可选的读模型缓存。缓存不是事实来源：竞价、幂等账本、审计、outbox 与 Worker 游标全部在 PostgreSQL；缓存只保存可随时回源重建的读取投影（目前是 Public 规则文档）。

| 接口 | 说明 |
| --- | --- |
| `CacheStore::disabled()` | 不建立任何外部连接；所有读取视为未命中 |
| `CacheStore::redis(url)` | Redis 后端；首次命令时按需建立连接，复用单个多路复用连接，命令并发有上限，满额立即返回不可用并回源 |
| `get` / `set(ttl)` / `invalidate` | 原始字节读写与失效；任何错误或未命中由调用方回源数据库 |

`redis` feature 控制是否编译 Redis 客户端（rustls TLS，支持 `rediss://`）。配置要求 Redis 但二进制未编译该 feature 时，`CacheStore::redis` 明确报错而不是静默降级。错误文本不含 Redis URL 或凭据，也不保留服务器原文。

规则发布（Admin 或 `auction-cli rules import`）提交后失效对应语言的缓存键；失效失败不会回滚已提交的发布，最长等待 `public_rules_ttl_seconds` 后自然过期。
