//! 可重建 Public 读模型的缓存端口。
//! 应用层只定义不携带具体 Redis 依赖的原始字节端口；缓存适配器负责传输，读模型装饰器负责
//! 序列化、有限 TTL 和权威回源。这样缓存不会进入竞价、会话、幂等或审计正确性路径。

use std::{future::Future, pin::Pin, time::Duration};

/// 缓存后端错误的脱敏分类。
/// 具体 Redis 错误不能穿过应用边界，也不能因为缓存故障改变权威读取结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadModelCacheError {
    /// 后端不可用、命令失败或返回内容无法使用。
    BackendUnavailable,
}

/// 可重建读模型缓存操作的异步返回值。
pub type ReadModelCacheFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ReadModelCacheError>> + Send + 'a>>;

/// `value` 只保存已经脱敏的读模型字节；实现不得把该端口用于命令结果、会话、锁、幂等账本
/// 或审计证据。调用方必须在 miss、错误和失效失败时回到权威数据源。
pub trait ReadModelCache: Send + Sync {
    /// 按稳定 key 读取可重建快照。
    fn get(&self, key: String) -> ReadModelCacheFuture<'_, Option<Vec<u8>>>;

    /// 写入带有限 TTL 的快照。
    fn set(&self, key: String, value: Vec<u8>, ttl: Duration) -> ReadModelCacheFuture<'_, ()>;

    /// 删除一个已经变更的快照。
    fn invalidate(&self, key: String) -> ReadModelCacheFuture<'_, ()>;
}
