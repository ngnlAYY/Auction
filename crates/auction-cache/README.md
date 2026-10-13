# auction-cache

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

默认关闭、按需启用 Redis 的可丢弃读模型缓存。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [启用条件](#启用条件)
- [正确性约束](#正确性约束)
- [实现说明](#实现说明)
- [维护与验证](#维护与验证)

## 源码入口

[src/cache/mod.rs](src/cache/mod.rs) 拥有 disabled/Redis 实现、操作预算、并发限制和脱敏错误；`lib.rs` 只负责导出。

## 职责与实现边界

实现应用层 `ReadModelCache` 端口，只保存可重建且带有限 TTL 的读取结果。默认不编译 Redis 客户端。

## 启用条件

目标进程以 `cache-redis` feature 构建（本 crate 对应 `redis`），公共 TOML 选择 `cache.backend = "redis"`，
敏感 TOML 提供 `redis_url`，三者缺一不可。只填 URL 或只开 feature 不会自动启用；配置要求 Redis 但未编译驱动时明确报错。
字段及模板见[配置手册](../../ops/config/README.md#缓存与拍卖模板)。

## 正确性约束

竞价、资格、幂等账本、锁、会话和审计以数据库为权威。miss、损坏、过期、Redis 故障或失效失败均须回源，
不能改变命令结果或撤销已提交的规则发布。缓存故障仅影响命中率和延迟。

## 实现说明

Redis 使用可克隆的 `ConnectionManager`，不另建通用池或进程内缓存。每次 get/set/invalidate 总预算为 750 毫秒，
最多 32 个在途操作，满额立即返回不可用；响应/连接内部超时分别为 500 毫秒/1 秒，总预算先到即结束调用。
错误与 `Debug` 不包含 URL 或凭据。

当前消费者是 Public 规则快照，键为 `auction:read-model:public-rules:v1:<locale>`。
只缓存有效非空文档，Admin 发布后按语言失效；已确认缓存不可用的请求回源后跳过回填，避免重复等待。

## 维护与验证

```bash
cargo check --locked -p auction-cache --no-default-features --features redis
```

仅静态编译；默认检查与禁止默认运行的测试范围见[开发指南](../../docs/development.md)，
运行证据见[验证状态](../../docs/status.md)。修改公共行为时同步对应契约。
