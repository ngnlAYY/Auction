# auction-cache

可选缓存边界。默认**完全关闭**，不编译任何缓存客户端。

源码布局：`src/lib.rs` 只保留模块声明和公共导出；缓存实现集中在 `src/cache/mod.rs`。

启用 Redis 需要两件事同时成立：

1. 目标进程以 `cache-redis` feature 构建；
2. 主配置中 `cache.backend = "redis"`，且敏感 TOML 提供 `redis_url`。

只填写 `redis_url` 不会把 Redis 依赖带入默认制品；只开 feature 不改配置也不会建立连接。
配置要求 Redis 但未编译该 feature 时返回明确错误，不静默降级。

## 正确性约束

缓存**不是**权威状态。只能保存带有限 TTL、可从数据库或外部权威服务重建的查询结果。

竞价事务、资格判断、命令幂等账本、锁、会话真值和审计证据必须由数据库决定。缓存 miss、
Redis 不可用或失效失败都必须回退到权威数据源，不得改变命令结果。

由此，未编译 Redis feature、运行时选择 `disabled` 或 Redis 临时故障都只影响性能，不改变
可用功能的正确性。

## 实现说明

直接复用 Redis 客户端的异步连接管理（`ConnectionManager` 可廉价克隆并复用同一多路复用
连接），不额外引入通用连接池，也不引入进程内缓存库。

错误文本和 `Debug` 输出都不包含 Redis URL 或凭据。

Public 规则读模型是当前首个消费者：按 `auction:read-model:public-rules:v1:<locale>` 读取和写入
通过校验的 JSON 快照，TTL 由 `cache.public_rules_ttl_seconds` 限制。Admin 规则发布成功后失效
对应语言；失效失败不会影响已提交事务，Public 仍会在 TTL 到期后回源数据库。

缓存装饰器不缓存空结果，命中快照解析失败会尽力删除并回源。禁用缓存、Redis 故障、序列化
失败或失效失败都只影响命中率，不改变规则、竞价、会话、幂等和审计的权威结果。
