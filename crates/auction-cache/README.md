# auction-cache

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

默认关闭、按需启用 Redis 的可丢弃读模型缓存，实现应用层 `ReadModelCache` 端口，只保存可重建且带有限 TTL 的读取结果。

## 源码入口

[src/cache/mod.rs](crates/auction-cache/src/cache/mod.rs) 拥有 disabled/Redis 实现、操作预算、并发限制和脱敏错误；`lib.rs` 只负责导出。

## 职责与实现边界

默认不编译 Redis 客户端；本 crate 不持有业务状态、不建通用池，缓存故障只影响命中率和延迟。

## 启用条件

目标进程以 `cache-redis` feature 构建（本 crate 对应 `redis`），公共 TOML 选择 `cache.backend = "redis"`，
敏感 TOML 提供 `redis_url`，三者缺一不可。只填 URL 或只开 feature 不会自动启用；配置要求 Redis 但未编译驱动时
明确报错。字段及模板见[配置手册](ops/config/README.md#缓存与拍卖模板)。

## 正确性约束

竞价、资格、幂等账本、锁、会话和审计以数据库为权威。读取 miss、损坏、过期或 Redis 故障须回源，
不能改变命令结果；缓存失联不撤销已提交的规则发布。

## 实现说明

Redis 使用可克隆的 `ConnectionManager`。每次 get/set/invalidate 总预算 750 毫秒、最多 32 个在途操作，满额立即
返回不可用；响应/连接内部超时分别为 500 毫秒/1 秒，总预算先到即结束调用，错误与 `Debug` 不含 URL 或凭据。
并发配额以 `try_acquire` 非阻塞取得：适配器不为满额请求建立等待队列。许可由操作 future 持有，成功、失败、
总预算到期或调用方取消都会释放；超时只说明调用未得到确认，不能推断已经发送的 Redis 命令没有执行。
不增加后台补写或重试任务，SET/DEL 的结果不确定仍不能影响数据库命令结果。
适配器只暴露闭集错误，不持有原始 Redis 错误；`Display`、`Debug` 及 `Error::source` 都不会传播
服务器错误正文或连接细节。应用端口仍统一映射为 `ReadModelCacheError::BackendUnavailable`。
当前消费者是 Public 规则快照（键 `auction:read-model:public-rules:v1:<locale>`）：只缓存有效非空文档，
Admin 发布后按语言失效，GET 或损坏命中后的删除已确认不可用时，权威回源成功后跳过回填。
失效失败不会通知其他进程绕过已有缓存，旧值可能保留至其 TTL 到期。即使删除成功，发布前启动的权威读取
也可能在删除后把旧快照重新写入缓存；该值最多保留此次回填的 TTL，不能把删除成功视为线性一致的读取保证。

## 维护与验证

```bash
cargo check --locked -p auction-cache --no-default-features --features redis
```

仅静态编译；默认检查与禁止默认运行的测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
